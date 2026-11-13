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
    InsertAll(InsertAll),
    Update(Update),
    Delete(Delete),
    Merge(Merge),
    CreateTable(CreateTable),
    CreateIndex(CreateIndex),
    CreateSequence(CreateSequence),
    CreateView(CreateView),
    AlterTable(AlterTable),
    Truncate(Truncate),
    Drop(Drop),
    /// A statement that is accepted but not modeled (`COMMIT`, `GRANT`, `ALTER VIEW ... COMPILE`,
    /// a PL/SQL block, ...). Its text is kept as written.
    Other(Other),
    /// A PL/SQL block, function, procedure or trigger.
    Plsql(Box<PlsqlUnit>),
    /// T-SQL `IF condition statement [ELSE statement]`.
    If(IfStatement),
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
    /// `FOR UPDATE [OF ...] [NOWAIT | WAIT n | SKIP LOCKED]`, as text after `FOR UPDATE`.
    pub for_update: Option<String>,
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
    /// Oracle `START WITH ... CONNECT BY ...`
    pub hierarchy: Option<Hierarchy>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hierarchy {
    pub start_with: Option<Expr>,
    pub nocycle: bool,
    pub connect_by: Expr,
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
    TypedString {
        data_type: DataType,
        value: String,
    },
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOperator,
        expr: Box<Expr>,
    },
    Nested(Box<Expr>),
    Tuple(Vec<Expr>),
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    Between {
        expr: Box<Expr>,
        negated: bool,
        low: Box<Expr>,
        high: Box<Expr>,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<Query>,
        negated: bool,
    },
    Like {
        expr: Box<Expr>,
        negated: bool,
        kind: LikeKind,
        pattern: Box<Expr>,
        escape: Option<Box<Expr>>,
    },
    Exists {
        subquery: Box<Query>,
        negated: bool,
    },
    Subquery(Box<Query>),
    Cast {
        expr: Box<Expr>,
        data_type: DataType,
    },
    /// `expr.member` or `expr.member(args)`: a method call on a value, as in `XMLELEMENT(...).EXTRACT('x')`.
    Postfix {
        expr: Box<Expr>,
        member: Ident,
        args: Option<Vec<Expr>>,
    },
    /// `expr AT TIME ZONE zone`
    AtTimeZone {
        expr: Box<Expr>,
        zone: Box<Expr>,
    },
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<CaseBranch>,
        else_result: Option<Box<Expr>>,
    },
    Function(Function),
    Trim {
        side: Option<TrimSide>,
        chars: Option<Box<Expr>>,
        expr: Box<Expr>,
    },
}

