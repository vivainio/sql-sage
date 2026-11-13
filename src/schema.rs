//! Derives a database schema by *simulating* DDL: feed it migration files in order and it
//! applies each `CREATE TABLE`, `ALTER TABLE`, `CREATE INDEX` and `DROP` to an in-memory
//! model, reporting statements that cannot apply (dropping a missing column, creating a
//! table twice, ...). The result can be re-emitted as DDL for any dialect.
//!
//! Names are matched on their last component (`public.users` and `users` are the same table)
//! and folded the way the dialect resolves unquoted identifiers. DML and queries are ignored.
//!
//! ```
//! use sql_sage::{dialect::DialectKind, schema::Schema};
//!
//! let mut schema = Schema::new(DialectKind::Postgres);
//! let errors = schema
//!     .apply_sql("CREATE TABLE t (id int PRIMARY KEY); ALTER TABLE t ADD COLUMN name text NOT NULL;")
//!     .unwrap();
//! assert!(errors.errors.is_empty() && errors.skipped.is_empty());
//! assert_eq!(
//!     schema.to_sql(DialectKind::Postgres).unwrap(),
//!     "CREATE TABLE t (id INT PRIMARY KEY, name TEXT NOT NULL);"
//! );
//! ```

use crate::ast::*;
use crate::dialect::{DialectKind, IdentifierFold};
use crate::emit::EmitError;
use crate::error::{Location, ParseError};
use std::fmt;

/// A statement that could not be applied to the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub message: String,
    /// Where the statement starts, when known.
    pub location: Option<Location>,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Some(l) => write!(f, "{} at line {}, column {}", self.message, l.line, l.column),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for SchemaError {}

fn err<T>(message: impl Into<String>) -> Result<T, SchemaError> {
    Err(SchemaError { message: message.into(), location: None })
}

/// The state of the database after applying a sequence of statements.
#[derive(Debug, Clone)]
pub struct Schema {
    kind: DialectKind,
    fold: IdentifierFold,
    tables: Vec<CreateTable>,
    indexes: Vec<CreateIndex>,
    sequences: Vec<CreateSequence>,
    views: Vec<CreateView>,
}

/// What [`Schema::apply_sql`] did with its input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Statements that parsed but could not apply (a missing table, a duplicate column, ...).
    pub errors: Vec<SchemaError>,
    /// Statements that did not parse (PL/SQL, unsupported syntax); they are left out of the schema.
    pub skipped: Vec<SchemaError>,
    /// Statements taken from `EXECUTE IMMEDIATE` strings that could not apply. Such statements
    /// are usually guarded (`IF ... THEN`) and are assumed to run, so these are likely to be
    /// guards that would have skipped them, not mistakes.
    pub dynamic_errors: Vec<SchemaError>,
    /// PL/SQL units (anonymous blocks, functions, procedures, triggers, packages, types). They
    /// are accepted but not analyzed, apart from SQL in `EXECUTE IMMEDIATE` strings of blocks.
    pub plsql: Vec<SchemaError>,
}

fn key(fold: IdentifierFold, id: &Ident) -> String {
    match fold {
        IdentifierFold::Insensitive => id.value.to_lowercase(),
        _ if id.quote_style.is_some() => id.value.clone(),
        _ => fold.fold(&id.value),
    }
}

fn last(n: &ObjectName) -> &Ident {
    n.0.last().expect("object names are never empty")
}

fn constraint_columns(k: &mut TableConstraintKind) -> Option<&mut Vec<Ident>> {
    match k {
        TableConstraintKind::PrimaryKey(c) | TableConstraintKind::Unique(c) => Some(c),
        TableConstraintKind::ForeignKey { columns, .. } => Some(columns),
        TableConstraintKind::Check(_) => None,
    }
}

fn leftmost_select(e: &SetExpr) -> Option<&Select> {
    match e {
        SetExpr::Select(s) => Some(s),
        SetExpr::Query(q) => leftmost_select(&q.body),
        SetExpr::SetOperation { left, .. } => leftmost_select(left),
        SetExpr::Values(_) => None,
    }
}

