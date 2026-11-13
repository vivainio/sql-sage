//! Renders the shared AST as SQL.
//!
//! * [`Emitter::neutral`] reproduces the AST faithfully in a normalized form
//!   (this is what `Display` uses; handy for debugging and tests).
//! * [`Emitter::for_dialect`] targets one engine: it rewrites what can be
//!   rewritten safely (pagination, `MINUS`, placeholders, data types, `ILIKE`,
//!   `FROM dual`, ...) and returns an [`EmitError`] for what cannot.
//!
//! Known limits: identifier case folding differs between engines and is not
//! adjusted; ambiguous types (Oracle `DATE` vs Postgres `DATE`) pass through.

use crate::ast::*;
use crate::compat::{Finding, Severity};
use crate::dialect::{Dialect, PlaceholderKind};
use std::cell::{Cell, RefCell};
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitError {
    pub message: String,
}

impl EmitError {
    fn unsupported(d: &dyn Dialect, what: &str) -> Self {
        Self { message: format!("{what} is not supported by {}", d.name()) }
    }
}

impl Display for EmitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EmitError {}

type R = Result<String, EmitError>;

pub struct Emitter<'a> {
    source: Option<&'a dyn Dialect>,
    target: Option<&'a dyn Dialect>,
    /// Analysis mode: record findings and keep going instead of failing.
    collect: bool,
    findings: RefCell<Vec<Finding>>,
    statement: Cell<usize>,
    anon_placeholders: Cell<usize>,
}

impl<'a> Emitter<'a> {
    fn build(source: Option<&'a dyn Dialect>, target: Option<&'a dyn Dialect>, collect: bool) -> Self {
        Self {
            source,
            target,
            collect,
            findings: RefCell::new(vec![]),
            statement: Cell::new(0),
            anon_placeholders: Cell::new(0),
        }
    }

    pub fn neutral() -> Self {
        Self::build(None, None, false)
    }

    pub fn for_dialect(target: &'a dyn Dialect) -> Self {
        Self::build(None, Some(target), false)
    }

    /// Collecting mode used by [`crate::compat::analyze`]: never fails, records
    /// everything that is rewritten, risky or impossible on `target`.
    pub fn for_analysis(source: &'a dyn Dialect, target: &'a dyn Dialect) -> Self {
        Self::build(Some(source), Some(target), true)
    }

    pub fn set_statement(&self, n: usize) {
        self.statement.set(n);
    }

    pub fn take_findings(&self) -> Vec<Finding> {
        std::mem::take(&mut *self.findings.borrow_mut())
    }

    fn note(&self, severity: Severity, message: impl Into<String>, at: Option<String>) {
        if self.collect {
            self.findings.borrow_mut().push(Finding {
                severity,
                statement: self.statement.get(),
                location: None,
                message: message.into(),
                at,
            });
        }
    }

    /// Source and target are both known and disagree on `f`.
    fn differs(&self, f: fn(&dyn Dialect) -> bool) -> bool {
        matches!((self.source, self.target), (Some(s), Some(t)) if f(s) != f(t))
    }

    /// `(source, target)` names, for messages about a difference between them.
    fn names(&self) -> (&'static str, &'static str) {
        (self.source.map_or("source", |d| d.name()), self.target.map_or("target", |d| d.name()))
    }

    fn has(&self, f: fn(&dyn Dialect) -> bool) -> bool {
        self.target.is_none_or(f)
    }

    /// Something the target cannot express: an error when emitting, a finding
    /// when analysing.
    fn fail(&self, message: String, at: Option<String>) -> Result<(), EmitError> {
        if self.collect {
            self.note(Severity::Incompatible, message, at);
            Ok(())
        } else {
            Err(EmitError { message })
        }
    }

    fn need(&self, f: fn(&dyn Dialect) -> bool, what: &str) -> Result<(), EmitError> {
        self.need_at(f, what, || None)
    }

    fn need_at(
        &self,
        f: fn(&dyn Dialect) -> bool,
        what: &str,
        at: impl FnOnce() -> Option<String>,
    ) -> Result<(), EmitError> {
        match self.target {
            Some(d) if !f(d) => {
                let at = if self.collect { at() } else { None };
                self.fail(EmitError::unsupported(d, what).message, at)
            }
            _ => Ok(()),
        }
    }

    fn list<T>(&self, items: &[T], f: impl Fn(&Self, &T) -> R) -> R {
        let parts = items.iter().map(|i| f(self, i)).collect::<Result<Vec<_>, _>>()?;
        Ok(parts.join(", "))
    }

    // ----------------------------------------------------------- names

    pub fn ident(&self, i: &Ident) -> String {
        if let (Some(_), Some(s), Some(t)) = (i.quote_style, self.source, self.target) {
            if s.identifier_fold() != t.identifier_fold() && t.identifier_fold().fold(&i.value) != i.value {
                self.note(
                    Severity::Warning,
                    format!("quoted identifier is case-sensitive, but unquoted names fold differently in {}", t.name()),
                    Some(format!("\"{}\"", i.value)),
                );
            }
        }
        match (i.quote_style, self.target) {
            (None, _) => i.value.clone(),
            (Some('['), None) => format!("[{}]", i.value),
            (Some(q), None) => format!("{q}{}{q}", i.value.replace(q, &format!("{q}{q}"))),
            (Some(_), Some(d)) => {
                let q = d.identifier_quote();
                format!("{q}{}{q}", i.value.replace(q, &format!("{q}{q}")))
            }
        }
    }

    pub fn object_name(&self, n: &ObjectName) -> String {
        self.idents(&n.0, ".")
    }

    fn idents(&self, v: &[Ident], sep: &str) -> String {
        v.iter().map(|i| self.ident(i)).collect::<Vec<_>>().join(sep)
    }

    fn paren_idents(&self, v: &[Ident]) -> String {
        format!("({})", self.idents(v, ", "))
    }

    fn render_type(dt: &DataType) -> String {
        let mut s = dt.name.clone();
        if !dt.args.is_empty() {
            s.push_str(&format!("({})", dt.args.join(", ")));
        }
        if let Some(x) = &dt.suffix {
            s.push(' ');
            s.push_str(x);
        }
        for _ in 0..dt.array_dims {
            s.push_str("[]");
        }
        s
    }

    pub fn data_type(&self, dt: &DataType) -> R {
        let mapped = match self.target {
            Some(d) => match d.map_data_type(dt) {
                Ok(m) => m,
                Err(m) => {
                    self.fail(format!("type {}: {m} ({})", dt.name, d.name()), Some(Self::render_type(dt)))?;
                    dt.clone()
                }
            },
            None => dt.clone(),
        };
        if mapped != *dt {
            self.note(
                Severity::Rewritten,
                format!("type {} -> {}", Self::render_type(dt), Self::render_type(&mapped)),
                None,
            );
        }
        Ok(Self::render_type(&mapped))
    }

    // ------------------------------------------------------ statements

