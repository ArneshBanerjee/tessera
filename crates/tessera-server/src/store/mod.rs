//! Data access. Each module owns the SQL for one area; the API layer never writes SQL itself,
//! except the JQL search compiler, which builds queries from user input.

pub mod changelog;
pub mod comments;
pub mod fields;
pub mod issues;
pub mod meta;
pub mod projects;
pub mod users;
pub mod workflows;
