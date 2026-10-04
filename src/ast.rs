//! Dialect-independent AST. Every dialect parser produces these types;
//! dialect-specific spellings are normalized (e.g. Oracle `MINUS` and
//! Postgres `EXCEPT` both become [`SetOperator::Except`], SQLite
//! `LIMIT a, b` becomes `limit: b, offset: a`, `x::int` becomes a `Cast`).

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub value: String,
    pub quote_style: Option<char>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectName(pub Vec<Ident>);

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Query(Box<Query>),
    Insert(Insert),
    Update(Update),
    Delete(Delete),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex),
    Drop(Drop),
}

// ---------------------------------------------------------------- queries

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub with: Option<With>,
    pub body: SetExpr,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
    pub fetch: Option<Fetch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct With {
    pub recursive: bool,
    pub ctes: Vec<Cte>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cte {
    pub name: Ident,
    pub columns: Vec<Ident>,
    pub query: Box<Query>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fetch {
    pub quantity: Option<Expr>,
    pub percent: bool,
    pub with_ties: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetExpr {
    Select(Box<Select>),
    Query(Box<Query>),
    SetOperation { op: SetOperator, quantifier: SetQuantifier, left: Box<SetExpr>, right: Box<SetExpr> },
    Values(Values),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperator {
    Union,
    Except,
    Intersect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetQuantifier {
    All,
    Distinct,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Values {
    pub rows: Vec<Vec<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub distinct: Option<Distinct>,
    pub projection: Vec<SelectItem>,
    pub from: Vec<TableWithJoins>,
    pub selection: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Distinct {
    Distinct,
    On(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    UnnamedExpr(Expr),
    ExprWithAlias { expr: Expr, alias: Ident },
    QualifiedWildcard(ObjectName),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableWithJoins {
    pub relation: TableFactor,
    pub joins: Vec<Join>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableFactor {
    Table { name: ObjectName, alias: Option<TableAlias> },
    Derived { subquery: Box<Query>, alias: Option<TableAlias> },
    Nested(Box<TableWithJoins>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableAlias {
    pub name: Ident,
    pub columns: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub kind: JoinKind,
    pub relation: TableFactor,
    pub constraint: JoinConstraint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinConstraint {
    On(Expr),
    Using(Vec<Ident>),
    Natural,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExpr {
    pub expr: Expr,
    /// `Some(true)` = ASC, `Some(false)` = DESC
    pub asc: Option<bool>,
    pub nulls_first: Option<bool>,
}

// ------------------------------------------------------------ expressions

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Identifier(Ident),
    CompoundIdentifier(Vec<Ident>),
    Value(Value),
    Placeholder(String),
    TypedString { data_type: DataType, value: String },
    BinaryOp { left: Box<Expr>, op: BinaryOperator, right: Box<Expr> },
    UnaryOp { op: UnaryOperator, expr: Box<Expr> },
    Nested(Box<Expr>),
    Tuple(Vec<Expr>),
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    Between { expr: Box<Expr>, negated: bool, low: Box<Expr>, high: Box<Expr> },
    InList { expr: Box<Expr>, list: Vec<Expr>, negated: bool },
    InSubquery { expr: Box<Expr>, subquery: Box<Query>, negated: bool },
    Like { expr: Box<Expr>, negated: bool, kind: LikeKind, pattern: Box<Expr>, escape: Option<Box<Expr>> },
    Exists { subquery: Box<Query>, negated: bool },
    Subquery(Box<Query>),
    Cast { expr: Box<Expr>, data_type: DataType },
    Case { operand: Option<Box<Expr>>, branches: Vec<CaseBranch>, else_result: Option<Box<Expr>> },
    Function(Function),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseBranch {
    pub condition: Expr,
    pub result: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: ObjectName,
    pub distinct: bool,
    pub args: Vec<FunctionArg>,
    pub over: Option<WindowSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArg {
    Wildcard,
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSpec {
    pub partition_by: Vec<Expr>,
    pub order_by: Vec<OrderByExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LikeKind {
    Like,
    ILike,
    Glob,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(String),
    String(String),
    Boolean(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    Concat,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    JsonGet,
    JsonGetText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Plus,
    Minus,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataType {
    /// Upper-cased base name, e.g. `VARCHAR2`, `DOUBLE PRECISION`.
    pub name: String,
    /// Raw modifiers, e.g. `["10", "2"]` for `NUMERIC(10, 2)`.
    pub args: Vec<String>,
    /// e.g. `WITH TIME ZONE`
    pub suffix: Option<String>,
    /// number of trailing `[]`
    pub array_dims: usize,
}

// ------------------------------------------------------------- statements

#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    /// SQLite `INSERT OR <action>`
    pub or: Option<String>,
    pub table: ObjectName,
    pub columns: Vec<Ident>,
    pub source: InsertSource,
    pub on_conflict: Option<OnConflict>,
    pub returning: Vec<SelectItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    Query(Box<Query>),
    DefaultValues,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnConflict {
    pub target: Vec<Ident>,
    pub action: OnConflictAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OnConflictAction {
    DoNothing,
    DoUpdate { assignments: Vec<Assignment>, selection: Option<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub column: ObjectName,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub table: TableFactor,
    pub assignments: Vec<Assignment>,
    pub from: Vec<TableWithJoins>,
    pub selection: Option<Expr>,
    pub returning: Vec<SelectItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delete {
    pub table: TableFactor,
    pub selection: Option<Expr>,
    pub returning: Vec<SelectItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub temporary: bool,
    pub if_not_exists: bool,
    pub name: ObjectName,
    pub columns: Vec<ColumnDef>,
    pub constraints: Vec<TableConstraint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: Ident,
    /// `None` only for dialects with untyped columns (SQLite).
    pub data_type: Option<DataType>,
    pub options: Vec<ColumnOptionDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnOptionDef {
    pub name: Option<Ident>,
    pub option: ColumnOption,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnOption {
    Null,
    NotNull,
    Default(Expr),
    PrimaryKey,
    Unique,
    Autoincrement,
    Identity { always: bool },
    References(ForeignKeyRef),
    Check(Expr),
    Collate(Ident),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyRef {
    pub table: ObjectName,
    pub columns: Vec<Ident>,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableConstraint {
    pub name: Option<Ident>,
    pub kind: TableConstraintKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableConstraintKind {
    PrimaryKey(Vec<Ident>),
    Unique(Vec<Ident>),
    ForeignKey { columns: Vec<Ident>, reference: ForeignKeyRef },
    Check(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex {
    pub unique: bool,
    pub if_not_exists: bool,
    pub name: ObjectName,
    pub table: ObjectName,
    pub columns: Vec<OrderByExpr>,
    pub selection: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Table,
    View,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropBehavior {
    Cascade,
    Restrict,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drop {
    pub object_type: ObjectType,
    pub if_exists: bool,
    pub names: Vec<ObjectName>,
    pub behavior: Option<DropBehavior>,
}