    pub fn statement(&self, s: &Statement) -> R {
        match s {
            Statement::Query(q) => self.query(q),
            Statement::Insert(i) => self.insert(i),
            Statement::Update(u) => self.update(u),
            Statement::Delete(d) => self.delete(d),
            Statement::Merge(m) => self.merge(m),
            Statement::CreateTable(c) => self.create_table(c),
            Statement::CreateIndex(c) => self.create_index(c),
            Statement::CreateSequence(c) => self.create_sequence(c),
            Statement::CreateView(c) => self.create_view(c),
            Statement::AlterTable(a) => self.alter_table(a),
            Statement::Truncate(t) => Ok(format!(
                "TRUNCATE TABLE {}",
                t.tables.iter().map(|n| self.object_name(n)).collect::<Vec<_>>().join(", ")
            )),
            Statement::Other(o) => {
                if let (Some(s), Some(t)) = (self.source, self.target) {
                    if s.name() != t.name() {
                        self.fail(format!("{} statements cannot be translated", o.kind), None)?;
                    }
                }
                Ok(o.text.clone())
            }
            Statement::Drop(d) => self.drop_stmt(d),
        }
    }

    fn returning(&self, r: &[SelectItem]) -> R {
        if r.is_empty() {
            return Ok(String::new());
        }
        self.need(|d| d.supports_returning(), "RETURNING")?;
        Ok(format!(" RETURNING {}", self.list(r, Self::select_item)?))
    }

    fn insert(&self, i: &Insert) -> R {
        let mut extra_conflict_ignore = false;
        let mut out = String::from("INSERT ");
        if let Some(or) = &i.or {
            if self.has(|d| d.supports_insert_or()) {
                out.push_str(&format!("OR {or} "));
            } else if or == "IGNORE" && i.on_conflict.is_none() && self.has(|d| d.supports_on_conflict()) {
                extra_conflict_ignore = true;
                self.note(Severity::Rewritten, "INSERT OR IGNORE -> ON CONFLICT DO NOTHING", None);
            } else {
                self.need(|d| d.supports_insert_or(), &format!("INSERT OR {or}"))?;
            }
        }
        out.push_str(&format!("INTO {}", self.object_name(&i.table)));
        if !i.columns.is_empty() {
            out.push(' ');
            out.push_str(&self.paren_idents(&i.columns));
        }
        match &i.source {
            InsertSource::Query(q) => out.push_str(&format!(" {}", self.query(q)?)),
            InsertSource::DefaultValues => out.push_str(" DEFAULT VALUES"),
        }
        if extra_conflict_ignore {
            out.push_str(" ON CONFLICT DO NOTHING");
        }
        if let Some(oc) = &i.on_conflict {
            self.need(|d| d.supports_on_conflict(), "ON CONFLICT")?;
            out.push_str(" ON CONFLICT");
            if !oc.target.is_empty() {
                out.push_str(&format!(" {}", self.paren_idents(&oc.target)));
            }
            match &oc.action {
                OnConflictAction::DoNothing => out.push_str(" DO NOTHING"),
                OnConflictAction::DoUpdate { assignments, selection } => {
                    out.push_str(&format!(" DO UPDATE SET {}", self.list(assignments, Self::assignment)?));
                    if let Some(s) = selection {
                        out.push_str(&format!(" WHERE {}", self.expr(s)?));
                    }
                }
            }
        }
        out.push_str(&self.returning(&i.returning)?);
        Ok(out)
    }

    fn assignment(&self, a: &Assignment) -> R {
        let cols: Vec<String> = a.columns.iter().map(|c| self.object_name(c)).collect();
        let cols = if cols.len() == 1 { cols[0].clone() } else { format!("({})", cols.join(", ")) };
        Ok(format!("{cols} = {}", self.expr(&a.value)?))
    }

    fn update(&self, u: &Update) -> R {
        let mut out =
            format!("UPDATE {} SET {}", self.table_factor(&u.table)?, self.list(&u.assignments, Self::assignment)?);
        if !u.from.is_empty() {
            self.need(|d| d.supports_update_from(), "UPDATE ... FROM")?;
            out.push_str(&format!(" FROM {}", self.list(&u.from, Self::table_with_joins)?));
        }
        if let Some(s) = &u.selection {
            out.push_str(&format!(" WHERE {}", self.expr(s)?));
        }
        out.push_str(&self.returning(&u.returning)?);
        Ok(out)
    }

    fn merge(&self, m: &Merge) -> R {
        self.need(|d| d.name() != "sqlite", "MERGE")?;
        let mut out = format!(
            "MERGE INTO {} USING {} ON {}",
            self.table_factor(&m.target)?,
            self.table_factor(&m.source)?,
            self.expr(&m.on)?
        );
        for c in &m.clauses {
            match c {
                MergeClause::Matched { condition, action } => {
                    out.push_str(" WHEN MATCHED");
                    if let Some(c) = condition {
                        out.push_str(&format!(" AND {}", self.expr(c)?));
                    }
                    out.push_str(" THEN ");
                    match action {
                        MergeAction::Update { assignments, selection, delete_where } => {
                            out.push_str(&format!("UPDATE SET {}", self.list(assignments, Self::assignment)?));
                            if let Some(w) = selection {
                                out.push_str(&format!(" WHERE {}", self.expr(w)?));
                            }
                            if let Some(w) = delete_where {
                                out.push_str(&format!(" DELETE WHERE {}", self.expr(w)?));
                            }
                        }
                        MergeAction::Delete => out.push_str("DELETE"),
                        MergeAction::DoNothing => out.push_str("DO NOTHING"),
                    }
                }
                MergeClause::NotMatched { condition, columns, values, selection } => {
                    out.push_str(" WHEN NOT MATCHED");
                    if let Some(c) = condition {
                        out.push_str(&format!(" AND {}", self.expr(c)?));
                    }
                    out.push_str(" THEN INSERT");
                    if !columns.is_empty() {
                        out.push_str(&format!(" {}", self.paren_idents(columns)));
                    }
                    out.push_str(&format!(" VALUES ({})", self.list(values, Self::expr)?));
                    if let Some(w) = selection {
                        out.push_str(&format!(" WHERE {}", self.expr(w)?));
                    }
                }
            }
        }
        Ok(out)
    }

    fn delete(&self, d: &Delete) -> R {
        let mut out = format!("DELETE FROM {}", self.table_factor(&d.table)?);
        if let Some(s) = &d.selection {
            out.push_str(&format!(" WHERE {}", self.expr(s)?));
        }
        out.push_str(&self.returning(&d.returning)?);
        Ok(out)
    }

    fn create_table(&self, c: &CreateTable) -> R {
        let mut out = String::from("CREATE ");
        if c.temporary {
            let kw = self.target.map_or("TEMPORARY", |d| d.temporary_table_keyword());
            if kw != "TEMPORARY" {
                self.note(Severity::Rewritten, format!("TEMPORARY TABLE -> {kw} TABLE"), None);
            }
            out.push_str(kw);
            out.push(' ');
        }
        out.push_str("TABLE ");
        if c.if_not_exists {
            self.need(|d| d.supports_if_exists(), "IF NOT EXISTS")?;
            out.push_str("IF NOT EXISTS ");
        }
        out.push_str(&self.object_name(&c.name));
        if let Some(q) = &c.as_query {
            if !c.columns.is_empty() {
                let names: Vec<_> = c.columns.iter().map(|c| self.ident(&c.name)).collect();
                out.push_str(&format!(" ({})", names.join(", ")));
            }
            out.push_str(&format!(" AS {}", self.query(q)?));
            return Ok(out);
        }
        let mut items = vec![];
        for col in &c.columns {
            items.push(self.column_def(col)?);
        }
        for con in &c.constraints {
            items.push(self.table_constraint(con)?);
        }
        out.push_str(&format!(" ({})", items.join(", ")));
        Ok(out)
    }

