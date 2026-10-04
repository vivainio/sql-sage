//! A query *fingerprint*: which tables a statement touches (and how), and which
//! operations it applies. Literals, columns, aliases, bind parameters, CTE names
//! and dialect spelling are ignored, so the same logical query shape gets the
//! same fingerprint on every dialect and with any parameter values.
//!
//! ```text
//! select | customers:read orders:read | aggregate filter group join:left limit sort | count() lower()
//! ```
//!
//! Sections are `kind | tables | operations | functions`; an empty section is `-`.
//!
//! [`Statement::fingerprint_for`] also fills in a compatibility code such as
//! `ora,pg,lite`: the dialects the statement runs on without manual changes
//! (see [`crate::compat`]). It is not part of [`Fingerprint::canonical`] or
//! [`Fingerprint::id`], so the id depends only on the query shape.

use crate::ast::*;
use crate::dialect::DialectKind;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
    CreateTable,
    CreateIndex,
    DropTable,
    DropView,
    DropIndex,
}

impl fmt::Display for StatementKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StatementKind::Select => "select",
            StatementKind::Insert => "insert",
            StatementKind::Update => "update",
            StatementKind::Delete => "delete",
            StatementKind::CreateTable => "create_table",
            StatementKind::CreateIndex => "create_index",
            StatementKind::DropTable => "drop_table",
            StatementKind::DropView => "drop_view",
            StatementKind::DropIndex => "drop_index",
        })
    }
}

/// How a statement uses a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Access {
    Read,
    Insert,
    Update,
    Delete,
    Create,
    Drop,
    Index,
}

impl Access {
    pub fn is_write(self) -> bool {
        self != Access::Read
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Access::Read => "read",
            Access::Insert => "insert",
            Access::Update => "update",
            Access::Delete => "delete",
            Access::Create => "create",
            Access::Drop => "drop",
            Access::Index => "index",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    Filter,
    Join(JoinKind),
    NaturalJoin,
    Group,
    Having,
    Distinct,
    Sort,
    Limit,
    Union,
    UnionAll,
    Intersect,
    Except,
    Aggregate,
    Window,
    Subquery,
    Cte,
    RecursiveCte,
    Values,
    Upsert,
    Returning,
    ForeignKey,
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Operation::Filter => "filter",
            Operation::Join(JoinKind::Inner) => "join:inner",
            Operation::Join(JoinKind::Left) => "join:left",
            Operation::Join(JoinKind::Right) => "join:right",
            Operation::Join(JoinKind::Full) => "join:full",
            Operation::Join(JoinKind::Cross) => "join:cross",
            Operation::NaturalJoin => "join:natural",
            Operation::Group => "group",
            Operation::Having => "having",
            Operation::Distinct => "distinct",
            Operation::Sort => "sort",
            Operation::Limit => "limit",
            Operation::Union => "union",
            Operation::UnionAll => "union_all",
            Operation::Intersect => "intersect",
            Operation::Except => "except",
            Operation::Aggregate => "aggregate",
            Operation::Window => "window",
            Operation::Subquery => "subquery",
            Operation::Cte => "cte",
            Operation::RecursiveCte => "recursive",
            Operation::Values => "values",
            Operation::Upsert => "upsert",
            Operation::Returning => "returning",
            Operation::ForeignKey => "foreign_key",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub kind: StatementKind,
    /// Normalized table name -> the ways the statement uses it.
    pub tables: BTreeMap<String, BTreeSet<Access>>,
    pub operations: BTreeSet<Operation>,
    /// Lower-cased names of every function called (aggregates and window
    /// functions included), e.g. `count`, `nvl`, `pg_catalog.now`.
    pub functions: BTreeSet<String>,
    /// Dialects the statement runs on without manual changes, in `ora, pg, lite`
    /// order. Empty unless computed with [`Statement::fingerprint_for`].
    pub compat: Vec<DialectKind>,
    /// Dialects where nothing is known to be wrong, but the statement calls
    /// functions that cannot be verified there (not built-ins of any supported
    /// dialect, so probably user-defined).
    pub compat_maybe: Vec<DialectKind>,
}

impl Fingerprint {
    /// Compatibility code such as `ora,pg,lite`; a `?` marks a dialect that cannot
    /// be fully verified (`ora?,pg,lite?`). `-` when no dialect is compatible (or
    /// compatibility was not computed).
    pub fn compat_code(&self) -> String {
        let parts: Vec<String> = DialectKind::ALL
            .into_iter()
            .filter_map(|k| {
                if self.compat.contains(&k) {
                    Some(k.code().to_string())
                } else if self.compat_maybe.contains(&k) {
                    Some(format!("{}?", k.code()))
                } else {
                    None
                }
            })
            .collect();
        if parts.is_empty() {
            "-".into()
        } else {
            parts.join(",")
        }
    }