fn index_uses(fold: IdentifierFold, i: &CreateIndex, col: &str) -> bool {
    i.columns.iter().any(|c| matches!(&c.expr, Expr::Identifier(id) if key(fold, id) == col))
}

impl Schema {
    /// An empty schema; `kind` decides how identifiers are matched and how input is parsed.
    pub fn new(kind: DialectKind) -> Self {
        Schema {
            kind,
            fold: kind.dialect().identifier_fold(),
            tables: vec![],
            indexes: vec![],
            sequences: vec![],
            views: vec![],
        }
    }

    /// Tables in creation order.
    pub fn tables(&self) -> &[CreateTable] {
        &self.tables
    }

    pub fn indexes(&self) -> &[CreateIndex] {
        &self.indexes
    }

    pub fn sequences(&self) -> &[CreateSequence] {
        &self.sequences
    }

    pub fn views(&self) -> &[CreateView] {
        &self.views
    }

    pub fn table(&self, name: &str) -> Option<&CreateTable> {
        let k = name.rsplit('.').next().unwrap_or(name);
        let k = if self.fold == IdentifierFold::Insensitive { k.to_lowercase() } else { self.fold.fold(k) };
        self.tables.iter().find(|t| key(self.fold, last(&t.name)) == k)
    }

    fn k(&self, id: &Ident) -> String {
        key(self.fold, id)
    }

    fn find_table(&self, name: &ObjectName) -> Option<usize> {
        let k = self.k(last(name));
        self.tables.iter().position(|t| self.k(last(&t.name)) == k)
    }

    /// Parses `sql` and applies every statement. Statements that do not parse are skipped (see
    /// [`Applied::skipped`]); statements that parse but cannot apply are reported in
    /// [`Applied::errors`] and leave the schema unchanged. SQL inside `EXECUTE IMMEDIATE` string
    /// literals in anonymous PL/SQL blocks is applied as if the block ran it unconditionally. Only a lexical error (an unterminated
    /// string, say) is returned as `Err`, and nothing is applied then.
    pub fn apply_sql(&mut self, sql: &str) -> Result<Applied, ParseError> {
        let dialect = self.kind.dialect();
        let parsed = crate::parser::Parser::new(dialect.as_ref(), sql)?.parse_statements_lenient();
        let mut applied = Applied::default();
        for (at, e) in parsed.skipped {
            applied.skipped.push(SchemaError { message: e.to_string(), location: Some(at) });
        }
        for s in parsed.statements {
            if let Statement::Plsql(u) = &s.statement {
                applied.plsql.push(SchemaError {
                    message: format!("{} not analyzed", u.kind.label()),
                    location: Some(s.location.clone()),
                });
            }
            if let Err(mut e) = self.apply(&s.statement) {
                e.location = Some(s.location);
                if s.dynamic {
                    applied.dynamic_errors.push(e);
                } else {
                    applied.errors.push(e);
                }
            }
        }
        Ok(applied)
    }

    /// Applies one statement. On error the schema is left unchanged by that statement.
    pub fn apply(&mut self, stmt: &Statement) -> Result<(), SchemaError> {
        match stmt {
            Statement::CreateTable(c) => self.create_table(c),
            Statement::CreateIndex(c) => self.create_index(c),
            Statement::CreateSequence(c) => self.create_sequence(c),
            Statement::CreateView(c) => self.create_view(c),
            Statement::AlterTable(a) => self.alter_table(a),
            Statement::Drop(d) => self.drop(d),
            Statement::Query(_)
            | Statement::Insert(_)
            | Statement::Update(_)
            | Statement::Delete(_)
            | Statement::Merge(_)
            | Statement::InsertAll(_)
            | Statement::Plsql(_)
            | Statement::If(_)
            | Statement::Truncate(_)
            | Statement::Other(_) => Ok(()),
        }
    }