    fn column_def(&self, c: &ColumnDef) -> R {
        let mut out = self.ident(&c.name);
        match &c.data_type {
            Some(t) => out.push_str(&format!(" {}", self.data_type(t)?)),
            None => self.need(|d| d.supports_untyped_columns(), &format!("column {} without a type", c.name.value))?,
        }
        let mut opts: Vec<&ColumnOptionDef> = c.options.iter().collect();
        if self.target.is_some() {
            // Oracle requires DEFAULT before inline constraints; valid everywhere.
            opts.sort_by_key(|o| !matches!(o.option, ColumnOption::Default(_)));
        }
        for o in opts {
            out.push(' ');
            if let Some(n) = &o.name {
                out.push_str(&format!("CONSTRAINT {} ", self.ident(n)));
            }
            out.push_str(&self.column_option(&o.option)?);
        }
        Ok(out)
    }

    fn column_option(&self, o: &ColumnOption) -> R {
        Ok(match o {
            ColumnOption::Null => "NULL".into(),
            ColumnOption::NotNull => "NOT NULL".into(),
            ColumnOption::Default(e) => format!("DEFAULT {}", self.expr(e)?),
            ColumnOption::PrimaryKey => "PRIMARY KEY".into(),
            ColumnOption::Unique => "UNIQUE".into(),
            ColumnOption::Autoincrement => {
                self.need(|d| d.supports_autoincrement(), "AUTOINCREMENT")?;
                "AUTOINCREMENT".into()
            }
            ColumnOption::Identity { always } => {
                self.need(|d| d.supports_identity_columns(), "identity columns")?;
                format!("GENERATED {} AS IDENTITY", if *always { "ALWAYS" } else { "BY DEFAULT" })
            }
            ColumnOption::Generated { expr, stored } => {
                let pg = self.target.is_some_and(|d| d.name() == "postgres");
                let stored = *stored || pg;
                if stored {
                    self.need(|d| d.name() != "oracle", "stored generated columns")?;
                }
                format!("GENERATED ALWAYS AS ({}){}", self.expr(expr)?, if stored { " STORED" } else { "" })
            }
            ColumnOption::References(r) => format!("REFERENCES {}", self.foreign_key_ref(r)),
            ColumnOption::Check(e) => format!("CHECK ({})", self.expr(e)?),
            ColumnOption::Collate(c) => format!("COLLATE {}", self.ident(c)),
        })
    }

    fn foreign_key_ref(&self, r: &ForeignKeyRef) -> String {
        let mut s = self.object_name(&r.table);
        if !r.columns.is_empty() {
            s.push(' ');
            s.push_str(&self.paren_idents(&r.columns));
        }
        if let Some(a) = &r.on_delete {
            s.push_str(&format!(" ON DELETE {a}"));
        }
        if let Some(a) = &r.on_update {
            s.push_str(&format!(" ON UPDATE {a}"));
        }
        s
    }

    fn table_constraint(&self, c: &TableConstraint) -> R {
        let mut out = String::new();
        if let Some(n) = &c.name {
            out.push_str(&format!("CONSTRAINT {} ", self.ident(n)));
        }
        out.push_str(&match &c.kind {
            TableConstraintKind::PrimaryKey(cols) => format!("PRIMARY KEY {}", self.paren_idents(cols)),
            TableConstraintKind::Unique(cols) => format!("UNIQUE {}", self.paren_idents(cols)),
            TableConstraintKind::ForeignKey { columns, reference } => {
                format!("FOREIGN KEY {} REFERENCES {}", self.paren_idents(columns), self.foreign_key_ref(reference))
            }
            TableConstraintKind::Check(e) => format!("CHECK ({})", self.expr(e)?),
        });
        Ok(out)
    }

    fn create_index(&self, c: &CreateIndex) -> R {
        let mut out = String::from("CREATE ");
        if c.unique {
            out.push_str("UNIQUE ");
        }
        out.push_str("INDEX ");
        if c.if_not_exists {
            self.need(|d| d.supports_if_exists(), "IF NOT EXISTS")?;
            out.push_str("IF NOT EXISTS ");
        }
        out.push_str(&format!(
            "{} ON {} ({})",
            self.object_name(&c.name),
            self.object_name(&c.table),
            self.list(&c.columns, Self::order_by_expr)?
        ));
        if let Some(s) = &c.selection {
            self.need(|d| d.supports_partial_index(), "partial indexes")?;
            out.push_str(&format!(" WHERE {}", self.expr(s)?));
        }
        Ok(out)
    }

    fn create_sequence(&self, c: &CreateSequence) -> R {
        self.need(|d| d.name() != "sqlite", "sequences")?;
        let pg = self.target.is_some_and(|d| d.name() == "postgres");
        let mut out = String::from("CREATE SEQUENCE ");
        if c.if_not_exists {
            self.need(|d| d.name() == "postgres", "CREATE SEQUENCE IF NOT EXISTS")?;
            out.push_str("IF NOT EXISTS ");
        }
        out.push_str(&self.object_name(&c.name));
        for o in &c.options {
            if matches!(o, SequenceOption::Order(_)) {
                self.need(|d| d.name() != "postgres", "sequence ORDER")?;
            }
            let text = o.text();
            // PostgreSQL spells the negated forms with a space and has no NOCACHE/ORDER.
            let text = match o {
                SequenceOption::MinValue(None) | SequenceOption::MaxValue(None) | SequenceOption::Cycle(false)
                    if pg =>
                {
                    text.replacen("NO", "NO ", 1)
                }
                SequenceOption::Cache(None) if pg => "CACHE 1".into(),
                _ => text,
            };
            out.push(' ');
            out.push_str(&text);
        }
        Ok(out)
    }

    fn create_view(&self, c: &CreateView) -> R {
        let mut out = String::from("CREATE ");
        if c.or_replace {
            self.need(|d| d.name() != "sqlite", "CREATE OR REPLACE VIEW")?;
            out.push_str("OR REPLACE ");
        }
        if c.force {
            self.need(|d| d.name() == "oracle", "CREATE FORCE VIEW")?;
            out.push_str("FORCE ");
        }
        out.push_str("VIEW ");
        out.push_str(&self.object_name(&c.name));
        if !c.columns.is_empty() {
            out.push_str(&format!(" {}", self.paren_idents(&c.columns)));
        }
        out.push_str(&format!(" AS {}", self.query(&c.query)?));
        Ok(out)
    }

