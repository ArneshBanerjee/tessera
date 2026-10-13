//! Compiles parsed JQL into SQL over the `issues` table, which queries alias as `i`.
//!
//! The output is a `WHERE` condition with `?` placeholders, the values to bind in order, and an
//! `ORDER BY` list. User input only ever reaches SQL through bound values.
//!
//! Semantics follow Jira's documentation:
//! - `!=` and `NOT IN` never match issues where the field is empty.
//! - `resolution = Unresolved` matches issues without a resolution.
//! - `priority > Medium` means more urgent than Medium.
//! - `~` matches when every word appears in the text (case-insensitive); `*` and `?` are wildcards.
//! - `WAS` and `CHANGED` search the issue history.

use tessera_jql::ast::{
    Clause, Direction, Field, Operand, Operator, OrderBy, Predicate, Query, Terminal,
};

use crate::store::fields::{CustomCatalog, CustomField};
use crate::store::meta::Catalog;
use crate::store::projects::Project;
use crate::store::users::User;
use crate::time;

/// What the compiler needs to resolve names and functions.
pub struct SearchContext<'a> {
    pub catalog: &'a Catalog,
    pub custom: &'a CustomCatalog,
    pub projects: &'a [Project],
    pub users: &'a [User],
    pub current_user: Option<i64>,
    pub now: i64,
}

/// A value bound to a `?` placeholder.
#[derive(Debug, Clone, PartialEq)]
pub enum Bind {
    Int(i64),
    Real(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    /// A SQL condition over `issues i`. `1 = 1` when the query has no filter.
    pub filter: String,
    pub binds: Vec<Bind>,
    pub order_by: String,
}

/// A JQL error, with Jira's wording.
#[derive(Debug, Clone, PartialEq)]
pub struct JqlError(pub String);

type Result<T> = std::result::Result<T, JqlError>;

pub fn compile(query: &Query, ctx: &SearchContext) -> Result<Compiled> {
    let mut compiler = Compiler {
        ctx,
        binds: Vec::new(),
    };
    let filter = match &query.filter {
        Some(clause) => compiler.clause(clause)?,
        None => "1 = 1".to_string(),
    };
    let order_by = compiler.order_by(&query.order_by)?;
    Ok(Compiled {
        filter,
        binds: compiler.binds,
        order_by,
    })
}

/// The kind of field a JQL clause names, which decides the SQL it compiles to.
#[derive(Clone, Copy)]
enum Kind<'a> {
    Project,
    IssueType,
    Status,
    StatusCategory,
    Priority,
    Resolution,
    /// A user column. `history` is the field id in the changelog, when history is kept.
    User {
        column: &'static str,
        history: Option<&'static str>,
    },
    Text {
        column: &'static str,
    },
    Comment,
    AllText,
    Labels,
    /// A millisecond timestamp column.
    Date {
        column: &'static str,
    },
    DueDate,
    Key,
    Parent,
    Custom(&'a CustomField),
}

/// A single value after lists are flattened.
#[derive(Clone, Copy)]
enum Atom<'q> {
    Value(&'q str),
    Empty,
    Function(&'q str, &'q [String]),
}

fn atoms(operand: &Operand) -> Vec<Atom<'_>> {
    match operand {
        Operand::Value { text, .. } => vec![Atom::Value(text)],
        Operand::Empty => vec![Atom::Empty],
        Operand::Function { name, args } => vec![Atom::Function(name, args)],
        Operand::List(items) => items.iter().flat_map(atoms).collect(),
    }
}

fn describe(atom: Atom) -> String {
    match atom {
        Atom::Value(text) => text.to_string(),
        Atom::Empty => "EMPTY".into(),
        Atom::Function(name, _) => format!("{name}()"),
    }
}

/// A comma-separated list of `n` placeholders.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

struct Compiler<'a> {
    ctx: &'a SearchContext<'a>,
    binds: Vec<Bind>,
}

impl<'a> Compiler<'a> {
    fn clause(&mut self, clause: &Clause) -> Result<String> {
        Ok(match clause {
            Clause::And(parts) => self.join(parts, " AND ")?,
            Clause::Or(parts) => self.join(parts, " OR ")?,
            Clause::Not(inner) => format!("NOT ({})", self.clause(inner)?),
            Clause::Terminal(terminal) => self.terminal(terminal)?,
        })
    }

    fn join(&mut self, parts: &[Clause], separator: &str) -> Result<String> {
        let compiled: Vec<String> = parts
            .iter()
            .map(|p| self.clause(p).map(|sql| format!("({sql})")))
            .collect::<Result<_>>()?;
        Ok(compiled.join(separator))
    }

