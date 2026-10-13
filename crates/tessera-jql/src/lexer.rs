//! Tokens. Keywords such as `AND` or `ORDER` are lexed as words and recognized by the parser,
//! because JQL keywords are case-insensitive and many of them are also valid values.

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
pub enum Token {
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token("=")]
    Eq,
    #[token("!=")]
    NotEq,
    #[token(">")]
    Gt,
    #[token(">=")]
    Gte,
    #[token("<")]
    Lt,
    #[token("<=")]
    Lte,
    #[token("~")]
    Contains,
    #[token("!~")]
    NotContains,
    #[regex(r#""([^"\\]|\\.)*""#, |lex| unescape(lex.slice()))]
    #[regex(r"'([^'\\]|\\.)*'", |lex| unescape(lex.slice()))]
    Str(String),
    /// Unquoted text: field names, keywords, and values such as `PROJ-12` or `-7d`.
    #[regex(r#"[^ \t\r\n()\[\],=!<>~"']+"#, |lex| lex.slice().to_string())]
    Word(String),
}

fn unescape(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}