    /// Tables the statement modifies or defines.
    pub fn writes(&self) -> Vec<&str> {
        self.tables.iter().filter(|(_, a)| a.iter().any(|a| a.is_write())).map(|(t, _)| t.as_str()).collect()
    }

    /// Tables the statement only reads.
    pub fn reads(&self) -> Vec<&str> {
        self.tables.iter().filter(|(_, a)| a.iter().all(|a| !a.is_write())).map(|(t, _)| t.as_str()).collect()
    }

    pub fn is_read_only(&self) -> bool {
        self.writes().is_empty()
    }

    /// Stable text form: `kind | table:access ... | operation ... | function() ...`.
    pub fn canonical(&self) -> String {
        let tables: Vec<String> = self
            .tables
            .iter()
            .map(|(t, a)| format!("{t}:{}", a.iter().map(|a| a.to_string()).collect::<Vec<_>>().join("+")))
            .collect();
        let ops: Vec<String> = self.operations.iter().map(|o| o.to_string()).collect();
        let funcs: Vec<String> = self.functions.iter().map(|f| format!("{f}()")).collect();
        let section = |v: Vec<String>| if v.is_empty() { "-".to_string() } else { v.join(" ") };
        format!("{} | {} | {} | {}", self.kind, section(tables), section(ops), section(funcs))
    }