    fn resolve(&self, field: &Field) -> Result<(Kind<'a>, String)> {
        let custom = |f: &'a CustomField| (Kind::Custom(f), f.name.clone());
        match field {
            Field::CustomField(id) => self
                .ctx
                .custom
                .fields
                .iter()
                .find(|f| u64::try_from(f.id) == Ok(*id))
                .map(custom)
                .ok_or_else(|| unknown_field(&format!("cf[{id}]"))),
            Field::Name(name) => {
                let kind = match name.to_ascii_lowercase().as_str() {
                    "project" => Kind::Project,
                    "issuetype" | "type" => Kind::IssueType,
                    "status" => Kind::Status,
                    "statuscategory" => Kind::StatusCategory,
                    "priority" => Kind::Priority,
                    "resolution" => Kind::Resolution,
                    "assignee" => Kind::User {
                        column: "assignee_id",
                        history: Some("assignee"),
                    },
                    "reporter" => Kind::User {
                        column: "reporter_id",
                        history: Some("reporter"),
                    },
                    "creator" => Kind::User {
                        column: "creator_id",
                        history: None,
                    },
                    "summary" => Kind::Text { column: "summary" },
                    "description" => Kind::Text {
                        column: "description_text",
                    },
                    "environment" => Kind::Text {
                        column: "environment_text",
                    },
                    "comment" => Kind::Comment,
                    "text" => Kind::AllText,
                    "labels" => Kind::Labels,
                    "created" | "createddate" => Kind::Date {
                        column: "created_at",
                    },
                    "updated" | "updateddate" => Kind::Date {
                        column: "updated_at",
                    },
                    "resolved" | "resolutiondate" => Kind::Date {
                        column: "resolution_date",
                    },
                    "due" | "duedate" => Kind::DueDate,
                    "key" | "issuekey" | "issue" | "id" => Kind::Key,
                    "parent" => Kind::Parent,
                    _ => {
                        return self
                            .ctx
                            .custom
                            .fields
                            .iter()
                            .find(|f| f.name.eq_ignore_ascii_case(name))
                            .map(custom)
                            .ok_or_else(|| unknown_field(name));
                    }
                };
                Ok((kind, name.clone()))
            }
        }
    }

    fn terminal(&mut self, terminal: &Terminal) -> Result<String> {
        let (kind, name) = self.resolve(&terminal.field)?;
        let op = terminal.operator;
        if op.is_history() {
            return self.history(kind, &name, terminal);
        }
        let atoms = terminal.operand.as_ref().map(atoms).unwrap_or_default();
        match kind {
            Kind::Project
            | Kind::IssueType
            | Kind::Status
            | Kind::StatusCategory
            | Kind::Resolution => {
                let (ids, empty) = self.ids(kind, &name, &atoms)?;
                id_condition(&mut self.binds, column(kind), op, &ids, empty, &name)
            }
            Kind::Priority if is_comparison(op) => {
                let (ids, _) = self.ids(kind, &name, &atoms)?;
                let sequence = ids
                    .first()
                    .and_then(|id| self.ctx.catalog.priority(*id))
                    .map(|p| p.sequence)
                    .ok_or_else(|| unsupported(op, &name))?;
                // A more urgent priority has a lower sequence, so the comparison flips.
                let flipped = match op {
                    Operator::GreaterThan => "<",
                    Operator::GreaterThanEquals => "<=",
                    Operator::LessThan => ">",
                    _ => ">=",
                };
                self.binds.push(Bind::Int(sequence));
                Ok(format!(
                    "i.priority_id IN (SELECT id FROM priorities WHERE sequence {flipped} ?)"
                ))
            }
            Kind::Priority | Kind::User { .. } => {
                let (ids, empty) = self.ids(kind, &name, &atoms)?;
                id_condition(&mut self.binds, column(kind), op, &ids, empty, &name)
            }
            Kind::Text { column } => {
                let column = format!("i.{column}");
                self.text(&column, op, &atoms, &name)
            }
            Kind::Comment => self.comment(op, &atoms, &name),
            Kind::AllText => {
                if !matches!(op, Operator::Contains | Operator::NotContains) {
                    return Err(unsupported(op, &name));
                }
                let mut parts = Vec::new();
                for column in ["i.summary", "i.description_text", "i.environment_text"] {
                    parts.push(self.text(column, Operator::Contains, &atoms, &name)?);
                }
                parts.push(self.comment(Operator::Contains, &atoms, &name)?);
                let any = format!("({})", parts.join(" OR "));
                Ok(if op == Operator::NotContains {
                    format!("NOT {any}")
                } else {
                    any
                })
            }
            Kind::Labels => self.labels(op, &atoms, &name),
            Kind::Date { column } => self.date(&format!("i.{column}"), op, &atoms, &name),
            Kind::DueDate => self.due_date(op, &atoms, &name),
            Kind::Key => self.key(op, &atoms, &name),
            Kind::Parent => self.parent(op, &atoms, &name),
            Kind::Custom(field) => self.custom(field, op, &atoms, &name),
        }
    }