    fn create_table(&mut self, c: &CreateTable) -> Result<(), SchemaError> {
        if c.temporary {
            return Ok(());
        }
        if self.find_table(&c.name).is_some() {
            return if c.if_not_exists { Ok(()) } else { err(format!("table {} already exists", last(&c.name).value)) };
        }
        if let Some(q) = &c.as_query {
            let mut t = c.clone();
            t.if_not_exists = false;
            t.as_query = None;
            t.columns = self.derive_columns(q, &c.columns);
            self.tables.push(t);
            return Ok(());
        }
        let mut seen = std::collections::HashSet::new();
        for col in &c.columns {
            if !seen.insert(self.k(&col.name)) {
                return err(format!("column {} specified more than once", col.name.value));
            }
        }
        let mut t = c.clone();
        t.if_not_exists = false;
        self.tables.push(t);
        Ok(())
    }

    /// The columns `CREATE TABLE ... AS query` would produce. Names come from the column list,
    /// the select list aliases, or the column itself; types are copied from known source
    /// columns and `CAST`s, and are `UNKNOWN` for anything else (no type inference).
    fn derive_columns(&self, q: &Query, names: &[ColumnDef]) -> Vec<ColumnDef> {
        let unknown = || DataType { name: "UNKNOWN".into(), args: vec![], suffix: None, array_dims: 0 };
        let plain = |name: &Ident, ty: Option<DataType>| ColumnDef {
            name: name.clone(),
            data_type: Some(ty.unwrap_or_else(unknown)),
            options: vec![],
        };
        let mut out: Vec<ColumnDef> = vec![];
        if let Some(select) = leftmost_select(&q.body) {
            let mut sources: Vec<(String, Option<usize>)> = vec![];
            for t in &select.from {
                self.collect_sources(t, &mut sources);
            }
            for item in &select.projection {
                match item {
                    SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => {
                        let only = match item {
                            SelectItem::QualifiedWildcard(n) => Some(self.k(last(n))),
                            _ => None,
                        };
                        for (qual, ti) in &sources {
                            if only.as_ref().is_some_and(|o| o != qual) {
                                continue;
                            }
                            if let Some(ti) = ti {
                                for col in &self.tables[*ti].columns {
                                    out.push(plain(&col.name, col.data_type.clone()));
                                }
                            }
                        }
                    }
                    SelectItem::ExprWithAlias { expr, alias } => out.push(plain(alias, self.expr_type(expr, &sources))),
                    SelectItem::UnnamedExpr(expr) => {
                        let name = match expr {
                            Expr::Identifier(id) => id.clone(),
                            Expr::CompoundIdentifier(parts) if !parts.is_empty() => parts.last().unwrap().clone(),
                            _ => Ident { value: "?column?".into(), quote_style: None },
                        };
                        out.push(plain(&name, self.expr_type(expr, &sources)));
                    }
                }
            }
        }
        for (col, given) in out.iter_mut().zip(names) {
            col.name = given.name.clone();
        }
        for given in names.iter().skip(out.len()) {
            out.push(plain(&given.name, None));
        }
        out
    }

    fn collect_sources(&self, t: &TableWithJoins, out: &mut Vec<(String, Option<usize>)>) {
        let mut factors = vec![&t.relation];
        factors.extend(t.joins.iter().map(|j| &j.relation));
        for f in factors {
            match f {
                TableFactor::Table { name, alias } => {
                    let qual = alias.as_ref().map_or_else(|| self.k(last(name)), |a| self.k(&a.name));
                    out.push((qual, self.find_table(name)));
                }
                TableFactor::Derived { alias, .. } => {
                    if let Some(a) = alias {
                        out.push((self.k(&a.name), None));
                    }
                }
                TableFactor::Nested(inner) => self.collect_sources(inner, out),
            }
        }
    }