    fn alter_table(&self, a: &AlterTable) -> R {
        let oracle = self.target.is_some_and(|d| d.name() == "oracle");
        let mut out = String::from("ALTER TABLE ");
        if a.if_exists {
            self.need(|d| d.name() == "postgres", "ALTER TABLE IF EXISTS")?;
            out.push_str("IF EXISTS ");
        }
        out.push_str(&self.object_name(&a.name));
        let behavior = |b: &Option<DropBehavior>| match b {
            Some(DropBehavior::Cascade) => " CASCADE",
            Some(DropBehavior::Restrict) => " RESTRICT",
            None => "",
        };
        let mut parts = vec![];
        for act in &a.actions {
            let only_pg = |what: &str| self.need(|d| d.name() != "sqlite", what);
            parts.push(match act {
                AlterAction::AddColumn { if_not_exists, column } => {
                    let mut p = String::from(if oracle { "ADD " } else { "ADD COLUMN " });
                    if *if_not_exists {
                        self.need(|d| d.name() == "postgres", "ADD COLUMN IF NOT EXISTS")?;
                        p.push_str("IF NOT EXISTS ");
                    }
                    p.push_str(&self.column_def(column)?);
                    p
                }
                AlterAction::DropColumn { if_exists, name, behavior: b } => {
                    let mut p = String::from("DROP COLUMN ");
                    if *if_exists {
                        self.need(|d| d.name() == "postgres", "DROP COLUMN IF EXISTS")?;
                        p.push_str("IF EXISTS ");
                    }
                    p.push_str(&self.ident(name));
                    if !oracle {
                        p.push_str(behavior(b));
                    }
                    p
                }
                AlterAction::RenameColumn { old, new } => {
                    format!("RENAME COLUMN {} TO {}", self.ident(old), self.ident(new))
                }
                AlterAction::RenameTable(n) => format!("RENAME TO {}", self.ident(n)),
                AlterAction::AlterColumnType { name, data_type } => {
                    only_pg("ALTER COLUMN TYPE")?;
                    let t = self.data_type(data_type)?;
                    if oracle {
                        format!("MODIFY {} {t}", self.ident(name))
                    } else {
                        format!("ALTER COLUMN {} TYPE {t}", self.ident(name))
                    }
                }
                AlterAction::SetNotNull(n) => {
                    only_pg("ALTER COLUMN SET NOT NULL")?;
                    if oracle {
                        format!("MODIFY {} NOT NULL", self.ident(n))
                    } else {
                        format!("ALTER COLUMN {} SET NOT NULL", self.ident(n))
                    }
                }
                AlterAction::DropNotNull(n) => {
                    only_pg("ALTER COLUMN DROP NOT NULL")?;
                    if oracle {
                        format!("MODIFY {} NULL", self.ident(n))
                    } else {
                        format!("ALTER COLUMN {} DROP NOT NULL", self.ident(n))
                    }
                }
                AlterAction::SetDefault { name, default } => {
                    only_pg("ALTER COLUMN SET DEFAULT")?;
                    if oracle {
                        format!("MODIFY {} DEFAULT {}", self.ident(name), self.expr(default)?)
                    } else {
                        format!("ALTER COLUMN {} SET DEFAULT {}", self.ident(name), self.expr(default)?)
                    }
                }
                AlterAction::DropDefault(n) => {
                    only_pg("ALTER COLUMN DROP DEFAULT")?;
                    if oracle {
                        format!("MODIFY {} DEFAULT NULL", self.ident(n))
                    } else {
                        format!("ALTER COLUMN {} DROP DEFAULT", self.ident(n))
                    }
                }
                AlterAction::AddConstraint(c) => {
                    only_pg("ADD CONSTRAINT")?;
                    format!("ADD {}", self.table_constraint(c)?)
                }
                AlterAction::DropPrimaryKey => "DROP PRIMARY KEY".to_string(),
                AlterAction::DropUnique(cols) => format!("DROP UNIQUE {}", self.paren_idents(cols)),
                AlterAction::RenameConstraint { old, new } => {
                    format!("RENAME CONSTRAINT {} TO {}", self.ident(old), self.ident(new))
                }
                AlterAction::Other(text) => text.clone(),
                AlterAction::DropConstraint { if_exists, name, behavior: b } => {
                    only_pg("DROP CONSTRAINT")?;
                    let mut p = String::from("DROP CONSTRAINT ");
                    if *if_exists {
                        self.need(|d| d.name() == "postgres", "DROP CONSTRAINT IF EXISTS")?;
                        p.push_str("IF EXISTS ");
                    }
                    p.push_str(&self.ident(name));
                    if !oracle {
                        p.push_str(behavior(b));
                    }
                    p
                }
            });
        }
        out.push(' ');
        out.push_str(&parts.join(", "));
        Ok(out)
    }

    fn drop_stmt(&self, d: &Drop) -> R {
        let t = match d.object_type {
            ObjectType::Table => "TABLE",
            ObjectType::View => "VIEW",
            ObjectType::Index => "INDEX",
            ObjectType::Sequence => {
                self.need(|d| d.name() != "sqlite", "sequences")?;
                "SEQUENCE"
            }
        };
        let mut out = format!("DROP {t} ");
        if d.if_exists {
            self.need(|d| d.supports_if_exists(), "IF EXISTS")?;
            out.push_str("IF EXISTS ");
        }
        out.push_str(&d.names.iter().map(|n| self.object_name(n)).collect::<Vec<_>>().join(", "));
        match d.behavior {
            Some(DropBehavior::Cascade) => out.push_str(" CASCADE"),
            Some(DropBehavior::Restrict) => out.push_str(" RESTRICT"),
            None => {}
        }
        Ok(out)
    }

    // --------------------------------------------------------- queries

    pub fn query(&self, q: &Query) -> R {
        if let Some(limited) = self.rownum_as_limit(q) {
            return self.query(&limited);
        }
        let mut out = String::new();
        if let Some(w) = &q.with {
            out.push_str(&self.with(w)?);
            out.push(' ');
        }
        out.push_str(&self.set_expr(&q.body)?);
        if !q.order_by.is_empty() {
            out.push_str(&format!(" ORDER BY {}", self.list(&q.order_by, Self::order_by_expr)?));
        }
        out.push_str(&self.pagination(q)?);
        Ok(out)
    }