/// Which end `TRIM(<side> chars FROM expr)` cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimSide {
    Leading,
    Trailing,
    Both,
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
    /// `ORDER BY` inside the arguments: `XMLAGG(x ORDER BY y)`.
    pub order_by: Vec<OrderByExpr>,
    /// `LISTAGG(...) WITHIN GROUP (ORDER BY ...)`
    pub within_group: Vec<OrderByExpr>,
    pub over: Option<WindowSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArg {
    /// `name => value`
    Named(Ident, Expr),
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
    /// `X'CAFE'`: the hex digits of a blob literal.
    Blob(String),
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
    /// Oracle `PRIOR expr` in `CONNECT BY`
    Prior,
    /// Oracle `CONNECT_BY_ROOT expr`
    ConnectByRoot,
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

impl DataType {
    /// The units of an interval literal (` DAY`, ` YEAR TO MONTH`), or an empty string.
    pub fn interval_units(&self) -> String {
        self.suffix.as_ref().map(|u| format!(" {u}")).unwrap_or_default()
    }
}

// ------------------------------------------------------------- statements

#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    /// SQLite `INSERT OR <action>`
    pub or: Option<String>,
    pub table: ObjectName,
    /// Oracle `INSERT INTO t alias (...)`
    pub alias: Option<Ident>,
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
    /// More than one for `SET (a, b) = (SELECT ...)`.
    pub columns: Vec<ObjectName>,
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
    /// With `as_query` only the names are given (`CREATE TABLE t (a, b) AS ...`, `data_type` is
    /// `None`), or none at all.
    pub columns: Vec<ColumnDef>,
    pub constraints: Vec<TableConstraint>,
    /// `CREATE TABLE ... AS SELECT ...`
    pub as_query: Option<Box<Query>>,
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
    Identity {
        always: bool,
    },
    /// `GENERATED ALWAYS AS (expr)` / Oracle `AS (expr)`: a computed column.
    Generated {
        expr: Expr,
        stored: bool,
    },
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

/// Oracle multi-table insert: `INSERT ALL|FIRST [WHEN c THEN] INTO ... [ELSE INTO ...] SELECT ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertAll {
    /// `FIRST` rather than `ALL`.
    pub first: bool,
    /// One arm without a condition for plain `INSERT ALL INTO ...`.
    pub arms: Vec<InsertAllArm>,
    pub else_targets: Vec<InsertTarget>,
    pub source: Box<Query>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertAllArm {
    pub condition: Option<Expr>,
    pub targets: Vec<InsertTarget>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertTarget {
    pub table: ObjectName,
    pub columns: Vec<Ident>,
    /// Empty when `VALUES` is omitted (the select list is used).
    pub values: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merge {
    pub target: TableFactor,
    pub source: TableFactor,
    pub on: Expr,
    pub clauses: Vec<MergeClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeClause {
    Matched { condition: Option<Expr>, action: MergeAction },
    NotMatched { condition: Option<Expr>, columns: Vec<Ident>, values: Vec<Expr>, selection: Option<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum MergeAction {
    /// `delete_where` is Oracle's `DELETE WHERE` after the assignments.
    Update {
        assignments: Vec<Assignment>,
        selection: Option<Expr>,
        delete_where: Option<Expr>,
    },
    Delete,
    DoNothing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Truncate {
    pub tables: Vec<ObjectName>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStatement {
    pub condition: Expr,
    pub then: Box<Statement>,
    pub otherwise: Option<Box<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlsqlUnit {
    pub kind: PlsqlKind,
    pub name: Option<ObjectName>,
    pub params: Vec<PlParam>,
    /// The `RETURN` type of a function.
    pub returns: Option<String>,
    pub trigger: Option<PlTrigger>,
    pub block: PlsqlBlock,
    /// The unit as source text (spacing normalized).
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlsqlKind {
    Block,
    Function,
    Procedure,
    Trigger,
}

impl PlsqlKind {
    pub fn label(self) -> &'static str {
        match self {
            PlsqlKind::Block => "PL/SQL BLOCK",
            PlsqlKind::Function => "CREATE FUNCTION",
            PlsqlKind::Procedure => "CREATE PROCEDURE",
            PlsqlKind::Trigger => "CREATE TRIGGER",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlTrigger {
    /// `BEFORE`, `AFTER` or `INSTEAD OF`.
    pub timing: String,
    /// `INSERT`, `DELETE`, `UPDATE`, `UPDATE OF a, b`.
    pub events: Vec<String>,
    pub table: ObjectName,
    pub for_each_row: bool,
    pub when: Option<Expr>,
    /// A compound trigger: `FOR ... COMPOUND TRIGGER`, with one block per timing point
    /// (`BEFORE STATEMENT`, `AFTER EACH ROW`, ...). The unit's own block holds the shared declarations.
    pub sections: Vec<(String, PlsqlBlock)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlParam {
    pub name: Ident,
    /// `IN`, `OUT`, `IN OUT`.
    pub mode: Option<String>,
    /// The type as written: `NUMBER`, `VARCHAR2(10)`, `t.c%TYPE`.
    pub data_type: String,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlsqlBlock {
    pub label: Option<Ident>,
    pub declarations: Vec<PlDeclaration>,
    pub body: Vec<PlStatement>,
    pub handlers: Vec<PlHandler>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlHandler {
    /// Exception names; `OTHERS` is one of them.
    pub exceptions: Vec<ObjectName>,
    pub body: Vec<PlStatement>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlDeclaration {
    Variable {
        name: Ident,
        constant: bool,
        data_type: String,
        not_null: bool,
        default: Option<Expr>,
    },
    Cursor {
        name: Ident,
        params: Vec<PlParam>,
        query: Box<Query>,
    },
    Exception(Ident),
    Pragma(String),
    /// `TYPE name IS ...`, kept as text.
    Type {
        name: Ident,
        definition: String,
    },
    Subprogram(Box<PlSubprogram>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlSubprogram {
    pub is_function: bool,
    pub name: Ident,
    pub params: Vec<PlParam>,
    pub returns: Option<String>,
    /// `None` for a forward declaration.
    pub block: Option<PlsqlBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlStatement {
    Null,
    Assign {
        target: Expr,
        value: Expr,
    },
    If {
        branches: Vec<(Expr, Vec<PlStatement>)>,
        else_body: Vec<PlStatement>,
    },
    Case {
        operand: Option<Expr>,
        whens: Vec<(Expr, Vec<PlStatement>)>,
        else_body: Option<Vec<PlStatement>>,
    },
    Loop {
        label: Option<Ident>,
        body: Vec<PlStatement>,
    },
    While {
        label: Option<Ident>,
        condition: Expr,
        body: Vec<PlStatement>,
    },
    ForRange {
        label: Option<Ident>,
        var: Ident,
        reverse: bool,
        low: Expr,
        high: Expr,
        body: Vec<PlStatement>,
    },
    ForIn {
        label: Option<Ident>,
        var: Ident,
        source: PlCursorSource,
        body: Vec<PlStatement>,
    },
    Forall {
        var: Ident,
        low: Expr,
        high: Expr,
        statement: Box<PlStatement>,
    },
    Return(Option<Expr>),
    Exit {
        label: Option<Ident>,
        when: Option<Expr>,
    },
    Continue {
        label: Option<Ident>,
        when: Option<Expr>,
    },
    Goto(Ident),
    Raise(Option<ObjectName>),
    /// Embedded SQL; `into` are the `INTO` / `BULK COLLECT INTO` variables.
    Sql {
        statement: Box<Statement>,
        into: Vec<Expr>,
    },
    ExecuteImmediate {
        sql: Expr,
        into: Vec<Expr>,
        using: Vec<Expr>,
    },
    Open {
        cursor: Expr,
        query: Option<Box<Query>>,
    },
    Fetch {
        cursor: Expr,
        into: Vec<Expr>,
    },
    Close(Expr),
    /// A procedure call, or any other expression statement.
    Call(Expr),
    Block(Box<PlsqlBlock>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlCursorSource {
    Query(Box<Query>),
    /// A declared cursor, with its arguments.
    Cursor(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Other {
    /// The leading keywords, upper-cased: `COMMIT`, `ALTER VIEW`, `PL/SQL BLOCK`, ...
    pub kind: String,
    pub text: String,
}

/// Does an `ALTER TABLE` clause (as text) only set storage options (`MOVE`, `LOGGING`,
/// `PARALLEL`, `COMPRESS`, `ENABLE ROW MOVEMENT`, ...)? Those have no meaning in SQLite.
pub fn alter_clause_is_physical(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    let mut words = upper.split_whitespace();
    match words.next() {
        Some(
            "MOVE" | "LOGGING" | "NOLOGGING" | "PARALLEL" | "NOPARALLEL" | "COMPRESS" | "NOCOMPRESS" | "SHRINK"
            | "CACHE" | "NOCACHE" | "MONITORING" | "NOMONITORING" | "INMEMORY",
        ) => true,
        Some("ENABLE" | "DISABLE") => words.next() == Some("ROW"),
        Some("MODIFY") => words.next() == Some("PARTITION") && words.next() == Some("BY"),
        _ => false,
    }
}

impl Other {
    /// What a statement that cannot be translated in general means in SQLite, if anything:
    /// `ALTER INDEX i REBUILD ...` is `REINDEX i`, and index storage options are no-ops.
    pub fn sqlite_equivalent(&self) -> Option<String> {
        if self.kind != "ALTER INDEX" {
            return None;
        }
        let words: Vec<&str> = self.text.split_whitespace().collect();
        let (name, action) = (words.get(2)?, words.get(3)?.to_ascii_uppercase());
        match action.as_str() {
            "REBUILD" => Some(format!("REINDEX {name}")),
            "PARALLEL" | "NOPARALLEL" | "LOGGING" | "NOLOGGING" | "COMPRESS" | "NOCOMPRESS" | "MONITORING"
            | "NOMONITORING" | "SHRINK" | "COALESCE" | "VISIBLE" | "INVISIBLE" => {
                Some(format!("-- {} (storage option: nothing to do in SQLite)", self.text))
            }
            _ => None,
        }
    }

    /// Transaction control is the same in every dialect, so it translates as written.
    pub fn is_portable(&self) -> bool {
        matches!(self.kind.as_str(), "COMMIT" | "ROLLBACK" | "SAVEPOINT")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSequence {
    pub if_not_exists: bool,
    pub name: ObjectName,
    pub options: Vec<SequenceOption>,
}

/// Numbers are kept as written (they may be negative or exceed 64 bits).
#[derive(Debug, Clone, PartialEq)]
pub enum SequenceOption {
    StartWith(String),
    IncrementBy(String),
    /// `None` is `NOMINVALUE`.
    MinValue(Option<String>),
    /// `None` is `NOMAXVALUE`.
    MaxValue(Option<String>),
    Cycle(bool),
    /// `None` is `NOCACHE`.
    Cache(Option<String>),
    Order(bool),
}

impl SequenceOption {
    /// `START WITH 1`, `NOCYCLE`, ... in the Oracle spelling.
    pub fn text(&self) -> String {
        match self {
            SequenceOption::StartWith(n) => format!("START WITH {n}"),
            SequenceOption::IncrementBy(n) => format!("INCREMENT BY {n}"),
            SequenceOption::MinValue(Some(n)) => format!("MINVALUE {n}"),
            SequenceOption::MinValue(None) => "NOMINVALUE".into(),
            SequenceOption::MaxValue(Some(n)) => format!("MAXVALUE {n}"),
            SequenceOption::MaxValue(None) => "NOMAXVALUE".into(),
            SequenceOption::Cycle(true) => "CYCLE".into(),
            SequenceOption::Cycle(false) => "NOCYCLE".into(),
            SequenceOption::Cache(Some(n)) => format!("CACHE {n}"),
            SequenceOption::Cache(None) => "NOCACHE".into(),
            SequenceOption::Order(true) => "ORDER".into(),
            SequenceOption::Order(false) => "NOORDER".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateView {
    pub or_replace: bool,
    /// Oracle `FORCE`: create even if the underlying objects do not exist.
    pub force: bool,
    pub name: ObjectName,
    pub columns: Vec<Ident>,
    pub query: Box<Query>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterTable {
    pub if_exists: bool,
    pub name: ObjectName,
    pub actions: Vec<AlterAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlterAction {
    AddColumn {
        if_not_exists: bool,
        column: ColumnDef,
    },
    DropColumn {
        if_exists: bool,
        name: Ident,
        behavior: Option<DropBehavior>,
    },
    RenameColumn {
        old: Ident,
        new: Ident,
    },
    RenameTable(Ident),
    AlterColumnType {
        name: Ident,
        data_type: DataType,
    },
    SetNotNull(Ident),
    DropNotNull(Ident),
    SetDefault {
        name: Ident,
        default: Expr,
    },
    DropDefault(Ident),
    AddConstraint(TableConstraint),
    DropConstraint {
        if_exists: bool,
        name: Ident,
        behavior: Option<DropBehavior>,
    },
    DropPrimaryKey,
    DropUnique(Vec<Ident>),
    RenameConstraint {
        old: Ident,
        new: Ident,
    },
    /// A clause that is accepted but not modeled: `MOVE`, `ENABLE CONSTRAINT`, `LOGGING`,
    /// partition maintenance, ... Its text is kept as written.
    Other(String),
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
    Sequence,
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
