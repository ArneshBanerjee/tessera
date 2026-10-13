//! Compiles CQL, Confluence's query language, into SQL over `content c JOIN spaces s`.
//!
//! CQL has JQL's grammar, so the JQL parser reads it; this module gives the fields their
//! Confluence meaning. User input reaches SQL only through bound values.
//!
//! Supported fields: `type`, `space` (and `space.key`, `space.id`, `space.title`, `space.type`), `title`,
//! `text`, `id`, `content`, `ancestor`, `parent`, `container`, `label`, `creator`,
//! `contributor`, `created`, and `lastmodified`. Functions: `currentUser()`, `now()`, and
//! `startOf…`/`endOf…` for days, weeks, months, and years, with offsets such as `"-1w"`.

use tessera_jql::ast::{Clause, Direction, Field, Operand, Operator, Query, Terminal};

use crate::jql::{Bind, like_pattern};
use crate::store::users::User;
use crate::time;

/// What a compiled query runs over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `content c JOIN spaces s ON s.id = c.space_id`.
    Content,
    /// `spaces s`, for `type = space` in `/search`.
    Spaces,
}

pub struct CqlContext<'a> {
    pub users: &'a [User],
    pub current_user: Option<i64>,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub filter: String,
    pub binds: Vec<Bind>,
    pub order_by: String,
    /// Whether the query can match spaces, for `/search` (`type = space`).
    pub spaces: bool,
}

/// A CQL error. Confluence reports all of them as unparsable CQL.
#[derive(Debug)]
pub struct CqlError(pub String);

type Result<T> = std::result::Result<T, CqlError>;

pub fn compile(cql: &str, ctx: &CqlContext, target: Target) -> Result<Compiled> {
    let unparsable = || CqlError(format!("Could not parse cql : {cql}"));
    let query: Query = tessera_jql::parse(cql).map_err(|_| unparsable())?;
    let mut compiler = Compiler {
        ctx,
        target,
        binds: Vec::new(),
    };
    let filter = match &query.filter {
        Some(clause) => compiler
            .clause(clause)
            .map_err(|e| CqlError(format!("{} : {}", unparsable().0, e.0)))?,
        None => "1 = 1".to_string(),
    };
    let mut order = Vec::new();
    for item in &query.order_by {
        let Field::Name(name) = &item.field else {
            return Err(unparsable());
        };
        let spaces = target == Target::Spaces;
        let column = match name.to_lowercase().as_str() {
            "title" if spaces => "s.name COLLATE NOCASE",
            "title" => "c.title COLLATE NOCASE",
            "created" | "lastmodified" if spaces => "s.created_at",
            "created" => "c.created_at",
            "lastmodified" => "c.updated_at",
            "id" | "content" if spaces => "s.id",
            "id" | "content" => "c.id",
            "type" if spaces => "s.key",
            "type" => "c.type",
            "space" | "space.key" => "s.key COLLATE NOCASE",
            "space.title" => "s.name COLLATE NOCASE",
            _ => return Err(unparsable()),
        };
        let direction = match item.direction {
            Some(Direction::Desc) => "DESC",
            _ => "ASC",
        };
        order.push(format!("{column} {direction}"));
    }
    let (default_order, tie_break) = match target {
        Target::Content => ("c.updated_at DESC", "c.id DESC"),
        Target::Spaces => ("s.key COLLATE NOCASE", "s.id"),
    };
    if order.is_empty() {
        order.push(default_order.into());
    }
    order.push(tie_break.into());
    Ok(Compiled {
        spaces: mentions_space_type(query.filter.as_ref()),
        filter,
        binds: compiler.binds,
        order_by: order.join(", "),
    })
}