    /// Resolves values of an id-like field. Returns the ids and whether EMPTY was included.
    fn ids(&self, kind: Kind, name: &str, atoms: &[Atom]) -> Result<(Vec<i64>, bool)> {
        let catalog = self.ctx.catalog;
        let mut ids = Vec::new();
        let mut empty = false;
        for &atom in atoms {
            let text = match atom {
                Atom::Empty => {
                    empty = true;
                    continue;
                }
                Atom::Function(function, _) if function.eq_ignore_ascii_case("currentUser") => {
                    if !matches!(kind, Kind::User { .. }) {
                        return Err(function_not_supported(function, name));
                    }
                    // An anonymous search matches no user.
                    ids.extend(self.ctx.current_user);
                    if self.ctx.current_user.is_none() {
                        ids.push(-1);
                    }
                    continue;
                }
                Atom::Function(function, _) => return Err(function_not_supported(function, name)),
                Atom::Value(text) => text,
            };
            let numeric: Option<i64> = text.parse().ok();
            let is = |id: i64, item_name: &str| {
                Some(id) == numeric || item_name.eq_ignore_ascii_case(text)
            };
            let found: Vec<i64> = match kind {
                Kind::Project => self
                    .ctx
                    .projects
                    .iter()
                    .filter(|p| is(p.id, &p.key) || p.name.eq_ignore_ascii_case(text))
                    .map(|p| p.id)
                    .collect(),
                Kind::IssueType => catalog
                    .issue_types
                    .iter()
                    .filter(|t| is(t.id, &t.name))
                    .map(|t| t.id)
                    .collect(),
                Kind::Status => catalog
                    .statuses
                    .iter()
                    .filter(|s| is(s.id, &s.name))
                    .map(|s| s.id)
                    .collect(),
                Kind::StatusCategory => {
                    let categories: Vec<i64> = catalog
                        .status_categories
                        .iter()
                        .filter(|c| is(c.id, &c.name) || c.key.eq_ignore_ascii_case(text))
                        .map(|c| c.id)
                        .collect();
                    catalog
                        .statuses
                        .iter()
                        .filter(|s| categories.contains(&s.category_id))
                        .map(|s| s.id)
                        .collect()
                }
                Kind::Priority => catalog
                    .priorities
                    .iter()
                    .filter(|p| is(p.id, &p.name))
                    .map(|p| p.id)
                    .collect(),
                Kind::Resolution if text.eq_ignore_ascii_case("Unresolved") => {
                    empty = true;
                    continue;
                }
                Kind::Resolution => catalog
                    .resolutions
                    .iter()
                    .filter(|r| is(r.id, &r.name))
                    .map(|r| r.id)
                    .collect(),
                Kind::User { .. } => self
                    .ctx
                    .users
                    .iter()
                    .filter(|u| {
                        u.account_id == text
                            || u.email.eq_ignore_ascii_case(text)
                            || u.display_name.eq_ignore_ascii_case(text)
                    })
                    .map(|u| u.id)
                    .collect(),
                _ => unreachable!("only id-like fields resolve ids"),
            };
            if found.is_empty() {
                return Err(JqlError(format!(
                    "The value '{text}' does not exist for the field '{name}'."
                )));
            }
            ids.extend(found);
        }
        Ok((ids, empty))
    }

    fn text(&mut self, column: &str, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        match op {
            Operator::Is | Operator::IsNot => {
                let empty = format!("({column} IS NULL OR {column} = '')");
                Ok(if op == Operator::Is {
                    empty
                } else {
                    format!("NOT {empty}")
                })
            }
            Operator::Contains | Operator::NotContains => {
                let text = single_text(atoms, name)?;
                let condition = self.like_all(column, text);
                Ok(if op == Operator::NotContains {
                    format!("NOT ({condition})")
                } else {
                    condition
                })
            }
            _ => Err(unsupported(op, name)),
        }
    }

    /// Every word of `text` must appear in `column`. A quoted phrase must appear as written.
    fn like_all(&mut self, column: &str, text: &str) -> String {
        let trimmed = text.trim();
        let terms: Vec<String> =
            if trimmed.len() > 1 && trimmed.starts_with('"') && trimmed.ends_with('"') {
                vec![trimmed[1..trimmed.len() - 1].to_string()]
            } else {
                trimmed.split_whitespace().map(str::to_string).collect()
            };
        if terms.is_empty() {
            return "1 = 1".into();
        }
        let conditions: Vec<String> = terms
            .iter()
            .map(|term| {
                self.binds
                    .push(Bind::Text(format!("%{}%", like_pattern(term))));
                format!("{column} LIKE ? ESCAPE '\\'")
            })
            .collect();
        conditions.join(" AND ")
    }

    fn comment(&mut self, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        match op {
            Operator::Contains | Operator::NotContains => {
                let text = single_text(atoms, name)?;
                let condition = self.like_all("c.body_text", text);
                let exists = format!(
                    "EXISTS (SELECT 1 FROM comments c WHERE c.issue_id = i.id AND {condition})"
                );
                Ok(if op == Operator::NotContains {
                    format!("NOT {exists}")
                } else {
                    exists
                })
            }
            Operator::Is | Operator::IsNot => {
                let none = "NOT EXISTS (SELECT 1 FROM comments c WHERE c.issue_id = i.id)";
                Ok(if op == Operator::Is {
                    none.into()
                } else {
                    format!("NOT {none}")
                })
            }
            _ => Err(unsupported(op, name)),
        }
    }

