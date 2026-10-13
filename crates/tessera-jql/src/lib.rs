//! A parser for the Jira Query Language (JQL).
//!
//! Built clean-room from Atlassian's public JQL reference: fields, operators, keywords,
//! functions, history predicates (`WAS`, `CHANGED`), and `ORDER BY`.
//!
//! ```
//! let query = tessera_jql::parse(r#"project = PROJ AND status WAS "Done" BY jsmith ORDER BY created DESC"#).unwrap();
//! assert!(query.filter.is_some());
//! assert_eq!(query.order_by.len(), 1);
//! ```

pub mod ast;
mod lexer;
mod parser;

pub use parser::{ParseError, parse};