    fn expr_type(&self, e: &Expr, sources: &[(String, Option<usize>)]) -> Option<DataType> {
        let lookup = |qual: Option<String>, col: &Ident| {
            let ck = self.k(col);
            sources
                .iter()
                .filter(|(q, _)| qual.as_ref().is_none_or(|w| w == q))
                .filter_map(|(_, ti)| ti.map(|ti| &self.tables[ti]))
                .find_map(|t| t.columns.iter().find(|c| self.k(&c.name) == ck))
                .and_then(|c| c.data_type.clone())
        };
        match e {
            Expr::Identifier(id) => lookup(None, id),
            Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
                lookup(Some(self.k(&parts[parts.len() - 2])), &parts[parts.len() - 1])
            }
            Expr::Cast { data_type, .. } => Some(data_type.clone()),
            _ => None,
        }
    }

    fn create_index(&mut self, c: &CreateIndex) -> Result<(), SchemaError> {
        if self.find_table(&c.table).is_none() {
            return err(format!("table {} does not exist", last(&c.table).value));
        }
        let k = self.k(last(&c.name));
        if self.indexes.iter().any(|i| self.k(last(&i.name)) == k) {
            return if c.if_not_exists { Ok(()) } else { err(format!("index {} already exists", last(&c.name).value)) };
        }
        let mut i = c.clone();
        i.if_not_exists = false;
        self.indexes.push(i);
        Ok(())
    }

    fn create_sequence(&mut self, c: &CreateSequence) -> Result<(), SchemaError> {
        let k = self.k(last(&c.name));
        if self.sequences.iter().any(|x| self.k(last(&x.name)) == k) {
            return if c.if_not_exists {
                Ok(())
            } else {
                err(format!("sequence {} already exists", last(&c.name).value))
            };
        }
        let mut c = c.clone();
        c.if_not_exists = false;
        self.sequences.push(c);
        Ok(())
    }

    fn create_view(&mut self, c: &CreateView) -> Result<(), SchemaError> {
        let k = self.k(last(&c.name));
        match self.views.iter().position(|x| self.k(last(&x.name)) == k) {
            Some(i) if c.or_replace => self.views[i] = c.clone(),
            Some(_) => return err(format!("view {} already exists", last(&c.name).value)),
            None => self.views.push(c.clone()),
        }
        Ok(())
    }

    fn drop(&mut self, d: &Drop) -> Result<(), SchemaError> {
        for name in &d.names {
            match d.object_type {
                // Views that failed to parse are not tracked, so dropping an unknown one is fine.
                ObjectType::View => {
                    let k = self.k(last(name));
                    let fold = self.fold;
                    self.views.retain(|v| key(fold, last(&v.name)) != k);
                }
                ObjectType::Sequence => {
                    let k = self.k(last(name));
                    match self.sequences.iter().position(|x| self.k(last(&x.name)) == k) {
                        Some(i) => {
                            self.sequences.remove(i);
                        }
                        None if d.if_exists => {}
                        None => return err(format!("sequence {} does not exist", last(name).value)),
                    }
                }
                ObjectType::Table => match self.find_table(name) {
                    Some(i) => {
                        let t = self.tables.remove(i);
                        let k = self.k(last(&t.name));
                        let fold = self.fold;
                        self.indexes.retain(|ix| key(fold, last(&ix.table)) != k);
                        if d.behavior == Some(DropBehavior::Cascade) {
                            self.drop_references_to(&k);
                        }
                    }
                    None if d.if_exists => {}
                    None => return err(format!("table {} does not exist", last(name).value)),
                },
                ObjectType::Index => {
                    let k = self.k(last(name));
                    match self.indexes.iter().position(|i| self.k(last(&i.name)) == k) {
                        Some(i) => {
                            self.indexes.remove(i);
                        }
                        None if d.if_exists => {}
                        None => return err(format!("index {} does not exist", last(name).value)),
                    }
                }
            }
        }
        Ok(())
    }

    /// `DROP TABLE ... CASCADE` removes foreign keys pointing at the dropped table.
    fn drop_references_to(&mut self, table_key: &str) {
        let fold = self.fold;
        for t in &mut self.tables {
            t.constraints.retain(|c| match &c.kind {
                TableConstraintKind::ForeignKey { reference, .. } => key(fold, last(&reference.table)) != table_key,
                _ => true,
            });
            for col in &mut t.columns {
                col.options.retain(|o| match &o.option {
                    ColumnOption::References(r) => key(fold, last(&r.table)) != table_key,
                    _ => true,
                });
            }
        }
    }

    fn alter_table(&mut self, a: &AlterTable) -> Result<(), SchemaError> {
        let ti = match self.find_table(&a.name) {
            Some(i) => i,
            None if a.if_exists => return Ok(()),
            None => return err(format!("table {} does not exist", last(&a.name).value)),
        };
        // All actions apply or none do.
        let backup = (self.tables.clone(), self.indexes.clone());
        for act in &a.actions {
            if let Err(e) = self.alter_action(ti, act) {
                (self.tables, self.indexes) = backup;
                return Err(e);
            }
        }
        Ok(())
    }

    fn column_idx(&self, ti: usize, name: &Ident) -> Result<usize, SchemaError> {
        let k = self.k(name);
        match self.tables[ti].columns.iter().position(|c| self.k(&c.name) == k) {
            Some(i) => Ok(i),
            None => err(format!("column {} of table {} does not exist", name.value, last(&self.tables[ti].name).value)),
        }
    }

    fn alter_action(&mut self, ti: usize, act: &AlterAction) -> Result<(), SchemaError> {
        let fold = self.fold;
        match act {
            AlterAction::AddColumn { if_not_exists, column } => {
                if self.column_idx(ti, &column.name).is_ok() {
                    return if *if_not_exists {
                        Ok(())
                    } else {
                        err(format!("column {} already exists", column.name.value))
                    };
                }
                self.tables[ti].columns.push(column.clone());
            }
            AlterAction::DropColumn { if_exists, name, .. } => {
                let ci = match self.column_idx(ti, name) {
                    Ok(i) => i,
                    Err(_) if *if_exists => return Ok(()),
                    Err(e) => return Err(e),
                };
                let ck = self.k(name);
                self.tables[ti].columns.remove(ci);
                // Constraints and indexes that involve the column go with it.
                self.tables[ti].constraints.retain_mut(|c| match constraint_columns(&mut c.kind) {
                    Some(cols) => !cols.iter().any(|x| key(fold, x) == ck),
                    None => true,
                });
                let tk = self.k(last(&self.tables[ti].name));
                self.indexes.retain(|i| !(key(fold, last(&i.table)) == tk && index_uses(fold, i, &ck)));
            }
            AlterAction::RenameColumn { old, new } => {
                let ci = self.column_idx(ti, old)?;
                if self.column_idx(ti, new).is_ok() {
                    return err(format!("column {} already exists", new.value));
                }
                let ok = self.k(old);
                let tk = self.k(last(&self.tables[ti].name));
                self.tables[ti].columns[ci].name = new.clone();
                for c in &mut self.tables[ti].constraints {
                    if let Some(cols) = constraint_columns(&mut c.kind) {
                        for x in cols.iter_mut().filter(|x| key(fold, x) == ok) {
                            *x = new.clone();
                        }
                    }
                }
                for i in self.indexes.iter_mut().filter(|i| key(fold, last(&i.table)) == tk) {
                    for c in &mut i.columns {
                        if matches!(&c.expr, Expr::Identifier(id) if key(fold, id) == ok) {
                            c.expr = Expr::Identifier(new.clone());
                        }
                    }
                }
            }
            AlterAction::RenameTable(new) => {
                let (ok, nk) = (self.k(last(&self.tables[ti].name)), self.k(new));
                if ok != nk && self.tables.iter().any(|t| self.k(last(&t.name)) == nk) {
                    return err(format!("table {} already exists", new.value));
                }
                *self.tables[ti].name.0.last_mut().unwrap() = new.clone();
                for i in self.indexes.iter_mut().filter(|i| key(fold, last(&i.table)) == ok) {
                    *i.table.0.last_mut().unwrap() = new.clone();
                }
                let retarget = |r: &mut ForeignKeyRef| {
                    if key(fold, last(&r.table)) == ok {
                        *r.table.0.last_mut().unwrap() = new.clone();
                    }
                };
                for t in &mut self.tables {
                    for c in &mut t.constraints {
                        if let TableConstraintKind::ForeignKey { reference, .. } = &mut c.kind {
                            retarget(reference);
                        }
                    }
                    for col in &mut t.columns {
                        for o in &mut col.options {
                            if let ColumnOption::References(r) = &mut o.option {
                                retarget(r);
                            }
                        }
                    }
                }
            }
            AlterAction::AlterColumnType { name, data_type } => {
                let ci = self.column_idx(ti, name)?;
                self.tables[ti].columns[ci].data_type = Some(data_type.clone());
            }
            AlterAction::SetNotNull(name) => {
                let ci = self.column_idx(ti, name)?;
                let opts = &mut self.tables[ti].columns[ci].options;
                opts.retain(|o| o.option != ColumnOption::Null);
                if !opts.iter().any(|o| o.option == ColumnOption::NotNull) {
                    opts.push(ColumnOptionDef { name: None, option: ColumnOption::NotNull });
                }
            }
            AlterAction::DropNotNull(name) => {
                let ci = self.column_idx(ti, name)?;
                self.tables[ti].columns[ci].options.retain(|o| o.option != ColumnOption::NotNull);
            }
            AlterAction::SetDefault { name, default } => {
                let ci = self.column_idx(ti, name)?;
                let opts = &mut self.tables[ti].columns[ci].options;
                opts.retain(|o| !matches!(o.option, ColumnOption::Default(_)));
                opts.push(ColumnOptionDef { name: None, option: ColumnOption::Default(default.clone()) });
            }
            AlterAction::DropDefault(name) => {
                let ci = self.column_idx(ti, name)?;
                self.tables[ti].columns[ci].options.retain(|o| !matches!(o.option, ColumnOption::Default(_)));
            }
            AlterAction::AddConstraint(c) => {
                if let Some(n) = &c.name {
                    let nk = self.k(n);
                    if self.tables[ti].constraints.iter().any(|x| x.name.as_ref().is_some_and(|x| self.k(x) == nk)) {
                        return err(format!("constraint {} already exists", n.value));
                    }
                }
                self.tables[ti].constraints.push(c.clone());
            }
            AlterAction::Other(_) => {}
            AlterAction::DropPrimaryKey => {
                let t = &mut self.tables[ti];
                let (c0, o0) = (t.constraints.len(), t.columns.iter().map(|c| c.options.len()).sum::<usize>());
                t.constraints.retain(|c| !matches!(c.kind, TableConstraintKind::PrimaryKey(_)));
                for col in &mut t.columns {
                    col.options.retain(|o| o.option != ColumnOption::PrimaryKey);
                }
                let o1 = t.columns.iter().map(|c| c.options.len()).sum::<usize>();
                if (c0, o0) == (t.constraints.len(), o1) {
                    return err(format!("table {} has no primary key", last(&t.name).value));
                }
            }
            AlterAction::DropUnique(cols) => {
                // The column set matters, not its order.
                let sorted = |v: &[Ident]| {
                    let mut k: Vec<String> = v.iter().map(|c| key(fold, c)).collect();
                    k.sort();
                    k
                };
                let want = sorted(cols);
                let t = &mut self.tables[ti];
                let before = t.constraints.len();
                t.constraints.retain(|c| match &c.kind {
                    TableConstraintKind::Unique(have) => sorted(have) != want,
                    _ => true,
                });
                let mut found = t.constraints.len() != before;
                if let [only] = want.as_slice() {
                    for col in t.columns.iter_mut().filter(|c| &key(fold, &c.name) == only) {
                        let before = col.options.len();
                        col.options.retain(|o| o.option != ColumnOption::Unique);
                        found |= col.options.len() != before;
                    }
                }
                if !found {
                    return err(format!("table {} has no such unique constraint", last(&t.name).value));
                }
            }
            AlterAction::RenameConstraint { old, new } => {
                let ok = self.k(old);
                let t = &mut self.tables[ti];
                let mut found = false;
                for c in t.constraints.iter_mut().filter(|c| c.name.as_ref().is_some_and(|n| key(fold, n) == ok)) {
                    c.name = Some(new.clone());
                    found = true;
                }
                for o in t.columns.iter_mut().flat_map(|c| c.options.iter_mut()) {
                    if o.name.as_ref().is_some_and(|n| key(fold, n) == ok) {
                        o.name = Some(new.clone());
                        found = true;
                    }
                }
                if !found {
                    return err(format!("constraint {} of table {} does not exist", old.value, last(&t.name).value));
                }
            }
            AlterAction::DropConstraint { if_exists, name, .. } => {
                if !self.drop_constraint(ti, name) && !*if_exists {
                    return err(format!(
                        "constraint {} of table {} does not exist",
                        name.value,
                        last(&self.tables[ti].name).value
                    ));
                }
            }
        }
        Ok(())
    }

    /// Drops a constraint by its name, or by the name PostgreSQL would have generated for an
    /// unnamed one (`t_pkey`, `t_col_key`, `t_col_fkey`).
    fn drop_constraint(&mut self, ti: usize, name: &Ident) -> bool {
        let fold = self.fold;
        let nk = self.k(name);
        let tk = self.k(last(&self.tables[ti].name));
        let t = &mut self.tables[ti];
        let named = |n: &Option<Ident>| n.as_ref().is_some_and(|n| key(fold, n) == nk);
        let before = t.constraints.len();
        t.constraints.retain(|c| !named(&c.name));
        let mut found = t.constraints.len() != before;
        for col in &mut t.columns {
            let before = col.options.len();
            col.options.retain(|o| !named(&o.name));
            found |= col.options.len() != before;
        }
        if found {
            return true;
        }
        if nk == format!("{tk}_pkey") {
            let before = t.constraints.len();
            t.constraints.retain(|c| !(c.name.is_none() && matches!(c.kind, TableConstraintKind::PrimaryKey(_))));
            found |= t.constraints.len() != before;
            for col in &mut t.columns {
                let before = col.options.len();
                col.options.retain(|o| !(o.name.is_none() && o.option == ColumnOption::PrimaryKey));
                found |= col.options.len() != before;
            }
            return found;
        }
        for (suffix, unique) in [("_key", true), ("_fkey", false)] {
            let Some(col) = nk.strip_prefix(&format!("{tk}_")).and_then(|r| r.strip_suffix(suffix)) else {
                continue;
            };
            let before = t.constraints.len();
            t.constraints.retain(|c| {
                let hit = c.name.is_none()
                    && match &c.kind {
                        TableConstraintKind::Unique(cols) => unique && cols.len() == 1 && key(fold, &cols[0]) == col,
                        TableConstraintKind::ForeignKey { columns, .. } => {
                            !unique && columns.len() == 1 && key(fold, &columns[0]) == col
                        }
                        _ => false,
                    };
                !hit
            });
            found |= t.constraints.len() != before;
            for c in t.columns.iter_mut().filter(|c| key(fold, &c.name) == col) {
                let before = c.options.len();
                c.options.retain(|o| {
                    !(o.name.is_none()
                        && match &o.option {
                            ColumnOption::Unique => unique,
                            ColumnOption::References(_) => !unique,
                            _ => false,
                        })
                });
                found |= c.options.len() != before;
            }
            if found {
                return true;
            }
        }
        false
    }

    /// The schema as `CREATE SEQUENCE`, `CREATE TABLE`, `CREATE INDEX` and `CREATE VIEW` statements for `target`, one per line.
    pub fn to_sql(&self, target: DialectKind) -> Result<String, EmitError> {
        let d = target.dialect();
        let mut out = vec![];
        for q in &self.sequences {
            out.push(format!("{};", Statement::CreateSequence(q.clone()).to_sql(d.as_ref())?));
        }
        for t in &self.tables {
            out.push(format!("{};", Statement::CreateTable(t.clone()).to_sql(d.as_ref())?));
        }
        for i in &self.indexes {
            out.push(format!("{};", Statement::CreateIndex(i.clone()).to_sql(d.as_ref())?));
        }
        for v in &self.views {
            out.push(format!("{};", Statement::CreateView(v.clone()).to_sql(d.as_ref())?));
        }
        Ok(out.join("\n"))
    }
}
