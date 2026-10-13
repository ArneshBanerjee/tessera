use tessera_jql::ast::*;
use tessera_jql::parse;

fn value(text: &str) -> Operand {
    Operand::Value {
        text: text.into(),
        quoted: false,
    }
}

fn quoted(text: &str) -> Operand {
    Operand::Value {
        text: text.into(),
        quoted: true,
    }
}

fn terminal(field: &str, operator: Operator, operand: Operand) -> Clause {
    Clause::Terminal(Terminal {
        field: Field::Name(field.into()),
        operator,
        operand: Some(operand),
        predicates: vec![],
    })
}

fn filter(jql: &str) -> Clause {
    parse(jql)
        .unwrap_or_else(|e| panic!("{jql}: {e}"))
        .filter
        .expect("query has no filter")
}

#[test]
fn simple_equality() {
    assert_eq!(
        filter("project = PROJ"),
        terminal("project", Operator::Equals, value("PROJ"))
    );
}

#[test]
fn all_comparison_operators() {
    for (jql, op) in [
        ("votes != 3", Operator::NotEquals),
        ("votes > 3", Operator::GreaterThan),
        ("votes >= 3", Operator::GreaterThanEquals),
        ("votes < 3", Operator::LessThan),
        ("votes <= 3", Operator::LessThanEquals),
    ] {
        assert_eq!(filter(jql), terminal("votes", op, value("3")), "{jql}");
    }
}

