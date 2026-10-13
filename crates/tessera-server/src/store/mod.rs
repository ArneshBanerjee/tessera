//! Data access. Each module owns the SQL for one area; the API layer never writes SQL itself,
//! except the JQL search compiler, which builds queries from user input.

pub mod attachments;
pub mod boards;
pub mod changelog;
pub mod comments;
pub mod components;
pub mod epics;
pub mod fields;
pub mod filters;
pub mod issues;
pub mod links;
pub mod meta;
pub mod projects;
pub mod properties;
pub mod ranks;
pub mod social;
pub mod sprints;
pub mod users;
pub mod versions;
pub mod workflows;
pub mod worklogs;
