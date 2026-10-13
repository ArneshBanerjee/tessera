//! Recursive-descent parser. Precedence, from loosest to tightest: `OR`, `AND`, `NOT`.

use std::ops::Range;

use logos::Logos;

use crate::ast::*;
use crate::lexer::Token;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} (at character {offset})")]
pub struct ParseError {
    pub message: String,
    /// Byte offset into the query where the problem starts.
    pub offset: usize,
}

pub fn parse(input: &str) -> Result<Query, ParseError> {
    let mut tokens = Vec::new();
    for (token, span) in Token::lexer(input).spanned() {
        match token {
            Ok(token) => tokens.push((token, span)),
            Err(()) => {
                return Err(ParseError {
                    message: format!("Unexpected character '{}'", &input[span.clone()]),
                    offset: span.start,
                });
            }
        }
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        end: input.len(),
    };
    let query = parser.query()?;
    match parser.peek() {
        None => Ok(query),
        Some(token) => Err(parser.error(format!(
            "Unexpected {} after the end of the query",
            describe(token)
        ))),
    }
}

struct Parser {
    tokens: Vec<(Token, Range<usize>)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn query(&mut self) -> Result<Query, ParseError> {
        let filter = if self.peek().is_none() || self.at_order_by() {
            None
        } else {
            Some(self.or()?)
        };
        let mut order_by = Vec::new();
        if self.at_order_by() {
            self.pos += 2;
            loop {
                let field = self.field()?;
                let direction = if self.eat_keyword("asc") {
                    Some(Direction::Asc)
                } else if self.eat_keyword("desc") {
                    Some(Direction::Desc)
                } else {
                    None
                };
                order_by.push(OrderBy { field, direction });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        Ok(Query { filter, order_by })
    }

    fn or(&mut self) -> Result<Clause, ParseError> {
        let mut clauses = vec![self.and()?];
        while self.eat_keyword("or") {
            clauses.push(self.and()?);
        }
        Ok(if clauses.len() == 1 {
            clauses.pop().unwrap()
        } else {
            Clause::Or(clauses)
        })
    }

    fn and(&mut self) -> Result<Clause, ParseError> {
        let mut clauses = vec![self.not()?];
        while self.eat_keyword("and") {
            clauses.push(self.not()?);
        }
        Ok(if clauses.len() == 1 {
            clauses.pop().unwrap()
        } else {
            Clause::And(clauses)
        })
    }

    fn not(&mut self) -> Result<Clause, ParseError> {
        if self.eat_keyword("not") {
            return Ok(Clause::Not(Box::new(self.not()?)));
        }
        if self.eat(&Token::LParen) {
            let clause = self.or()?;
            self.expect(&Token::RParen, "')' to close the group")?;
            return Ok(clause);
        }
        self.terminal().map(Clause::Terminal)
    }

    fn terminal(&mut self) -> Result<Terminal, ParseError> {
        let field = self.field()?;
        let operator = self.operator()?;
        let operand = if operator == Operator::Changed {
            None
        } else {
            let start = self.offset();
            let operand = self.operand()?;
            if !operator.accepts(&operand) {
                return Err(ParseError {
                    message: format!(
                        "The operator {operator:?} does not accept this kind of value"
                    ),
                    offset: start,
                });
            }
            Some(operand)
        };
        let mut predicates = Vec::new();
        if operator.is_history() {
            while let Some(predicate) = self.predicate()? {
                predicates.push(predicate);
            }
        }
        Ok(Terminal {
            field,
            operator,
            operand,
            predicates,
        })
    }

    fn field(&mut self) -> Result<Field, ParseError> {
        match self.next() {
            Some(Token::Word(word))
                if word.eq_ignore_ascii_case("cf") && self.peek() == Some(&Token::LBracket) =>
            {
                self.pos += 1;
                let id = match self.next() {
                    Some(Token::Word(id)) => id.parse().ok(),
                    _ => None,
                };
                let id = id.ok_or_else(|| {
                    self.error_at_previous(
                        "Expected a numeric custom field id inside cf[...]".into(),
                    )
                })?;
                self.expect(&Token::RBracket, "']' to close cf[...]")?;
                Ok(Field::CustomField(id))
            }
            Some(Token::Word(word)) | Some(Token::Str(word)) => Ok(Field::Name(word)),
            other => {
                let found = other
                    .as_ref()
                    .map_or("the end of the query".into(), describe);
                Err(self.error_at_previous(format!("Expected a field name but found {found}")))
            }
        }
    }

    fn operator(&mut self) -> Result<Operator, ParseError> {
        let operator = match self.next() {
            Some(Token::Eq) => Operator::Equals,
            Some(Token::NotEq) => Operator::NotEquals,
            Some(Token::Gt) => Operator::GreaterThan,
            Some(Token::Gte) => Operator::GreaterThanEquals,
            Some(Token::Lt) => Operator::LessThan,
            Some(Token::Lte) => Operator::LessThanEquals,
            Some(Token::Contains) => Operator::Contains,
            Some(Token::NotContains) => Operator::NotContains,
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("in") => Operator::In,
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("changed") => Operator::Changed,
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("not") => {
                if !self.eat_keyword("in") {
                    return Err(self.error("Expected IN after NOT".into()));
                }
                Operator::NotIn
            }
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("is") => {
                if self.eat_keyword("not") {
                    Operator::IsNot
                } else {
                    Operator::Is
                }
            }
            Some(Token::Word(w)) if w.eq_ignore_ascii_case("was") => {
                let not = self.eat_keyword("not");
                match (not, self.eat_keyword("in")) {
                    (false, false) => Operator::Was,
                    (false, true) => Operator::WasIn,
                    (true, false) => Operator::WasNot,
                    (true, true) => Operator::WasNotIn,
                }
            }
            other => {
                let found = other
                    .as_ref()
                    .map_or("the end of the query".into(), describe);
                return Err(
                    self.error_at_previous(format!("Expected an operator but found {found}"))
                );
            }
        };
        Ok(operator)
    }

    fn operand(&mut self) -> Result<Operand, ParseError> {
        match self.next() {
            Some(Token::LParen) => {
                let mut items = Vec::new();
                if !self.eat(&Token::RParen) {
                    loop {
                        items.push(self.operand()?);
                        if self.eat(&Token::RParen) {
                            break;
                        }
                        self.expect(&Token::Comma, "',' or ')' in the list")?;
                    }
                }
                Ok(Operand::List(items))
            }
            Some(Token::Str(text)) => Ok(Operand::Value { text, quoted: true }),
            Some(Token::Word(word)) if self.peek() == Some(&Token::LParen) => {
                self.pos += 1;
                let mut args = Vec::new();
                if !self.eat(&Token::RParen) {
                    loop {
                        match self.next() {
                            Some(Token::Word(arg)) | Some(Token::Str(arg)) => args.push(arg),
                            _ => {
                                return Err(self.error_at_previous(format!(
                                    "Expected an argument for {word}()"
                                )));
                            }
                        }
                        if self.eat(&Token::RParen) {
                            break;
                        }
                        self.expect(&Token::Comma, "',' or ')' in the function arguments")?;
                    }
                }
                Ok(Operand::Function { name: word, args })
            }
            Some(Token::Word(word))
                if word.eq_ignore_ascii_case("empty") || word.eq_ignore_ascii_case("null") =>
            {
                Ok(Operand::Empty)
            }
            Some(Token::Word(text)) => Ok(Operand::Value {
                text,
                quoted: false,
            }),
            other => {
                let found = other
                    .as_ref()
                    .map_or("the end of the query".into(), describe);
                Err(self.error_at_previous(format!("Expected a value but found {found}")))
            }
        }
    }

    fn predicate(&mut self) -> Result<Option<Predicate>, ParseError> {
        let Some(Token::Word(word)) = self.peek() else {
            return Ok(None);
        };
        let word = word.to_ascii_lowercase();
        let make: fn(Operand) -> Predicate = match word.as_str() {
            "after" => Predicate::After,
            "before" => Predicate::Before,
            "by" => Predicate::By,
            "on" => Predicate::On,
            "from" => Predicate::From,
            "to" => Predicate::To,
            "during" => {
                self.pos += 1;
                self.expect(&Token::LParen, "'(' after DURING")?;
                let start = self.operand()?;
                self.expect(&Token::Comma, "',' between the two DURING dates")?;
                let end = self.operand()?;
                self.expect(&Token::RParen, "')' to close DURING")?;
                return Ok(Some(Predicate::During(start, end)));
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(make(self.operand()?)))
    }

    fn at_order_by(&self) -> bool {
        matches!(
            (self.tokens.get(self.pos), self.tokens.get(self.pos + 1)),
            (Some((Token::Word(a), _)), Some((Token::Word(b), _)))
                if a.eq_ignore_ascii_case("order") && b.eq_ignore_ascii_case("by")
        )
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).map(|(t, _)| t.clone());
        self.pos += 1;
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        let matched = self.peek() == Some(token);
        if matched {
            self.pos += 1;
        }
        matched
    }

    fn eat_keyword(&mut self, keyword: &str) -> bool {
        let matched =
            matches!(self.peek(), Some(Token::Word(w)) if w.eq_ignore_ascii_case(keyword));
        if matched {
            self.pos += 1;
        }
        matched
    }

    fn expect(&mut self, token: &Token, what: &str) -> Result<(), ParseError> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.error(format!("Expected {what}")))
        }
    }

    fn offset(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map_or(self.end, |(_, span)| span.start)
    }

    fn error(&self, message: String) -> ParseError {
        ParseError {
            message,
            offset: self.offset(),
        }
    }

    fn error_at_previous(&self, message: String) -> ParseError {
        let offset = self
            .tokens
            .get(self.pos.saturating_sub(1))
            .map_or(self.end, |(_, span)| span.start);
        ParseError { message, offset }
    }
}

fn describe(token: &Token) -> String {
    match token {
        Token::Word(w) => format!("'{w}'"),
        Token::Str(s) => format!("\"{s}\""),
        other => format!("{other:?}"),
    }
}