    fn labels(&mut self, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        let any = "EXISTS (SELECT 1 FROM issue_labels l WHERE l.issue_id = i.id)";
        let mut values = Vec::new();
        let mut empty = false;
        for &atom in atoms {
            match atom {
                Atom::Value(text) => values.push(text.to_string()),
                Atom::Empty => empty = true,
                Atom::Function(function, _) => return Err(function_not_supported(function, name)),
            }
        }
        let has = |binds: &mut Vec<Bind>, values: &[String]| {
            binds.extend(values.iter().cloned().map(Bind::Text));
            format!(
                "EXISTS (SELECT 1 FROM issue_labels l WHERE l.issue_id = i.id AND l.label IN ({}))",
                placeholders(values.len())
            )
        };
        Ok(match op {
            Operator::Is => format!("NOT {any}"),
            Operator::IsNot => any.into(),
            Operator::Equals | Operator::In => {
                let mut parts = Vec::new();
                if !values.is_empty() {
                    parts.push(has(&mut self.binds, &values));
                }
                if empty {
                    parts.push(format!("NOT {any}"));
                }
                format!("({})", parts.join(" OR "))
            }
            Operator::NotEquals | Operator::NotIn => {
                if values.is_empty() {
                    any.into()
                } else {
                    format!("({any} AND NOT {})", has(&mut self.binds, &values))
                }
            }
            _ => return Err(unsupported(op, name)),
        })
    }

    /// A point in time, in milliseconds, from a date value or function.
    fn time(&self, atom: Atom, name: &str) -> Result<i64> {
        let now = self.ctx.now;
        let invalid = |value: &str| {
            JqlError(format!(
                "Date value '{value}' for field '{name}' is invalid. Valid formats include: 'yyyy/MM/dd HH:mm', \
                 'yyyy-MM-dd HH:mm', 'yyyy/MM/dd', 'yyyy-MM-dd', or a period format e.g. '-5d', '4w 2d'."
            ))
        };
        match atom {
            Atom::Value(text) => time::parse_jql_datetime(text)
                .or_else(|| time::parse_relative(text, now))
                .ok_or_else(|| invalid(text)),
            Atom::Function(function, args) => {
                let lower = function.to_ascii_lowercase();
                if lower == "now" || lower == "currentdate" {
                    return Ok(now);
                }
                let (start, unit) = if let Some(unit) = lower.strip_prefix("startof") {
                    (true, unit)
                } else if let Some(unit) = lower.strip_prefix("endof") {
                    (false, unit)
                } else {
                    return Err(function_not_supported(function, name));
                };
                let boundary = |offset: i64| {
                    if start {
                        time::start_of(unit, now, offset)
                    } else {
                        time::end_of(unit, now, offset)
                    }
                };
                match args.first().map(|a| a.trim()) {
                    None => boundary(0),
                    // A plain number shifts by whole units: startOfWeek(-1) is last week.
                    Some(arg)
                        if arg
                            .trim_start_matches(['+', '-'])
                            .chars()
                            .all(|c| c.is_ascii_digit()) =>
                    {
                        boundary(arg.trim_start_matches('+').parse().ok().unwrap_or(0))
                    }
                    // A period shifts by that amount: startOfMonth("+15d").
                    Some(arg) => boundary(0).and_then(|base| time::parse_relative(arg, base)),
                }
                .ok_or_else(|| function_not_supported(function, name))
            }
            Atom::Empty => Err(invalid("EMPTY")),
        }
    }

    fn date(&mut self, column: &str, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        if matches!(op, Operator::Is | Operator::IsNot) {
            return Ok(format!(
                "{column} IS {}NULL",
                if op == Operator::IsNot { "NOT " } else { "" }
            ));
        }
        let times: Vec<i64> = atoms
            .iter()
            .filter(|a| !matches!(a, Atom::Empty))
            .map(|&a| self.time(a, name))
            .collect::<Result<_>>()?;
        self.compare(column, op, times.into_iter().map(Bind::Int).collect(), name)
    }

    fn due_date(&mut self, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        if matches!(op, Operator::Is | Operator::IsNot) {
            return Ok(format!(
                "i.due_date IS {}NULL",
                if op == Operator::IsNot { "NOT " } else { "" }
            ));
        }
        let dates: Vec<Bind> = atoms
            .iter()
            .filter(|a| !matches!(a, Atom::Empty))
            .map(|&a| {
                self.time(a, name)
                    .map(|ms| Bind::Text(time::format(ms)[..10].to_string()))
            })
            .collect::<Result<_>>()?;
        self.compare("i.due_date", op, dates, name)
    }