    /// Oracle's `WHERE ... AND ROWNUM <= n` as a `LIMIT n`, for a target that has one: the query without the ROWNUM
    /// predicate and with the limit, or `None` when there is nothing to rewrite. Only the `AND`ed predicates of the
    /// query's own WHERE count (a ROWNUM in a subquery is that subquery's: it is rewritten when the subquery is
    /// emitted). Oracle numbers the rows before `ORDER BY` and a LIMIT cuts them after it, so a sorted query can
    /// pick other rows: that is reported as a warning.
    fn rownum_as_limit(&self, q: &Query) -> Option<Query> {
        let d = self.target?;
        if !d.supports_limit() || q.limit.is_some() || q.fetch.is_some() {
            return None;
        }
        let SetExpr::Select(select) = &q.body else { return None };
        let mut conjuncts = Vec::new();
        flatten_and(select.selection.as_ref()?, &mut conjuncts);
        let (at, n) = conjuncts.iter().enumerate().find_map(|(i, c)| rownum_limit(c).map(|n| (i, n)))?;
        let rest: Vec<Expr> =
            conjuncts.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, c)| (*c).clone()).collect();
        let selection = rest.into_iter().reduce(|l, r| Expr::BinaryOp {
            left: Box::new(l),
            op: BinaryOperator::And,
            right: Box::new(r),
        });
        let mut limited = q.clone();
        if let SetExpr::Select(s) = &mut limited.body {
            s.selection = selection;
        }
        limited.limit = Some(Expr::Value(Value::Number(n.to_string())));
        self.note(Severity::Rewritten, format!("ROWNUM predicate rewritten to LIMIT {n}"), None);
        if !q.order_by.is_empty() {
            self.note(
                Severity::Warning,
                "Oracle numbers the rows before ORDER BY, LIMIT cuts them after it: a sorted ROWNUM query can return other rows",
                None,
            );
        }
        Some(limited)
    }

    fn fetch_clause(&self, f: &Fetch) -> R {
        let mut s = String::from("FETCH FIRST");
        if let Some(q) = &f.quantity {
            s.push_str(&format!(" {}", self.expr(q)?));
        }
        if f.percent {
            s.push_str(" PERCENT");
        }
        s.push_str(if f.with_ties { " ROWS WITH TIES" } else { " ROWS ONLY" });
        Ok(s)
    }

    fn pagination(&self, q: &Query) -> R {
        let mut out = String::new();
        let Some(d) = self.target else {
            if let Some(l) = &q.limit {
                out.push_str(&format!(" LIMIT {}", self.expr(l)?));
            }
            if let Some(o) = &q.offset {
                out.push_str(&format!(" OFFSET {}", self.expr(o)?));
            }
            if let Some(f) = &q.fetch {
                out.push_str(&format!(" {}", self.fetch_clause(f)?));
            }
            return Ok(out);
        };
        if q.limit.is_some() && q.fetch.is_some() {
            self.fail("query has both LIMIT and FETCH FIRST".into(), None)?;
        }
        let mut limit = q.limit.as_ref().map(|l| self.expr(l)).transpose()?;
        let mut fetch: Option<String> = None;
        if let Some(f) = &q.fetch {
            let simple = !f.percent && !f.with_ties;
            if simple && d.supports_limit() {
                limit = Some(match &f.quantity {
                    Some(e) => self.expr(e)?,
                    None => "1".into(),
                });
            } else if d.supports_fetch_first() {
                fetch = Some(self.fetch_clause(f)?);
            } else {
                self.fail(EmitError::unsupported(d, "FETCH FIRST ... PERCENT / WITH TIES").message, None)?;
                fetch = Some(self.fetch_clause(f)?);
            }
        }
        if !d.supports_limit() {
            if let Some(l) = limit.take() {
                if d.supports_fetch_first() {
                    fetch = Some(format!("FETCH FIRST {l} ROWS ONLY"));
                } else {
                    self.fail(EmitError::unsupported(d, "LIMIT").message, None)?;
                    limit = Some(l);
                }
            }
        }
        let rewritten = (q.limit.is_some() && !d.supports_limit())
            || (q.fetch.is_some() && d.supports_limit() && limit.is_some())
            || (q.offset.is_some() && d.offset_requires_rows());
        let offset = q.offset.as_ref().map(|o| self.expr(o)).transpose()?;
        let mut rewritten = rewritten;
        if offset.is_some() && limit.is_none() && fetch.is_none() && d.requires_limit_for_offset() {
            limit = Some("-1".into());
            rewritten = true;
        }
        if rewritten {
            self.note(
                Severity::Rewritten,
                format!("row limiting rewritten to {}'s LIMIT / OFFSET / FETCH FIRST form", d.name()),
                None,
            );
        }
        if let Some(l) = limit {
            out.push_str(&format!(" LIMIT {l}"));
        }
        if let Some(o) = offset {
            out.push_str(&format!(" OFFSET {o}"));
            if d.offset_requires_rows() {
                out.push_str(" ROWS");
            }
        }
        if let Some(f) = fetch {
            out.push_str(&format!(" {f}"));
        }
        Ok(out)
    }

    fn with(&self, w: &With) -> R {
        let rec = if w.recursive && self.has(|d| d.supports_recursive_keyword()) { "RECURSIVE " } else { "" };
        if w.recursive && rec.is_empty() {
            self.note(Severity::Rewritten, "WITH RECURSIVE -> WITH (recursion is implicit)", None);
        }
        Ok(format!("WITH {rec}{}", self.list(&w.ctes, Self::cte)?))
    }

    fn cte(&self, c: &Cte) -> R {
        let mut s = self.ident(&c.name);
        if !c.columns.is_empty() {
            s.push(' ');
            s.push_str(&self.paren_idents(&c.columns));
        }
        Ok(format!("{s} AS ({})", self.query(&c.query)?))
    }

    pub fn set_expr(&self, e: &SetExpr) -> R {
        Ok(match e {
            SetExpr::Select(s) => self.select(s)?,
            SetExpr::Query(q) => format!("({})", self.query(q)?),
            SetExpr::Values(v) => {
                if v.rows.len() > 1 {
                    self.need(|d| d.supports_multirow_values(), "multi-row VALUES")?;
                }
                let rows = v
                    .rows
                    .iter()
                    .map(|r| Ok(format!("({})", self.list(r, Self::expr)?)))
                    .collect::<Result<Vec<_>, EmitError>>()?;
                format!("VALUES {}", rows.join(", "))
            }
            SetExpr::SetOperation { op, quantifier, left, right } => {
                let op = match op {
                    SetOperator::Union => "UNION",
                    SetOperator::Intersect => "INTERSECT",
                    SetOperator::Except if self.has(|d| !d.supports_minus_operator()) => "EXCEPT",
                    SetOperator::Except => "MINUS",
                };
                if matches!(op, "EXCEPT" | "MINUS") && self.differs(|d| d.supports_minus_operator()) {
                    self.note(Severity::Rewritten, format!("set difference spelled {op} on the target"), None);
                }
                let q = match (quantifier, self.target) {
                    (SetQuantifier::All, _) => " ALL",
                    (SetQuantifier::Distinct, None) => " DISTINCT",
                    _ => "", // DISTINCT is the default; Oracle rejects the keyword
                };
                format!("{} {op}{q} {}", self.set_expr(left)?, self.set_expr(right)?)
            }
        })
    }

    fn is_dual(from: &[TableWithJoins]) -> bool {
        matches!(
            from,
            [TableWithJoins { relation: TableFactor::Table { name, alias: None }, joins }]
                if joins.is_empty() && name.0.len() == 1 && name.0[0].value.eq_ignore_ascii_case("dual")
        )
    }

    fn select(&self, s: &Select) -> R {
        let mut out = String::from("SELECT");
        match &s.distinct {
            Some(Distinct::Distinct) => out.push_str(" DISTINCT"),
            Some(Distinct::On(e)) => {
                self.need(|d| d.supports_distinct_on(), "DISTINCT ON")?;
                out.push_str(&format!(" DISTINCT ON ({})", self.list(e, Self::expr)?));
            }
            None => {}
        }
        out.push_str(&format!(" {}", self.list(&s.projection, Self::select_item)?));
        let from = match self.target {
            Some(d) if s.from.is_empty() && d.requires_from_clause() => {
                self.note(Severity::Rewritten, "SELECT without FROM -> FROM DUAL", None);
                Some("DUAL".to_string())
            }
            Some(d) if Self::is_dual(&s.from) && !d.requires_from_clause() => {
                self.note(Severity::Rewritten, "FROM dual removed", None);
                None
            }
            _ if s.from.is_empty() => None,
            _ => Some(self.list(&s.from, Self::table_with_joins)?),
        };
        if let Some(f) = from {
            out.push_str(&format!(" FROM {f}"));
        }
        if let Some(w) = &s.selection {
            out.push_str(&format!(" WHERE {}", self.expr(w)?));
        }
        if let Some(h) = &s.hierarchy {
            self.need(|d| d.supports_connect_by(), "CONNECT BY")?;
            if let Some(st) = &h.start_with {
                out.push_str(&format!(" START WITH {}", self.expr(st)?));
            }
            out.push_str(&format!(
                " CONNECT BY {}{}",
                if h.nocycle { "NOCYCLE " } else { "" },
                self.expr(&h.connect_by)?
            ));
        }
        if !s.group_by.is_empty() {
            out.push_str(&format!(" GROUP BY {}", self.list(&s.group_by, Self::expr)?));
        }
        if let Some(h) = &s.having {
            out.push_str(&format!(" HAVING {}", self.expr(h)?));
        }
        Ok(out)
    }

    fn select_item(&self, i: &SelectItem) -> R {
        Ok(match i {
            SelectItem::UnnamedExpr(e) => self.expr(e)?,
            SelectItem::ExprWithAlias { expr, alias } => format!("{} AS {}", self.expr(expr)?, self.ident(alias)),
            SelectItem::QualifiedWildcard(n) => format!("{}.*", self.object_name(n)),
            SelectItem::Wildcard => "*".into(),
        })
    }

    fn table_with_joins(&self, t: &TableWithJoins) -> R {
        let mut out = self.table_factor(&t.relation)?;
        for j in &t.joins {
            out.push(' ');
            out.push_str(&self.join(j)?);
        }
        Ok(out)
    }

    fn table_alias(&self, a: &Option<TableAlias>) -> String {
        let Some(a) = a else { return String::new() };
        let mut s = String::from(if self.has(|d| d.supports_table_alias_as()) { " AS " } else { " " });
        s.push_str(&self.ident(&a.name));
        if !a.columns.is_empty() {
            s.push(' ');
            s.push_str(&self.paren_idents(&a.columns));
        }
        s
    }

    fn table_factor(&self, t: &TableFactor) -> R {
        Ok(match t {
            TableFactor::Table { name, alias } => format!("{}{}", self.object_name(name), self.table_alias(alias)),
            TableFactor::Derived { subquery, alias } => {
                format!("({}){}", self.query(subquery)?, self.table_alias(alias))
            }
            TableFactor::Nested(t) => format!("({})", self.table_with_joins(t)?),
        })
    }

    fn join(&self, j: &Join) -> R {
        let natural = if j.constraint == JoinConstraint::Natural { "NATURAL " } else { "" };
        let kind = match j.kind {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Right => "RIGHT JOIN",
            JoinKind::Full => "FULL JOIN",
            JoinKind::Cross => "CROSS JOIN",
        };
        let mut out = format!("{natural}{kind} {}", self.table_factor(&j.relation)?);
        match &j.constraint {
            JoinConstraint::On(e) => out.push_str(&format!(" ON {}", self.expr(e)?)),
            JoinConstraint::Using(c) => out.push_str(&format!(" USING {}", self.paren_idents(c))),
            JoinConstraint::Natural | JoinConstraint::None => {}
        }
        Ok(out)
    }

    pub fn order_by_expr(&self, o: &OrderByExpr) -> R {
        let mut s = self.expr(&o.expr)?;
        match o.asc {
            Some(true) => s.push_str(" ASC"),
            Some(false) => s.push_str(" DESC"),
            None => {}
        }
        match o.nulls_first {
            Some(true) => s.push_str(" NULLS FIRST"),
            Some(false) => s.push_str(" NULLS LAST"),
            None => {}
        }
        Ok(s)
    }

    // ----------------------------------------------------- expressions

    pub fn value(&self, v: &Value) -> String {
        match v {
            Value::Number(n) => n.clone(),
            Value::String(s) => {
                if s.is_empty() && self.differs(|d| d.empty_string_is_null()) {
                    let (src, tgt) = self.names();
                    let src_null = self.source.is_some_and(|d| d.empty_string_is_null());
                    let (a, b) = if src_null { (src, tgt) } else { (tgt, src) };
                    self.note(
                        Severity::Warning,
                        format!("'' is NULL in {a} but an empty string in {b}"),
                        Some("''".into()),
                    );
                }
                quote_str(s)
            }
            Value::Boolean(b) if self.has(|d| d.supports_boolean_literals()) => {
                if *b { "TRUE" } else { "FALSE" }.into()
            }
            Value::Boolean(b) => {
                self.note(Severity::Rewritten, format!("boolean literal -> {}", if *b { 1 } else { 0 }), None);
                if *b { "1" } else { "0" }.into()
            }
            Value::Null => "NULL".into(),
        }
    }

    fn placeholder(&self, p: &str) -> R {
        let Some(d) = self.target else { return Ok(p.to_string()) };
        let rest = &p[1..];
        let kind = if rest.is_empty() {
            let n = self.anon_placeholders.get() + 1;
            self.anon_placeholders.set(n);
            PlaceholderKind::Numbered(n)
        } else if let Ok(n) = rest.parse::<usize>() {
            PlaceholderKind::Numbered(n)
        } else {
            PlaceholderKind::Named(rest.to_string())
        };
        match d.format_placeholder(&kind) {
            Some(out) => {
                if out != p {
                    self.note(Severity::Rewritten, format!("bind parameter {p} -> {out}"), None);
                }
                Ok(out)
            }
            None => {
                let hint = if matches!(kind, PlaceholderKind::Named(_)) { " (use positional parameters)" } else { "" };
                self.fail(
                    format!("{}{hint}", EmitError::unsupported(d, &format!("bind parameter '{p}'")).message),
                    Some(p.to_string()),
                )?;
                Ok(p.to_string())
            }
        }
    }

    fn typed_string(&self, dt: &DataType, value: &str) -> R {
        if self.has(|d| d.supports_typed_literals()) {
            return Ok(format!("{} {}", dt.name, quote_str(value)));
        }
        let lit = format!("{} {}", dt.name, quote_str(value));
        match dt.name.as_str() {
            "DATE" | "TIME" | "TIMESTAMP" => {
                self.note(Severity::Rewritten, format!("{} literal -> plain string", dt.name), Some(lit));
                Ok(quote_str(value))
            }
            other => {
                self.need_at(|d| d.supports_typed_literals(), &format!("{other} literals"), || Some(lit.clone()))?;
                Ok(lit)
            }
        }
    }

    pub fn expr(&self, e: &Expr) -> R {
        Ok(match e {
            Expr::Identifier(i) => {
                if let (true, Some(d), None) = (self.collect, self.target, i.quote_style) {
                    if let Some(hint) = d.function_hint(&i.value.to_ascii_uppercase()) {
                        self.note(
                            Severity::Incompatible,
                            format!(
                                "{} does not exist in {} (if it is not a column): {hint}",
                                i.value.to_ascii_uppercase(),
                                d.name()
                            ),
                            Some(i.value.clone()),
                        );
                    }
                }
                self.ident(i)
            }
            Expr::CompoundIdentifier(p) => self.idents(p, "."),
            Expr::Value(v) => self.value(v),
            Expr::Placeholder(p) => self.placeholder(p)?,
            Expr::TypedString { data_type, value } => self.typed_string(data_type, value)?,
            Expr::BinaryOp { left, op, right } => self.binary(left, *op, right)?,
            Expr::UnaryOp { op, expr } => {
                let inner = self.expr(expr)?;
                match op {
                    UnaryOperator::Not => format!("NOT {inner}"),
                    UnaryOperator::Prior => {
                        self.need(|d| d.supports_connect_by(), "PRIOR")?;
                        format!("PRIOR {inner}")
                    }
                    UnaryOperator::ConnectByRoot => {
                        self.need(|d| d.supports_connect_by(), "CONNECT_BY_ROOT")?;
                        format!("CONNECT_BY_ROOT {inner}")
                    }
                    UnaryOperator::Plus if matches!(**expr, Expr::UnaryOp { .. }) => format!("+ {inner}"),
                    UnaryOperator::Minus if matches!(**expr, Expr::UnaryOp { .. }) => format!("- {inner}"),
                    UnaryOperator::Plus => format!("+{inner}"),
                    UnaryOperator::Minus => format!("-{inner}"),
                }
            }
            Expr::Nested(e) => format!("({})", self.expr(e)?),
            Expr::Tuple(t) => format!("({})", self.list(t, Self::expr)?),
            Expr::IsNull(e) => format!("{} IS NULL", self.expr(e)?),
            Expr::IsNotNull(e) => format!("{} IS NOT NULL", self.expr(e)?),
            Expr::Between { expr, negated, low, high } => {
                format!("{} {}BETWEEN {} AND {}", self.expr(expr)?, neg(*negated), self.expr(low)?, self.expr(high)?)
            }
            Expr::InList { expr, list, negated } => {
                format!("{} {}IN ({})", self.expr(expr)?, neg(*negated), self.list(list, Self::expr)?)
            }
            Expr::InSubquery { expr, subquery, negated } => {
                format!("{} {}IN ({})", self.expr(expr)?, neg(*negated), self.query(subquery)?)
            }
            Expr::Like { expr, negated, kind, pattern, escape } => {
                let (l, p) = (self.expr(expr)?, self.expr(pattern)?);
                let mut s = match kind {
                    LikeKind::ILike if !self.has(|d| d.supports_ilike()) => {
                        self.note(
                            Severity::Rewritten,
                            "ILIKE -> LOWER(a) LIKE LOWER(b)",
                            Some(format!("{l} ILIKE {p}")),
                        );
                        format!("LOWER({l}) {}LIKE LOWER({p})", neg(*negated))
                    }
                    LikeKind::Glob => {
                        self.need_at(|d| d.supports_glob(), "GLOB", || Some(format!("{l} GLOB {p}")))?;
                        format!("{l} {}GLOB {p}", neg(*negated))
                    }
                    LikeKind::ILike => format!("{l} {}ILIKE {p}", neg(*negated)),
                    LikeKind::Like => format!("{l} {}LIKE {p}", neg(*negated)),
                };
                if let Some(esc) = escape {
                    s.push_str(&format!(" ESCAPE {}", self.expr(esc)?));
                }
                s
            }
            Expr::Exists { subquery, negated } => format!("{}EXISTS ({})", neg(*negated), self.query(subquery)?),
            Expr::Subquery(q) => format!("({})", self.query(q)?),
            Expr::Cast { expr, data_type } => format!("CAST({} AS {})", self.expr(expr)?, self.data_type(data_type)?),
            Expr::AtTimeZone { expr, zone } => format!("{} AT TIME ZONE {}", self.expr(expr)?, self.expr(zone)?),
            Expr::Case { operand, branches, else_result } => {
                let mut s = String::from("CASE");
                if let Some(o) = operand {
                    s.push_str(&format!(" {}", self.expr(o)?));
                }
                for b in branches {
                    s.push_str(&format!(" WHEN {} THEN {}", self.expr(&b.condition)?, self.expr(&b.result)?));
                }
                if let Some(e) = else_result {
                    s.push_str(&format!(" ELSE {}", self.expr(e)?));
                }
                s.push_str(" END");
                s
            }
            Expr::Function(f) => self.function(f)?,
            Expr::Trim { side, chars, expr } => self.trim(*side, chars.as_deref(), expr)?,
        })
    }

    fn binary(&self, left: &Expr, op: BinaryOperator, right: &Expr) -> R {
        let (l, r) = (self.expr(left)?, self.expr(right)?);
        let sym = match op {
            BinaryOperator::Plus => "+",
            BinaryOperator::Minus => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => {
                if self.differs(|d| d.integer_division_truncates()) {
                    let (src, tgt) = self.names();
                    let src_trunc = self.source.is_some_and(|d| d.integer_division_truncates());
                    let (a, b) = if src_trunc { (src, tgt) } else { (tgt, src) };
                    self.note(
                        Severity::Warning,
                        format!("integer / integer truncates in {a} but returns a decimal in {b}"),
                        Some(format!("{l} / {r}")),
                    );
                }
                "/"
            }
            BinaryOperator::Modulo => {
                if !self.has(|d| d.supports_modulo_operator()) {
                    self.note(Severity::Rewritten, "a % b -> MOD(a, b)", Some(format!("{l} % {r}")));
                    return Ok(format!("MOD({l}, {r})"));
                }
                "%"
            }
            BinaryOperator::Concat => {
                if self.differs(|d| d.concat_null_is_empty()) {
                    let (src, tgt) = self.names();
                    let src_empty = self.source.is_some_and(|d| d.concat_null_is_empty());
                    let (a, b) = if src_empty { (src, tgt) } else { (tgt, src) };
                    self.note(
                        Severity::Warning,
                        format!("|| treats NULL as an empty string in {a}, but yields NULL in {b}"),
                        Some(format!("{l} || {r}")),
                    );
                }
                "||"
            }
            BinaryOperator::Eq => "=",
            BinaryOperator::NotEq => "<>",
            BinaryOperator::Lt => "<",
            BinaryOperator::LtEq => "<=",
            BinaryOperator::Gt => ">",
            BinaryOperator::GtEq => ">=",
            BinaryOperator::And => "AND",
            BinaryOperator::Or => "OR",
            BinaryOperator::JsonGet | BinaryOperator::JsonGetText => {
                self.need_at(|d| d.supports_json_arrows(), "JSON operators (->, ->>)", || Some(format!("{l} -> {r}")))?;
                if op == BinaryOperator::JsonGet {
                    "->"
                } else {
                    "->>"
                }
            }
        };
        Ok(format!("{l} {sym} {r}"))
    }

    /// Is this function call available on the target, judged against the
    /// catalogs of built-in functions (see [`crate::functions`])?
    fn check_function(&self, s: &dyn Dialect, t: &dyn Dialect, f: &Function, rendered: &str) {
        // package / schema-qualified calls (other than system schemas) are user or
        // package objects we know nothing about
        let name = match f.name.0.as_slice() {
            [n] => n,
            [schema, n] if matches!(schema.value.to_lowercase().as_str(), "pg_catalog" | "sys" | "main") => n,
            _ => return,
        };
        let lower = name.value.to_lowercase();
        let upper = lower.to_uppercase();
        let at = Some(rendered.to_string());
        if let Some(why) = t.function_collision(&upper, f.args.len()) {
            self.note(
                Severity::Incompatible,
                format!("function {upper} exists in {} but means something else: {why}", t.name()),
                at,
            );
        } else if t.has_function(&lower) {
        } else if let Some(hint) = t.function_hint(&upper) {
            self.note(Severity::Incompatible, format!("function {upper} does not exist in {}: {hint}", t.name()), at);
        } else {
            let others = crate::functions::dialects_with(&lower);
            if !others.is_empty() {
                self.note(
                    Severity::Incompatible,
                    format!("function {upper} does not exist in {} (available in {})", t.name(), others.join("/")),
                    at,
                );
            } else if t.name() != s.name() {
                // trusted in the dialect it was written for; unverifiable elsewhere
                self.note(
                    Severity::Unverified,
                    format!(
                        "function {upper} is not a built-in of any supported dialect; assumed user-defined, check that it exists in {}",
                        t.name()
                    ),
                    at,
                );
            }
        }
    }

    fn trim(&self, side: Option<TrimSide>, chars: Option<&Expr>, expr: &Expr) -> R {
        let (e, c) = (self.expr(expr)?, chars.map(|c| self.expr(c)).transpose()?);
        if self.has(|d| d.supports_trim_from()) {
            let side = match side {
                Some(TrimSide::Leading) => "LEADING ",
                Some(TrimSide::Trailing) => "TRAILING ",
                Some(TrimSide::Both) => "BOTH ",
                None => "",
            };
            return Ok(match c {
                Some(c) => format!("TRIM({side}{c} FROM {e})"),
                None if side.is_empty() => format!("TRIM({e})"),
                None => format!("TRIM({side}FROM {e})"),
            });
        }
        let func = match side {
            Some(TrimSide::Leading) => "LTRIM",
            Some(TrimSide::Trailing) => "RTRIM",
            _ => "TRIM",
        };
        let out = match c {
            Some(c) => format!("{func}({e}, {c})"),
            None => format!("{func}({e})"),
        };
        self.note(Severity::Rewritten, "TRIM(... FROM ...) -> TRIM/LTRIM/RTRIM(expr, chars)", Some(out.clone()));
        Ok(out)
    }

    fn function(&self, f: &Function) -> R {
        let args = self.list(&f.args, |s, a| match a {
            FunctionArg::Wildcard => Ok("*".to_string()),
            FunctionArg::Expr(e) => s.expr(e),
        })?;
        let mut out = format!("{}({}{args})", self.object_name(&f.name), if f.distinct { "DISTINCT " } else { "" });
        if let (true, Some(s), Some(t)) = (self.collect, self.source, self.target) {
            self.check_function(s, t, f, &out);
        }
        if let Some(w) = &f.over {
            let mut parts = vec![];
            if !w.partition_by.is_empty() {
                parts.push(format!("PARTITION BY {}", self.list(&w.partition_by, Self::expr)?));
            }
            if !w.order_by.is_empty() {
                parts.push(format!("ORDER BY {}", self.list(&w.order_by, Self::order_by_expr)?));
            }
            out.push_str(&format!(" OVER ({})", parts.join(" ")));
        }
        Ok(out)
    }
}

fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn neg(negated: bool) -> &'static str {
    if negated {
        "NOT "
    } else {
        ""
    }
}

// ------------------------------------------------------------ public API

impl Statement {
    /// Emit this statement for a specific engine.
    pub fn to_sql(&self, target: &dyn Dialect) -> Result<String, EmitError> {
        Emitter::for_dialect(target).statement(self)
    }
}

macro_rules! display_neutral {
    ($($t:ty => $m:ident),* $(,)?) => {$(
        impl Display for $t {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                match Emitter::neutral().$m(self) {
                    Ok(s) => f.write_str(&s),
                    Err(_) => Err(fmt::Error), // neutral emission cannot fail
                }
            }
        }
    )*};
}

display_neutral! {
    Statement => statement,
    Query => query,
    SetExpr => set_expr,
    Expr => expr,
    DataType => data_type,
    OrderByExpr => order_by_expr,
}

impl Display for Ident {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&Emitter::neutral().ident(self))
    }
}

impl Display for ObjectName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&Emitter::neutral().object_name(self))
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&Emitter::neutral().value(self))
    }
}

/// The `AND`ed predicates of a condition.
fn flatten_and<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::BinaryOp { left, op: BinaryOperator::And, right } => {
            flatten_and(left, out);
            flatten_and(right, out);
        }
        other => out.push(other),
    }
}

