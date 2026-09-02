#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocation {
    pub file: Option<String>,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BaseType {
    Primitive(String),
    Array(Box<BaseType>),
    Generic(String, Vec<BaseType>),
    Union(Vec<BaseType>),
    Object(Vec<(String, BaseType)>),
    Function(Vec<RefinedParam>, Box<RefinementType>),
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefinedParam {
    pub name: String,
    pub ty: RefinementType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefinementType {
    pub base: BaseType,
    pub predicate: Option<PredicateExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PredicateExpr {
    Binary(BinaryOp, Box<PredicateExpr>, Box<PredicateExpr>),
    Logical(LogicalOp, Box<PredicateExpr>, Box<PredicateExpr>),
    Not(Box<PredicateExpr>),
    Identifier(String),
    Member(Box<PredicateExpr>, String),
    PredicateApply(String, Box<PredicateExpr>),
    Literal(Literal),
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOp {
    EqEqEq,
    NotEqEq,
    EqEq,
    NotEq,
    Gt,
    Lt,
    Gte,
    Lte,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Number(f64),
    String(String),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationTarget {
    Param {
        function_name: String,
        function_start: u32,
        param_name: String,
        index: usize,
    },
    Return {
        function_name: String,
        function_start: u32,
    },
    Variable {
        name: String,
        declaration_start: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub target: AnnotationTarget,
    pub ty: RefinementType,
    pub predicate_params: Vec<String>,
    pub loc: SourceLocation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RtError {
    pub message: String,
    pub loc: Option<SourceLocation>,
}