    /// `=`, `!=`, comparisons, `IN`, and `NOT IN` against bound values.
    fn compare(
        &mut self,
        expr: &str,
        op: Operator,
        values: Vec<Bind>,
        name: &str,
    ) -> Result<String> {
        let first = || values.first().cloned().ok_or_else(|| unsupported(op, name));
        let sql = match op {
            Operator::Equals
            | Operator::GreaterThan
            | Operator::GreaterThanEquals
            | Operator::LessThan
            | Operator::LessThanEquals => {
                self.binds.push(first()?);
                format!("{expr} {op} ?")
            }
            Operator::NotEquals => {
                self.binds.push(first()?);
                format!("({expr} IS NOT NULL AND {expr} != ?)")
            }
            Operator::In | Operator::NotIn => {
                let n = values.len();
                self.binds.extend(values);
                let not = if op == Operator::NotIn { "NOT " } else { "" };
                format!(
                    "({expr} IS NOT NULL AND {expr} {not}IN ({}))",
                    placeholders(n)
                )
            }
            _ => return Err(unsupported(op, name)),
        };
        Ok(sql)
    }

    fn key(&mut self, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        let values: Vec<&str> = atoms
            .iter()
            .map(|&a| match a {
                Atom::Value(text) => Ok(text),
                other => Err(JqlError(format!(
                    "The value '{}' is not valid for the field '{name}'.",
                    describe(other)
                ))),
            })
            .collect::<Result<_>>()?;
        if is_comparison(op) {
            let key = *values.first().ok_or_else(|| unsupported(op, name))?;
            let (project, number) = split_key(key).ok_or_else(|| invalid_key(key, name))?;
            self.binds.push(Bind::Text(project.to_string()));
            self.binds.push(Bind::Int(number));
            return Ok(format!(
                "(i.project_id = (SELECT id FROM projects WHERE key = ? COLLATE NOCASE) AND i.number {op} ?)"
            ));
        }
        let mut matches = Vec::new();
        for value in &values {
            if let Ok(id) = value.parse::<i64>() {
                self.binds.push(Bind::Int(id));
                matches.push("i.id = ?".to_string());
            } else if split_key(value).is_some() {
                self.binds.push(Bind::Text(value.to_string()));
                matches.push("i.key = ? COLLATE NOCASE".to_string());
            } else {
                return Err(invalid_key(value, name));
            }
        }
        let any = format!("({})", matches.join(" OR "));
        match op {
            Operator::Equals | Operator::In => Ok(any),
            Operator::NotEquals | Operator::NotIn => Ok(format!("NOT {any}")),
            _ => Err(unsupported(op, name)),
        }
    }

    fn parent(&mut self, op: Operator, atoms: &[Atom], name: &str) -> Result<String> {
        if matches!(op, Operator::Is | Operator::IsNot) {
            return Ok(format!(
                "i.parent_id IS {}NULL",
                if op == Operator::IsNot { "NOT " } else { "" }
            ));
        }
        let mut matches = Vec::new();
        for &atom in atoms {
            let Atom::Value(value) = atom else {
                return Err(unsupported(op, name));
            };
            match value.parse::<i64>() {
                Ok(id) => self.binds.push(Bind::Int(id)),
                Err(_) => self.binds.push(Bind::Text(value.to_string())),
            }
            matches.push(if value.parse::<i64>().is_ok() {
                "p.id = ?".to_string()
            } else {
                "p.key = ? COLLATE NOCASE".to_string()
            });
        }
        let any = format!(
            "i.parent_id IN (SELECT p.id FROM issues p WHERE {})",
            matches.join(" OR ")
        );
        match op {
            Operator::Equals | Operator::In => Ok(any),
            Operator::NotEquals | Operator::NotIn => {
                Ok(format!("(i.parent_id IS NOT NULL AND NOT {any})"))
            }
            _ => Err(unsupported(op, name)),
        }
    }