/// Whether the query has `type = space` (or `type IN (..., space)`).
fn mentions_space_type(clause: Option<&Clause>) -> bool {
    match clause {
        Some(Clause::And(items)) | Some(Clause::Or(items)) => {
            items.iter().any(|c| mentions_space_type(Some(c)))
        }
        Some(Clause::Not(_)) | None => false,
        Some(Clause::Terminal(t)) => {
            matches!(&t.field, Field::Name(n) if n.eq_ignore_ascii_case("type"))
                && matches!(t.operator, Operator::Equals | Operator::In)
                && t.operand
                    .as_ref()
                    .is_some_and(|o| values(o).iter().any(|v| v.eq_ignore_ascii_case("space")))
        }
    }
}

/// The literal values of an operand, ignoring functions.
fn values(operand: &Operand) -> Vec<String> {
    match operand {
        Operand::Value { text, .. } => vec![text.clone()],
        Operand::List(items) => items.iter().flat_map(values).collect(),
        Operand::Empty | Operand::Function { .. } => Vec::new(),
    }
}

struct Compiler<'a> {
    ctx: &'a CqlContext<'a>,
    target: Target,
    binds: Vec<Bind>,
}

/// Matches nothing, for fields that spaces do not have.
const NEVER: &str = "(0 = 1)";