    /// 64-bit FNV-1a hash of [`Self::canonical`] as 16 hex digits. Stable across
    /// runs; may change between major versions if the fingerprint rules change.
    pub fn id(&self) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.canonical().bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl Statement {
    pub fn fingerprint(&self) -> Fingerprint {
        let mut c = Collector::default();
        let kind = c.statement(self);
        Fingerprint {
            kind,
            tables: c.tables,
            operations: c.ops,
            functions: c.functions,
            compat: vec![],
            compat_maybe: vec![],
        }
    }

    /// Like [`Self::fingerprint`], plus the compatibility code for a statement
    /// written for `source`.
    pub fn fingerprint_for(&self, source: DialectKind) -> Fingerprint {
        let mut f = self.fingerprint();
        let src = source.dialect();
        for t in DialectKind::ALL {
            match crate::compat::verdict(self, src.as_ref(), t.dialect().as_ref()) {
                crate::compat::Verdict::Compatible => f.compat.push(t),
                crate::compat::Verdict::Unverified => f.compat_maybe.push(t),
                crate::compat::Verdict::Incompatible => {}
            }
        }
        f
    }
}

const AGGREGATES: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "total",
    "string_agg",
    "listagg",
    "group_concat",
    "array_agg",
    "json_agg",
    "jsonb_agg",
    "json_arrayagg",
    "stddev",
    "variance",
    "median",
    "bool_and",
    "bool_or",
    "every",
];

#[derive(Default)]
struct Collector {
    tables: BTreeMap<String, BTreeSet<Access>>,
    ops: BTreeSet<Operation>,
    functions: BTreeSet<String>,
    /// CTE names in scope (lower-case); references to them are not tables.
    ctes: Vec<BTreeSet<String>>,
}

fn normalize(n: &ObjectName) -> String {
    n.0.iter()
        .map(|i| match i.quote_style {
            Some(_)
                if i.value != i.value.to_lowercase() || !i.value.chars().all(|c| c.is_alphanumeric() || c == '_') =>
            {
                format!("\"{}\"", i.value)
            }
            _ => i.value.to_lowercase(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

impl Collector {
    fn table(&mut self, n: &ObjectName, access: Access) {
        let name = normalize(n);
        if name == "dual" {
            return;
        }
        if access == Access::Read && n.0.len() == 1 && self.ctes.iter().any(|s| s.contains(&name)) {
            return;
        }
        self.tables.entry(name).or_default().insert(access);
    }

    fn statement(&mut self, s: &Statement) -> StatementKind {
        match s {
            Statement::Query(q) => {
                self.query(q);
                StatementKind::Select
            }
            Statement::Insert(i) => {
                self.table(&i.table, Access::Insert);
                if let InsertSource::Query(q) = &i.source {
                    self.query(q);
                }
                if let Some(oc) = &i.on_conflict {
                    self.ops.insert(Operation::Upsert);
                    if let OnConflictAction::DoUpdate { assignments, selection } = &oc.action {
                        for a in assignments {
                            self.expr(&a.value);
                        }
                        if let Some(w) = selection {
                            self.expr(w);
                        }
                    }
                }
                self.returning(&i.returning);
                StatementKind::Insert
            }
            Statement::Update(u) => {
                self.write_target(&u.table, Access::Update);
                for a in &u.assignments {
                    self.expr(&a.value);
                }
                for t in &u.from {
                    self.table_with_joins(t);
                }
                if let Some(w) = &u.selection {
                    self.ops.insert(Operation::Filter);
                    self.expr(w);
                }
                self.returning(&u.returning);
                StatementKind::Update
            }
            Statement::Delete(d) => {
                self.write_target(&d.table, Access::Delete);
                if let Some(w) = &d.selection {
                    self.ops.insert(Operation::Filter);
                    self.expr(w);
                }
                self.returning(&d.returning);
                StatementKind::Delete
            }
            Statement::CreateTable(c) => {
                self.table(&c.name, Access::Create);
                let refs = c
                    .columns
                    .iter()
                    .flat_map(|col| col.options.iter())
                    .filter_map(|o| match &o.option {
                        ColumnOption::References(r) => Some(r),
                        _ => None,
                    })
                    .chain(c.constraints.iter().filter_map(|k| match &k.kind {
                        TableConstraintKind::ForeignKey { reference, .. } => Some(reference),
                        _ => None,
                    }));
                for r in refs {
                    self.ops.insert(Operation::ForeignKey);
                    if normalize(&r.table) != normalize(&c.name) {
                        self.table(&r.table, Access::Read);
                    }
                }
                StatementKind::CreateTable
            }
            Statement::CreateIndex(c) => {
                self.table(&c.table, Access::Index);
                if c.selection.is_some() {
                    self.ops.insert(Operation::Filter);
                }
                StatementKind::CreateIndex
            }
            Statement::Drop(d) => {
                for n in &d.names {
                    self.table(n, Access::Drop);
                }
                match d.object_type {
                    ObjectType::Table => StatementKind::DropTable,
                    ObjectType::View => StatementKind::DropView,
                    ObjectType::Index => StatementKind::DropIndex,
                }
            }
        }
    }

    fn returning(&mut self, r: &[SelectItem]) {
        if !r.is_empty() {
            self.ops.insert(Operation::Returning);
            for i in r {
                self.select_item(i);
            }
        }
    }

    fn write_target(&mut self, t: &TableFactor, access: Access) {
        match t {
            TableFactor::Table { name, .. } => self.table(name, access),
            other => self.table_factor(other),
        }
    }

    fn query(&mut self, q: &Query) {
        let mut scope = BTreeSet::new();
        if let Some(w) = &q.with {
            self.ops.insert(Operation::Cte);
            if w.recursive {
                self.ops.insert(Operation::RecursiveCte);
            }
            scope = w.ctes.iter().map(|c| c.name.value.to_lowercase()).collect();
        }
        self.ctes.push(scope);
        if let Some(w) = &q.with {
            for c in &w.ctes {
                self.query(&c.query);
            }
        }
        self.set_expr(&q.body);
        if !q.order_by.is_empty() {
            self.ops.insert(Operation::Sort);
            for o in &q.order_by {
                self.expr(&o.expr);
            }
        }
        if q.limit.is_some() || q.offset.is_some() || q.fetch.is_some() {
            self.ops.insert(Operation::Limit);
        }
        self.ctes.pop();
    }

    fn set_expr(&mut self, e: &SetExpr) {
        match e {
            SetExpr::Select(s) => self.select(s),
            SetExpr::Query(q) => self.query(q),
            SetExpr::Values(v) => {
                self.ops.insert(Operation::Values);
                for e in v.rows.iter().flatten() {
                    self.expr(e);
                }
            }
            SetExpr::SetOperation { op, quantifier, left, right } => {
                self.ops.insert(match (op, quantifier) {
                    (SetOperator::Union, SetQuantifier::All) => Operation::UnionAll,
                    (SetOperator::Union, _) => Operation::Union,
                    (SetOperator::Intersect, _) => Operation::Intersect,
                    (SetOperator::Except, _) => Operation::Except,
                });
                self.set_expr(left);
                self.set_expr(right);
            }
        }
    }

    fn select(&mut self, s: &Select) {
        if let Some(d) = &s.distinct {
            self.ops.insert(Operation::Distinct);
            if let Distinct::On(e) = d {
                for e in e {
                    self.expr(e);
                }
            }
        }
        for i in &s.projection {
            self.select_item(i);
        }
        for t in &s.from {
            self.table_with_joins(t);
        }
        if let Some(w) = &s.selection {
            self.ops.insert(Operation::Filter);
            self.expr(w);
        }
        if !s.group_by.is_empty() {
            self.ops.insert(Operation::Group);
            for e in &s.group_by {
                self.expr(e);
            }
        }
        if let Some(h) = &s.having {
            self.ops.insert(Operation::Having);
            self.expr(h);
        }
    }

    fn select_item(&mut self, i: &SelectItem) {
        match i {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => self.expr(e),
            SelectItem::QualifiedWildcard(_) | SelectItem::Wildcard => {}
        }
    }

    fn table_with_joins(&mut self, t: &TableWithJoins) {
        self.table_factor(&t.relation);
        for j in &t.joins {
            self.ops.insert(if j.constraint == JoinConstraint::Natural {
                Operation::NaturalJoin
            } else {
                Operation::Join(j.kind)
            });
            self.table_factor(&j.relation);
            if let JoinConstraint::On(e) = &j.constraint {
                self.expr(e);
            }
        }
    }

    fn table_factor(&mut self, t: &TableFactor) {
        match t {
            TableFactor::Table { name, .. } => self.table(name, Access::Read),
            TableFactor::Derived { subquery, .. } => {
                self.ops.insert(Operation::Subquery);
                self.query(subquery);
            }
            TableFactor::Nested(t) => self.table_with_joins(t),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Identifier(_)
            | Expr::CompoundIdentifier(_)
            | Expr::Value(_)
            | Expr::Placeholder(_)
            | Expr::TypedString { .. } => {}
            Expr::BinaryOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::UnaryOp { expr, .. }
            | Expr::Nested(expr)
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::Cast { expr, .. } => self.expr(expr),
            Expr::Tuple(t) => t.iter().for_each(|e| self.expr(e)),
            Expr::Between { expr, low, high, .. } => {
                self.expr(expr);
                self.expr(low);
                self.expr(high);
            }
            Expr::InList { expr, list, .. } => {
                self.expr(expr);
                list.iter().for_each(|e| self.expr(e));
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.expr(expr);
                self.ops.insert(Operation::Subquery);
                self.query(subquery);
            }
            Expr::Like { expr, pattern, escape, .. } => {
                self.expr(expr);
                self.expr(pattern);
                if let Some(e) = escape {
                    self.expr(e);
                }
            }
            Expr::Exists { subquery, .. } | Expr::Subquery(subquery) => {
                self.ops.insert(Operation::Subquery);
                self.query(subquery);
            }
            Expr::Case { operand, branches, else_result } => {
                if let Some(o) = operand {
                    self.expr(o);
                }
                for b in branches {
                    self.expr(&b.condition);
                    self.expr(&b.result);
                }
                if let Some(e) = else_result {
                    self.expr(e);
                }
            }
            Expr::Function(f) => {
                self.functions.insert(f.name.0.iter().map(|i| i.value.to_lowercase()).collect::<Vec<_>>().join("."));
                for a in &f.args {
                    if let FunctionArg::Expr(e) = a {
                        self.expr(e);
                    }
                }
                match &f.over {
                    Some(w) => {
                        self.ops.insert(Operation::Window);
                        w.partition_by.iter().for_each(|e| self.expr(e));
                        w.order_by.iter().for_each(|o| self.expr(&o.expr));
                    }
                    None => {
                        let name = f.name.0.last().map(|i| i.value.to_lowercase()).unwrap_or_default();
                        if AGGREGATES.contains(&name.as_str()) {
                            self.ops.insert(Operation::Aggregate);
                        }
                    }
                }
            }
        }
    }
}