    fn custom(
        &mut self,
        field: &CustomField,
        op: Operator,
        atoms: &[Atom],
        name: &str,
    ) -> Result<String> {
        let row = format!(
            "SELECT 1 FROM issue_field_values v WHERE v.issue_id = i.id AND v.field_id = {}",
            field.id
        );
        if matches!(op, Operator::Is | Operator::IsNot) {
            let not = if op == Operator::Is { "NOT " } else { "" };
            return Ok(format!("{not}EXISTS ({row})"));
        }
        let values: Vec<&str> = atoms
            .iter()
            .filter_map(|a| match a {
                Atom::Value(text) => Some(*text),
                _ => None,
            })
            .collect();
        let with = |condition: String| format!("EXISTS ({row} AND {condition})");
        let negate = matches!(
            op,
            Operator::NotEquals | Operator::NotIn | Operator::NotContains
        );
        let condition = match field.field_type.as_str() {
            "string" | "url" | "text"
                if matches!(op, Operator::Contains | Operator::NotContains) =>
            {
                let text = single_text(atoms, name)?;
                self.like_all("v.value", text)
            }
            "string" | "url"
                if matches!(
                    op,
                    Operator::Equals | Operator::NotEquals | Operator::In | Operator::NotIn
                ) =>
            {
                self.binds
                    .extend(values.iter().map(|v| Bind::Text(v.to_string())));
                format!(
                    "json_extract(v.value, '$') IN ({})",
                    placeholders(values.len())
                )
            }
            "number" => {
                let numbers: Vec<Bind> = values
                    .iter()
                    .map(|v| v.parse::<f64>().map(Bind::Real))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|_| {
                        JqlError(format!(
                            "The value '{}' is not a number for the field '{name}'.",
                            values.join(", ")
                        ))
                    })?;
                let positive = if negate { positive_of(op) } else { op };
                return self
                    .compare(
                        "CAST(json_extract(v.value, '$') AS REAL)",
                        positive,
                        numbers,
                        name,
                    )
                    .map(|condition| {
                        let exists = with(condition);
                        if negate {
                            format!("(EXISTS ({row}) AND NOT {exists})")
                        } else {
                            exists
                        }
                    });
            }
            "date" | "datetime" => {
                let times: Vec<i64> = atoms
                    .iter()
                    .filter(|a| !matches!(a, Atom::Empty))
                    .map(|&a| self.time(a, name))
                    .collect::<Result<_>>()?;
                let length = if field.field_type == "date" { 10 } else { 28 };
                let texts = times
                    .into_iter()
                    .map(|ms| Bind::Text(time::format(ms)[..length].to_string()))
                    .collect();
                let positive = if negate { positive_of(op) } else { op };
                return self
                    .compare("json_extract(v.value, '$')", positive, texts, name)
                    .map(|condition| {
                        let exists = with(condition);
                        if negate {
                            format!("(EXISTS ({row}) AND NOT {exists})")
                        } else {
                            exists
                        }
                    });
            }
            "user" if !is_comparison(op) && op != Operator::Contains => {
                let (ids, _) = self.ids(
                    Kind::User {
                        column: "",
                        history: None,
                    },
                    name,
                    atoms,
                )?;
                let account_ids: Vec<Bind> = ids
                    .iter()
                    .filter_map(|id| self.ctx.users.iter().find(|u| u.id == *id))
                    .map(|u| Bind::Text(u.account_id.clone()))
                    .collect();
                if account_ids.is_empty() {
                    // Only EMPTY was given, which `IS EMPTY` handles; nothing else can match.
                    "0".to_string()
                } else {
                    let list = placeholders(account_ids.len());
                    self.binds.extend(account_ids);
                    format!("json_extract(v.value, '$.accountId') IN ({list})")
                }
            }
            "option" | "options" if !is_comparison(op) && op != Operator::Contains => {
                let mut option_ids = Vec::new();
                for value in &values {
                    let found: Vec<String> = self
                        .ctx
                        .custom
                        .options
                        .iter()
                        .filter(|o| {
                            o.field_id == field.id
                                && (o.value.eq_ignore_ascii_case(value)
                                    || o.id.to_string() == *value)
                        })
                        .map(|o| o.id.to_string())
                        .collect();
                    if found.is_empty() {
                        return Err(JqlError(format!(
                            "The option '{value}' for field '{name}' does not exist."
                        )));
                    }
                    option_ids.extend(found);
                }
                let list = placeholders(option_ids.len());
                self.binds.extend(option_ids.into_iter().map(Bind::Text));
                if field.field_type == "option" {
                    format!("json_extract(v.value, '$.id') IN ({list})")
                } else {
                    format!(
                        "EXISTS (SELECT 1 FROM json_each(v.value) e WHERE json_extract(e.value, '$.id') IN ({list}))"
                    )
                }
            }
            "labels" if !is_comparison(op) && op != Operator::Contains => {
                self.binds
                    .extend(values.iter().map(|v| Bind::Text(v.to_string())));
                format!(
                    "EXISTS (SELECT 1 FROM json_each(v.value) e WHERE e.value IN ({}))",
                    placeholders(values.len())
                )
            }
            _ => return Err(unsupported(op, name)),
        };
        let exists = with(condition);
        Ok(if negate {
            format!("(EXISTS ({row}) AND NOT {exists})")
        } else {
            exists
        })
    }

    /// `WAS`, `WAS IN`, `WAS NOT`, `WAS NOT IN`, and `CHANGED`, over the issue history.
    fn history(&mut self, kind: Kind, name: &str, terminal: &Terminal) -> Result<String> {
        let op = terminal.operator;
        let field_id = match kind {
            Kind::Status => "status",
            Kind::Priority => "priority",
            Kind::Resolution => "resolution",
            Kind::User {
                history: Some(field),
                ..
            } => field,
            _ => {
                return Err(JqlError(format!(
                    "History searches do not support the '{name}' field."
                )));
            }
        };
        // Values in the changelog are ids as text; for users they are account ids.
        let history_values = |compiler: &Self, atoms: &[Atom]| -> Result<(Vec<String>, bool)> {
            let (ids, empty) = compiler.ids(kind, name, atoms)?;
            let values = ids
                .iter()
                .map(|id| match kind {
                    Kind::User { .. } => compiler
                        .ctx
                        .users
                        .iter()
                        .find(|u| u.id == *id)
                        .map(|u| u.account_id.clone())
                        .unwrap_or_default(),
                    _ => id.to_string(),
                })
                .collect();
            Ok((values, empty))
        };

        let mut conditions = vec!["ci.field_id = ?".to_string()];
        let mut binds = vec![Bind::Text(field_id.into())];
        let in_list = |column: &str, values: &[String], empty: bool, binds: &mut Vec<Bind>| {
            let mut parts = Vec::new();
            if !values.is_empty() {
                binds.extend(values.iter().cloned().map(Bind::Text));
                parts.push(format!("ci.{column} IN ({})", placeholders(values.len())));
            }
            if empty {
                parts.push(format!("ci.{column} IS NULL"));
            }
            format!("({})", parts.join(" OR "))
        };

        let mut current = None;
        if op != Operator::Changed {
            let atoms = terminal.operand.as_ref().map(atoms).unwrap_or_default();
            let (values, empty) = history_values(self, &atoms)?;
            let mut from_binds = Vec::new();
            let from = in_list("from_value", &values, empty, &mut from_binds);
            let to = in_list("to_value", &values, empty, &mut binds);
            binds.extend(from_binds);
            conditions.push(format!("({to} OR {from})"));
            // Without predicates, the current value also counts ("was" includes "is").
            if terminal.predicates.is_empty() {
                let (ids, empty) = self.ids(kind, name, &atoms)?;
                let mut parts = Vec::new();
                let col = column(kind);
                if !ids.is_empty() {
                    parts.push(format!(
                        "{col} IN ({})",
                        ids.iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if empty {
                    parts.push(format!("{col} IS NULL"));
                }
                current = Some(format!("({})", parts.join(" OR ")));
            }
        }
        for predicate in &terminal.predicates {
            match predicate {
                Predicate::From(value) | Predicate::To(value) => {
                    if op != Operator::Changed {
                        return Err(JqlError(
                            "FROM and TO can only be used with CHANGED.".into(),
                        ));
                    }
                    let column = if matches!(predicate, Predicate::From(_)) {
                        "from_value"
                    } else {
                        "to_value"
                    };
                    let (values, empty) = history_values(self, &atoms(value))?;
                    conditions.push(in_list(column, &values, empty, &mut binds));
                }
                Predicate::By(value) => {
                    let (ids, _) = self.ids(
                        Kind::User {
                            column: "",
                            history: None,
                        },
                        "by",
                        &atoms(value),
                    )?;
                    binds.extend(ids.iter().map(|id| Bind::Int(*id)));
                    conditions.push(format!("c.author_id IN ({})", placeholders(ids.len())));
                }
                Predicate::After(value) | Predicate::Before(value) => {
                    let at = self.time(single_atom(value)?, name)?;
                    let sign = if matches!(predicate, Predicate::After(_)) {
                        ">"
                    } else {
                        "<"
                    };
                    binds.push(Bind::Int(at));
                    conditions.push(format!("c.created_at {sign} ?"));
                }
                Predicate::On(value) => {
                    let at = self.time(single_atom(value)?, name)?;
                    let start = time::start_of("day", at, 0).unwrap_or(at);
                    binds.push(Bind::Int(start));
                    binds.push(Bind::Int(start + 86_400_000));
                    conditions.push("c.created_at >= ? AND c.created_at < ?".into());
                }
                Predicate::During(from, to) => {
                    binds.push(Bind::Int(self.time(single_atom(from)?, name)?));
                    binds.push(Bind::Int(self.time(single_atom(to)?, name)?));
                    conditions.push("c.created_at BETWEEN ? AND ?".into());
                }
            }
        }
        self.binds.extend(binds);
        let exists = format!(
            "EXISTS (SELECT 1 FROM changelog_items ci JOIN changelogs c ON c.id = ci.changelog_id \
             WHERE c.issue_id = i.id AND {})",
            conditions.join(" AND ")
        );
        let positive = match current {
            Some(current) => format!("({exists} OR {current})"),
            None => exists,
        };
        Ok(match op {
            Operator::WasNot | Operator::WasNotIn => format!("NOT {positive}"),
            _ => positive,
        })
    }

    fn order_by(&mut self, order: &[OrderBy]) -> Result<String> {
        if order.is_empty() {
            return Ok("i.created_at DESC, i.id DESC".into());
        }
        let mut parts = Vec::new();
        for item in order {
            let (kind, name) = self.resolve(&item.field)?;
            let direction = match item.direction {
                Some(Direction::Desc) => "DESC",
                _ => "ASC",
            };
            let expr = match kind {
                // Keys sort by project key, then by number: API-2 < API-10 < WEB-1.
                Kind::Key => {
                    parts.push(format!(
                        "(SELECT key FROM projects WHERE id = i.project_id) {direction}"
                    ));
                    "i.number".to_string()
                }
                Kind::Project => "(SELECT key FROM projects WHERE id = i.project_id)".into(),
                Kind::IssueType => "i.issue_type_id".into(),
                Kind::Status | Kind::StatusCategory => "i.status_id".into(),
                // Ascending priority means least urgent first, as in Jira.
                Kind::Priority => {
                    "(SELECT -sequence FROM priorities WHERE id = i.priority_id)".into()
                }
                Kind::Resolution => "i.resolution_id".into(),
                Kind::User { column, .. } => {
                    format!("(SELECT display_name FROM users WHERE id = i.{column}) COLLATE NOCASE")
                }
                Kind::Text { column: "summary" } => "i.summary COLLATE NOCASE".into(),
                Kind::Date { column } => format!("i.{column}"),
                Kind::DueDate => "i.due_date".into(),
                Kind::Parent => "i.parent_id".into(),
                Kind::Custom(field) => {
                    let value = format!(
                        "(SELECT {} FROM issue_field_values v WHERE v.issue_id = i.id AND v.field_id = {})",
                        if field.field_type == "number" {
                            "CAST(json_extract(v.value, '$') AS REAL)"
                        } else {
                            "json_extract(v.value, '$')"
                        },
                        field.id
                    );
                    value
                }
                _ => {
                    return Err(JqlError(format!(
                        "Field '{name}' does not support sorting."
                    )));
                }
            };
            parts.push(format!("{expr} {direction} NULLS LAST"));
        }
        parts.push("i.id DESC".into());
        Ok(parts.join(", "))
    }
}

/// The SQL column of an id-like field.
fn column(kind: Kind) -> String {
    match kind {
        Kind::Project => "i.project_id".into(),
        Kind::IssueType => "i.issue_type_id".into(),
        Kind::Status | Kind::StatusCategory => "i.status_id".into(),
        Kind::Priority => "i.priority_id".into(),
        Kind::Resolution => "i.resolution_id".into(),
        Kind::User { column, .. } => format!("i.{column}"),
        _ => unreachable!("only id-like fields have an id column"),
    }
}

/// `=`, `!=`, `IN`, `NOT IN`, `IS`, and `IS NOT` for an id column.
fn id_condition(
    binds: &mut Vec<Bind>,
    col: String,
    op: Operator,
    ids: &[i64],
    empty: bool,
    name: &str,
) -> Result<String> {
    let in_ids = |binds: &mut Vec<Bind>| {
        binds.extend(ids.iter().map(|id| Bind::Int(*id)));
        format!("{col} IN ({})", placeholders(ids.len()))
    };
    let positive = |binds: &mut Vec<Bind>| {
        let mut parts = Vec::new();
        if !ids.is_empty() {
            parts.push(in_ids(binds));
        }
        if empty {
            parts.push(format!("{col} IS NULL"));
        }
        format!("({})", parts.join(" OR "))
    };
    Ok(match op {
        Operator::Equals | Operator::In => positive(binds),
        Operator::NotEquals | Operator::NotIn => {
            // Jira's negative operators never match an empty field.
            if ids.is_empty() {
                format!("{col} IS NOT NULL")
            } else {
                format!("({col} IS NOT NULL AND NOT {})", in_ids(binds))
            }
        }
        Operator::Is => format!("{col} IS NULL"),
        Operator::IsNot => format!("{col} IS NOT NULL"),
        _ => return Err(unsupported(op, name)),
    })
}

fn is_comparison(op: Operator) -> bool {
    matches!(
        op,
        Operator::GreaterThan
            | Operator::GreaterThanEquals
            | Operator::LessThan
            | Operator::LessThanEquals
    )
}

/// The positive form of a negative operator: `!=` becomes `=`, `NOT IN` becomes `IN`.
fn positive_of(op: Operator) -> Operator {
    match op {
        Operator::NotEquals => Operator::Equals,
        Operator::NotIn => Operator::In,
        Operator::NotContains => Operator::Contains,
        other => other,
    }
}

fn single_atom(operand: &Operand) -> Result<Atom<'_>> {
    atoms(operand)
        .into_iter()
        .next()
        .ok_or_else(|| JqlError("Expected a single value.".into()))
}

fn single_text<'q>(atoms: &[Atom<'q>], name: &str) -> Result<&'q str> {
    match atoms {
        [Atom::Value(text)] => Ok(text),
        _ => Err(JqlError(format!(
            "The field '{name}' requires a single text value for this operator."
        ))),
    }
}

/// Converts JQL wildcards to a LIKE pattern and escapes LIKE's own special characters.
fn like_pattern(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for c in term.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Splits `PROJ-12` into `("PROJ", 12)`.
fn split_key(key: &str) -> Option<(&str, i64)> {
    let (project, number) = key.rsplit_once('-')?;
    Some((project, number.parse().ok()?))
}

fn unknown_field(name: &str) -> JqlError {
    JqlError(format!(
        "Field '{name}' does not exist or you do not have permission to view it."
    ))
}

fn unsupported(op: Operator, name: &str) -> JqlError {
    JqlError(format!(
        "The operator '{op}' is not supported by the '{name}' field."
    ))
}

fn function_not_supported(function: &str, name: &str) -> JqlError {
    JqlError(format!(
        "The function '{function}()' is not supported for the field '{name}'."
    ))
}

fn invalid_key(key: &str, name: &str) -> JqlError {
    JqlError(format!(
        "The issue key '{key}' for field '{name}' is invalid."
    ))
}
