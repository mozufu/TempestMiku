use crate::{Span, Spanned};

pub type Expr = Spanned<ExprKind>;
pub type Pattern = Spanned<PatternKind>;
pub type Form = Spanned<FormKind>;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub forms: Vec<Form>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormKind {
    Type(TypeDecl),
    Let {
        pattern: Pattern,
        value: Expr,
    },
    Fun {
        name: String,
        params: Vec<Pattern>,
        body: Expr,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub variants: Vec<VariantDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: String,
    pub payload: Option<TypeTerm>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeTerm {
    Named(String),
    List(Box<TypeTerm>),
    Option(Box<TypeTerm>),
    Record(Vec<(String, TypeTerm)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    String(String),
    Int(i64),
    Decimal(f64),
    Bool(bool),
    Null,
    Uri(String),
    Name(String),
    Constructor(String),
    Capability(String),
    List(Vec<Expr>),
    Record(Vec<(String, Expr)>),
    Lambda {
        params: Vec<Pattern>,
        body: Box<Expr>,
    },
    Apply {
        function: Box<Expr>,
        argument: Box<Expr>,
    },
    Field {
        target: Box<Expr>,
        field: String,
    },
    Unary {
        op: UnaryOp,
        value: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Pipe {
        value: Box<Expr>,
        target: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_value: Box<Expr>,
        else_value: Box<Expr>,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Handle {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block(Vec<Form>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Cons,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Bind(String),
    String(String),
    Int(i64),
    Bool(bool),
    Null,
    Constructor {
        name: String,
        payload: Option<Box<Pattern>>,
    },
    List(Vec<Pattern>),
    Cons {
        head: Box<Pattern>,
        tail: Box<Pattern>,
    },
    Record {
        fields: Vec<(String, Pattern)>,
        rest: bool,
    },
}