/// `n` of the predicates `ROWNUM = 1`, `ROWNUM <= n`, `ROWNUM < n` (and the reversed `1 >= ROWNUM`, ...): the number of rows
/// they let through. `ROWNUM = 2` and the like never match in Oracle and are not a row limit: left alone.
fn rownum_limit(e: &Expr) -> Option<u64> {
    let Expr::BinaryOp { left, op, right } = e else { return None };
    let is_rownum = |x: &Expr| matches!(x, Expr::Identifier(i) if i.quote_style.is_none() && i.value.eq_ignore_ascii_case("ROWNUM"));
    let number = |x: &Expr| match x {
        Expr::Value(Value::Number(n)) => n.parse::<u64>().ok(),
        _ => None,
    };
    let (op, n) = if is_rownum(left) {
        (*op, number(right)?)
    } else if is_rownum(right) {
        // `n >= ROWNUM` reads as `ROWNUM <= n`
        let flipped = match op {
            BinaryOperator::Eq => BinaryOperator::Eq,
            BinaryOperator::Gt => BinaryOperator::Lt,
            BinaryOperator::GtEq => BinaryOperator::LtEq,
            _ => return None,
        };
        (flipped, number(left)?)
    } else {
        return None;
    };
    match op {
        BinaryOperator::Eq if n == 1 => Some(1),
        BinaryOperator::LtEq => Some(n),
        BinaryOperator::Lt => n.checked_sub(1),
        _ => None,
    }
}
