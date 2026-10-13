//! The JQL syntax tree.

/// A full query: an optional filter and an optional sort order.
/// `ORDER BY created DESC` alone is a valid query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub filter: Option<Clause>,
    pub order_by: Vec<OrderBy>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    And(Vec<Clause>),
    Or(Vec<Clause>),
    Not(Box<Clause>),
    Terminal(Terminal),
}

/// `field operator operand [predicates...]`, for example `status WAS "Done" BY jsmith`.
#[derive(Debug, Clone, PartialEq)]
pub struct Terminal {
    pub field: Field,
    pub operator: Operator,
    /// `None` only for `CHANGED`, which takes no operand.
    pub operand: Option<Operand>,
    pub predicates: Vec<Predicate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// A system or custom field by name, such as `assignee` or `"Story Points"`.
    Name(String),
    /// A custom field by id, written `cf[10010]`.
    CustomField(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanEquals,
    LessThan,
    LessThanEquals,
    In,
    NotIn,
    Contains,
    NotContains,
    Is,
    IsNot,
    Was,
    WasIn,
    WasNot,
    WasNotIn,
    Changed,
}

impl Operator {
    /// Operators that look at field history and accept predicates such as `BY` or `DURING`.
    pub fn is_history(self) -> bool {
        matches!(
            self,
            Self::Was | Self::WasIn | Self::WasNot | Self::WasNotIn | Self::Changed
        )
    }

    fn takes_list(self) -> bool {
        matches!(self, Self::In | Self::NotIn | Self::WasIn | Self::WasNotIn)
    }

    /// Checks that the operand has a shape this operator accepts.
    pub(crate) fn accepts(self, operand: &Operand) -> bool {
        match self {
            Self::Is | Self::IsNot => matches!(operand, Operand::Empty),
            _ if self.takes_list() => {
                matches!(operand, Operand::List(_) | Operand::Function { .. })
            }
            _ => !matches!(operand, Operand::List(_)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A literal. `quoted` keeps `"EMPTY"` (text) apart from `EMPTY` (no value).
    Value { text: String, quoted: bool },
    /// `EMPTY` or `NULL`.
    Empty,
    /// A function call such as `currentUser()` or `membersOf("devs")`.
    Function { name: String, args: Vec<String> },
    /// `(a, b, c)`.
    List(Vec<Operand>),
}

/// History predicates for `WAS` and `CHANGED`.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    After(Operand),
    Before(Operand),
    By(Operand),
    During(Operand, Operand),
    On(Operand),
    From(Operand),
    To(Operand),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderBy {
    pub field: Field,
    pub direction: Option<Direction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}