impl Compiler<'_> {
    fn clause(&mut self, clause: &Clause) -> Result<String> {
        Ok(match clause {
            Clause::And(items) => self.join(items, " AND ")?,
            Clause::Or(items) => self.join(items, " OR ")?,
            Clause::Not(inner) => format!("NOT ({})", self.clause(inner)?),
            Clause::Terminal(terminal) => self.terminal(terminal)?,
        })
    }

    fn join(&mut self, items: &[Clause], separator: &str) -> Result<String> {
        let parts = items
            .iter()
            .map(|c| self.clause(c))
            .collect::<Result<Vec<_>>>()?;
        Ok(format!("({})", parts.join(separator)))
    }

    fn terminal(&mut self, terminal: &Terminal) -> Result<String> {
        let Field::Name(name) = &terminal.field else {
            return Err(CqlError("custom fields are not supported".into()));
        };
        if !terminal.predicates.is_empty() {
            return Err(CqlError("history predicates are not supported".into()));
        }
        let operand = terminal
            .operand
            .as_ref()
            .ok_or_else(|| CqlError(format!("{name} needs a value")))?;
        let op = terminal.operator;
        if self.target == Target::Spaces {
            return self.space_terminal(name, op, operand);
        }
        match name.to_lowercase().as_str() {
            "type" => self.text_in("c.type", op, operand, |v| v.to_lowercase()),
            "space" | "space.key" => {
                self.text_in("s.key COLLATE NOCASE", op, operand, str::to_string)
            }
            "space.id" => self.ids("s.id", op, operand),
            "space.type" => self.text_in("s.type", op, operand, |v| v.to_lowercase()),
            "space.title" => self.text_field("s.name", op, operand),
            "title" => self.text_field("c.title", op, operand),
            "text" => self.text_search(op, operand),
            "id" | "content" => self.ids("c.id", op, operand),
            "parent" => self.ids("c.parent_id", op, operand),
            "container" => self.ids("c.container_id", op, operand),
            "ancestor" => self.ancestor(op, operand),
            "label" => self.label(op, operand),
            "creator" => self.users_in("c.creator_id", op, operand),
            "contributor" => self.contributor(op, operand),
            "created" => self.date("c.created_at", op, operand),
            "lastmodified" => self.date("c.updated_at", op, operand),
            other => Err(CqlError(format!("unknown field {other}"))),
        }
    }

    /// A clause evaluated against a space instead of content.
    fn space_terminal(&mut self, name: &str, op: Operator, operand: &Operand) -> Result<String> {
        match name.to_lowercase().as_str() {
            "type" => self.text_in("'space'", op, operand, |v| v.to_lowercase()),
            "space" | "space.key" => {
                self.text_in("s.key COLLATE NOCASE", op, operand, str::to_string)
            }
            "space.id" => self.ids("s.id", op, operand),
            "space.type" => self.text_in("s.type", op, operand, |v| v.to_lowercase()),
            "title" | "space.title" => self.text_field("s.name", op, operand),
            "text" => {
                if op != Operator::Contains && op != Operator::NotContains {
                    return Err(CqlError("text supports only ~ and !~".into()));
                }
                let condition =
                    self.words(&["s.name", "s.description"], &values(operand).join(" "))?;
                Ok(if op == Operator::Contains {
                    condition
                } else {
                    format!("NOT {condition}")
                })
            }
            "id" | "content" => self.ids("s.id", op, operand),
            "creator" => self.users_in("s.creator_id", op, operand),
            "created" | "lastmodified" => self.date("s.created_at", op, operand),
            "parent" | "container" | "ancestor" | "label" | "contributor" => {
                // Validate the operand, then match nothing.
                let _ = values(operand);
                Ok(NEVER.to_string())
            }
            other => Err(CqlError(format!("unknown field {other}"))),
        }
    }

    fn bind_text(&mut self, value: String) -> &'static str {
        self.binds.push(Bind::Text(value));
        "?"
    }

    fn placeholders(&mut self, items: Vec<Bind>) -> String {
        let n = items.len();
        self.binds.extend(items);
        vec!["?"; n].join(", ")
    }

    /// `=`, `!=`, `IN`, and `NOT IN` on a text column.
    fn text_in(
        &mut self,
        column: &str,
        op: Operator,
        operand: &Operand,
        normalize: impl Fn(&str) -> String,
    ) -> Result<String> {
        let list = values(operand);
        if list.is_empty() {
            return Err(CqlError("expected a value".into()));
        }
        let binds = list.iter().map(|v| Bind::Text(normalize(v))).collect();
        let marks = self.placeholders(binds);
        match op {
            Operator::Equals | Operator::In => Ok(format!("{column} IN ({marks})")),
            Operator::NotEquals | Operator::NotIn => Ok(format!("{column} NOT IN ({marks})")),
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    /// Exact match and `~` word search on a text column such as the title.
    fn text_field(&mut self, column: &str, op: Operator, operand: &Operand) -> Result<String> {
        match op {
            Operator::Contains | Operator::NotContains => {
                let text = values(operand).join(" ");
                let condition = self.words(&[column], &text)?;
                Ok(if op == Operator::Contains {
                    condition
                } else {
                    format!("NOT {condition}")
                })
            }
            _ => self.text_in(
                &format!("{column} COLLATE NOCASE"),
                op,
                operand,
                str::to_string,
            ),
        }
    }

    /// Every word of `text` must appear in one of `columns` (case-insensitive).
    fn words(&mut self, columns: &[&str], text: &str) -> Result<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return Err(CqlError("expected text to search for".into()));
        }
        let haystack = columns
            .iter()
            .map(|c| format!("COALESCE({c}, '')"))
            .collect::<Vec<_>>()
            .join(" || ' ' || ");
        let mut parts = Vec::new();
        for word in words {
            let mark = self.bind_text(format!("%{}%", like_pattern(word)));
            parts.push(format!("({haystack}) LIKE {mark} ESCAPE '\\'"));
        }
        Ok(format!("({})", parts.join(" AND ")))
    }

    /// `text ~`: the title, the body, and the labels.
    fn text_search(&mut self, op: Operator, operand: &Operand) -> Result<String> {
        if op != Operator::Contains && op != Operator::NotContains {
            return Err(CqlError("text supports only ~ and !~".into()));
        }
        let labels = "(SELECT group_concat(l.name, ' ') FROM content_labels cl JOIN labels l ON l.id = cl.label_id WHERE cl.content_id = c.id)";
        let condition = self.words(
            &["c.title", "c.body_text", labels],
            &values(operand).join(" "),
        )?;
        Ok(if op == Operator::Contains {
            condition
        } else {
            format!("NOT {condition}")
        })
    }

    fn id_binds(&self, operand: &Operand) -> Result<Vec<Bind>> {
        values(operand)
            .iter()
            .map(|v| {
                v.parse::<i64>()
                    .map(Bind::Int)
                    .map_err(|_| CqlError(format!("{v} is not an id")))
            })
            .collect()
    }

    fn ids(&mut self, column: &str, op: Operator, operand: &Operand) -> Result<String> {
        let binds = self.id_binds(operand)?;
        if binds.is_empty() {
            return Err(CqlError("expected an id".into()));
        }
        let marks = self.placeholders(binds);
        match op {
            Operator::Equals | Operator::In => Ok(format!("{column} IN ({marks})")),
            Operator::NotEquals | Operator::NotIn => {
                Ok(format!("({column} IS NULL OR {column} NOT IN ({marks}))"))
            }
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    /// Content anywhere below the given pages.
    fn ancestor(&mut self, op: Operator, operand: &Operand) -> Result<String> {
        let binds = self.id_binds(operand)?;
        if binds.is_empty() {
            return Err(CqlError("expected an id".into()));
        }
        let marks = self.placeholders(binds);
        let below = format!(
            "c.id IN (WITH RECURSIVE below(id) AS (
                 SELECT id FROM content WHERE parent_id IN ({marks})
                 UNION SELECT x.id FROM content x JOIN below b ON x.parent_id = b.id
             ) SELECT id FROM below)"
        );
        match op {
            Operator::Equals | Operator::In => Ok(below),
            Operator::NotEquals | Operator::NotIn => Ok(format!("NOT {below}")),
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    fn label(&mut self, op: Operator, operand: &Operand) -> Result<String> {
        let list = values(operand);
        if list.is_empty() {
            return Err(CqlError("expected a label".into()));
        }
        let marks = self.placeholders(list.iter().map(|v| Bind::Text(v.to_lowercase())).collect());
        let has = format!(
            "EXISTS (SELECT 1 FROM content_labels cl JOIN labels l ON l.id = cl.label_id
                     WHERE cl.content_id = c.id AND l.name IN ({marks}))"
        );
        match op {
            Operator::Equals | Operator::In => Ok(has),
            Operator::NotEquals | Operator::NotIn => Ok(format!("NOT {has}")),
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    /// User ids for account ids, emails, or `currentUser()`.
    fn user_binds(&self, operand: &Operand) -> Result<Vec<Bind>> {
        let mut out = Vec::new();
        let items = match operand {
            Operand::List(items) => items.clone(),
            other => vec![other.clone()],
        };
        for item in &items {
            match item {
                Operand::Function { name, .. } if name.eq_ignore_ascii_case("currentUser") => {
                    // Anonymous searches match no one.
                    out.push(Bind::Int(self.ctx.current_user.unwrap_or(-1)));
                }
                Operand::Value { text, .. } => {
                    let user = self
                        .ctx
                        .users
                        .iter()
                        .find(|u| u.account_id == *text || u.email.eq_ignore_ascii_case(text));
                    out.push(Bind::Int(user.map_or(-1, |u| u.id)));
                }
                _ => return Err(CqlError("expected a user".into())),
            }
        }
        Ok(out)
    }

    fn users_in(&mut self, column: &str, op: Operator, operand: &Operand) -> Result<String> {
        let binds = self.user_binds(operand)?;
        let marks = self.placeholders(binds);
        match op {
            Operator::Equals | Operator::In => Ok(format!("{column} IN ({marks})")),
            Operator::NotEquals | Operator::NotIn => Ok(format!("{column} NOT IN ({marks})")),
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    fn contributor(&mut self, op: Operator, operand: &Operand) -> Result<String> {
        let binds = self.user_binds(operand)?;
        let marks = self.placeholders(binds);
        let edited = format!(
            "EXISTS (SELECT 1 FROM content_versions v WHERE v.content_id = c.id AND v.author_id IN ({marks}))"
        );
        match op {
            Operator::Equals | Operator::In => Ok(edited),
            Operator::NotEquals | Operator::NotIn => Ok(format!("NOT {edited}")),
            _ => Err(CqlError(format!("operator {op} is not supported here"))),
        }
    }

    /// A point in time: a date, a date and time, a relative time such as `-4w`, or a function.
    fn instant(&self, operand: &Operand) -> Result<i64> {
        let now = self.ctx.now;
        match operand {
            Operand::Value { text, .. } => time::parse_jql_datetime(text)
                .or_else(|| time::parse_relative(text, now))
                .ok_or_else(|| CqlError(format!("{text} is not a date"))),
            Operand::Function { name, args } => {
                let lower = name.to_lowercase();
                // An optional offset such as "-1w" moves the starting point first.
                let base = match args.first() {
                    Some(arg) => time::parse_relative(arg, now)
                        .ok_or_else(|| CqlError(format!("{arg} is not a time offset")))?,
                    None => now,
                };
                let unit_time = |prefix: &str| -> Option<i64> {
                    let unit = lower.strip_prefix(prefix)?;
                    if prefix == "startof" {
                        time::start_of(unit, base, 0)
                    } else {
                        time::end_of(unit, base, 0)
                    }
                };
                match lower.as_str() {
                    "now" => Ok(base),
                    _ => unit_time("startof")
                        .or_else(|| unit_time("endof"))
                        .ok_or_else(|| CqlError(format!("unknown function {name}"))),
                }
            }
            _ => Err(CqlError("expected a date".into())),
        }
    }

    fn date(&mut self, column: &str, op: Operator, operand: &Operand) -> Result<String> {
        let at = self.instant(operand)?;
        // A bare date means the whole day for `=`.
        let whole_day = matches!(operand, Operand::Value { text, .. } if time::parse_date(text).is_some() && !text.contains(':'));
        self.binds.push(Bind::Int(at));
        Ok(match op {
            Operator::Equals if whole_day => {
                self.binds.push(Bind::Int(at + 86_400_000));
                format!("({column} >= ? AND {column} < ?)")
            }
            Operator::Equals => format!("{column} = ?"),
            Operator::NotEquals => format!("{column} <> ?"),
            Operator::GreaterThan => format!("{column} > ?"),
            Operator::GreaterThanEquals => format!("{column} >= ?"),
            Operator::LessThan => format!("{column} < ?"),
            Operator::LessThanEquals => format!("{column} <= ?"),
            _ => {
                return Err(CqlError(format!(
                    "operator {op} is not supported for dates"
                )));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_ok(cql: &str) -> Compiled {
        let ctx = CqlContext {
            users: &[],
            current_user: Some(1),
            now: 0,
        };
        compile(cql, &ctx, Target::Content).unwrap_or_else(|e| panic!("{cql}: {}", e.0))
    }

    #[test]
    fn compiles_common_queries() {
        let c = compile_ok(
            "type = page AND space = DEV AND title ~ \"release notes\" ORDER BY lastmodified DESC",
        );
        assert!(c.filter.contains("c.type IN (?)"));
        assert!(c.order_by.starts_with("c.updated_at DESC"));
        assert_eq!(c.binds.len(), 4);
        assert!(!c.spaces);
    }

    #[test]
    fn finds_space_type() {
        assert!(compile_ok("type = space AND space.title ~ dev").spaces);
        assert!(compile_ok("type in (page, space)").spaces);
    }

    #[test]
    fn rejects_unknown_fields() {
        let ctx = CqlContext {
            users: &[],
            current_user: None,
            now: 0,
        };
        let error = compile("colour = red", &ctx, Target::Content).unwrap_err();
        assert!(error.0.starts_with("Could not parse cql : colour = red"));
    }

    #[test]
    fn spaces_target_maps_fields() {
        let ctx = CqlContext {
            users: &[],
            current_user: None,
            now: 0,
        };
        let c = compile("type = space AND title ~ eng", &ctx, Target::Spaces).unwrap();
        assert!(c.filter.contains("'space' IN (?)"));
        assert!(c.filter.contains("s.name"));
    }

    #[test]
    fn date_functions() {
        compile_ok("created > now(\"-4w\") and lastmodified < startOfDay()");
        compile_ok("created = 2026-10-01");
        compile_ok("space.id = \"98304\" and text ~ \"setup\"");
    }
}