#[test]
fn text_search_operators() {
    assert_eq!(
        filter(r#"summary ~ "login bug""#),
        terminal("summary", Operator::Contains, quoted("login bug"))
    );
    assert_eq!(
        filter("summary !~ crash"),
        terminal("summary", Operator::NotContains, value("crash"))
    );
}

#[test]
fn quoted_and_custom_field_names() {
    assert_eq!(
        filter(r#""Story Points" > 5"#),
        terminal("Story Points", Operator::GreaterThan, value("5"))
    );
    let Clause::Terminal(t) = filter("cf[10010] = 8") else {
        panic!()
    };
    assert_eq!(t.field, Field::CustomField(10010));
}

#[test]
fn issue_keys_and_relative_dates_need_no_quotes() {
    assert_eq!(
        filter("key = PROJ-123"),
        terminal("key", Operator::Equals, value("PROJ-123"))
    );
    assert_eq!(
        filter("created >= -7d"),
        terminal("created", Operator::GreaterThanEquals, value("-7d"))
    );
}

#[test]
fn escaped_quotes_in_strings() {
    assert_eq!(
        filter(r#"summary ~ "say \"hi\"""#),
        terminal("summary", Operator::Contains, quoted(r#"say "hi""#))
    );
    assert_eq!(
        filter(r"summary ~ 'it\'s'"),
        terminal("summary", Operator::Contains, quoted("it's"))
    );
}

#[test]
fn and_binds_tighter_than_or() {
    let expected = Clause::Or(vec![
        terminal("a", Operator::Equals, value("1")),
        Clause::And(vec![
            terminal("b", Operator::Equals, value("2")),
            terminal("c", Operator::Equals, value("3")),
        ]),
    ]);
    assert_eq!(filter("a = 1 OR b = 2 AND c = 3"), expected);
}

#[test]
fn parentheses_override_precedence() {
    let expected = Clause::And(vec![
        Clause::Or(vec![
            terminal("a", Operator::Equals, value("1")),
            terminal("b", Operator::Equals, value("2")),
        ]),
        terminal("c", Operator::Equals, value("3")),
    ]);
    assert_eq!(filter("(a = 1 OR b = 2) AND c = 3"), expected);
}

#[test]
fn not_prefix() {
    let expected = Clause::Not(Box::new(terminal(
        "assignee",
        Operator::Equals,
        value("jsmith"),
    )));
    assert_eq!(filter("NOT assignee = jsmith"), expected);
}

#[test]
fn keywords_are_case_insensitive() {
    let upper = parse("project = A AND status = B ORDER BY created DESC").unwrap();
    let lower = parse("project = A and status = B order by created desc").unwrap();
    assert_eq!(upper, lower);
}

#[test]
fn in_and_not_in_lists() {
    let list = Operand::List(vec![value("Open"), quoted("In Progress")]);
    assert_eq!(
        filter(r#"status IN (Open, "In Progress")"#),
        terminal("status", Operator::In, list.clone())
    );
    assert_eq!(
        filter(r#"status not in (Open, "In Progress")"#),
        terminal("status", Operator::NotIn, list)
    );
}

#[test]
fn functions_as_values() {
    let current_user = Operand::Function {
        name: "currentUser".into(),
        args: vec![],
    };
    assert_eq!(
        filter("assignee = currentUser()"),
        terminal("assignee", Operator::Equals, current_user)
    );

    let members = Operand::Function {
        name: "membersOf".into(),
        args: vec!["jira-devs".into()],
    };
    assert_eq!(
        filter(r#"assignee in membersOf("jira-devs")"#),
        terminal("assignee", Operator::In, members)
    );

    let linked = Operand::Function {
        name: "linkedIssues".into(),
        args: vec!["PROJ-1".into(), "blocks".into()],
    };
    assert_eq!(
        filter(r#"issue in linkedIssues(PROJ-1, "blocks")"#),
        terminal("issue", Operator::In, linked)
    );
}

#[test]
fn is_empty_and_is_not_null() {
    assert_eq!(
        filter("fixVersion IS EMPTY"),
        terminal("fixVersion", Operator::Is, Operand::Empty)
    );
    assert_eq!(
        filter("assignee is not null"),
        terminal("assignee", Operator::IsNot, Operand::Empty)
    );
}

#[test]
fn quoted_empty_is_text_not_the_keyword() {
    assert_eq!(
        filter(r#"labels = "EMPTY""#),
        terminal("labels", Operator::Equals, quoted("EMPTY"))
    );
}

#[test]
fn was_operators_with_predicates() {
    let Clause::Terminal(t) = filter(
        r#"status WAS "Done" BY jsmith AFTER "2026/01/01" DURING ("2026/01/01", "2026/06/30")"#,
    ) else {
        panic!()
    };
    assert_eq!(t.operator, Operator::Was);
    assert_eq!(t.operand, Some(quoted("Done")));
    assert_eq!(
        t.predicates,
        vec![
            Predicate::By(value("jsmith")),
            Predicate::After(quoted("2026/01/01")),
            Predicate::During(quoted("2026/01/01"), quoted("2026/06/30")),
        ]
    );

    for (jql, op) in [
        ("status WAS IN (Open, Done)", Operator::WasIn),
        ("status WAS NOT Done", Operator::WasNot),
        (
            "status WAS NOT IN (Open, Done) BEFORE -1w",
            Operator::WasNotIn,
        ),
    ] {
        let Clause::Terminal(t) = filter(jql) else {
            panic!()
        };
        assert_eq!(t.operator, op, "{jql}");
    }
}

#[test]
fn changed_takes_no_value_but_takes_predicates() {
    let Clause::Terminal(t) = filter(r#"status CHANGED FROM "In Progress" TO Done ON -1d"#) else {
        panic!()
    };
    assert_eq!(t.operator, Operator::Changed);
    assert_eq!(t.operand, None);
    assert_eq!(
        t.predicates,
        vec![
            Predicate::From(quoted("In Progress")),
            Predicate::To(value("Done")),
            Predicate::On(value("-1d"))
        ]
    );
}

#[test]
fn predicates_end_before_the_next_clause() {
    let Clause::And(parts) = filter("status CHANGED BY jsmith AND project = PROJ") else {
        panic!()
    };
    assert_eq!(parts.len(), 2);
}

#[test]
fn order_by_multiple_fields() {
    let query = parse("project = PROJ ORDER BY priority DESC, created").unwrap();
    assert_eq!(
        query.order_by,
        vec![
            OrderBy {
                field: Field::Name("priority".into()),
                direction: Some(Direction::Desc)
            },
            OrderBy {
                field: Field::Name("created".into()),
                direction: None
            },
        ]
    );
}

#[test]
fn order_by_alone_and_empty_query() {
    let query = parse("ORDER BY created ASC").unwrap();
    assert_eq!(query.filter, None);
    assert_eq!(query.order_by[0].direction, Some(Direction::Asc));
    assert_eq!(
        parse("   ").unwrap(),
        Query {
            filter: None,
            order_by: vec![]
        }
    );
}

#[test]
fn errors_point_at_the_problem() {
    let err = parse("project PROJ").unwrap_err();
    assert!(err.message.contains("Expected an operator"), "{err}");
    assert_eq!(err.offset, 8);

    let err = parse("(a = 1 OR b = 2").unwrap_err();
    assert!(err.message.contains("')'"), "{err}");

    assert!(
        parse("status IN Open")
            .unwrap_err()
            .message
            .contains("does not accept")
    );
    assert!(
        parse("assignee IS jsmith")
            .unwrap_err()
            .message
            .contains("does not accept")
    );
    assert!(
        parse("project = A B")
            .unwrap_err()
            .message
            .contains("after the end")
    );
    assert!(
        parse("project ! A")
            .unwrap_err()
            .message
            .contains("Unexpected character")
    );
    assert!(
        parse("project =")
            .unwrap_err()
            .message
            .contains("end of the query")
    );
}
