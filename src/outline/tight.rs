//! The `tight` syntax: one stage per line, each introduced by a symbol, so the
//! shape of a statement can be scanned down the left margin.

use super::{flatten_and, indent, list_block, Outline};
use crate::ast::*;

impl Outline {
    /// `{` / `}` block with the body indented two spaces.
    pub(super) fn block(inner: &str) -> String {
        format!("{{\n{}\n}}", indent(inner, 2))
    }

    /// `? a` followed by `& b` lines for further AND-ed conditions.
    fn tight_filter(&self, mark: &str, cond: &Expr) -> String {
        let mut parts = vec![];
        flatten_and(cond, &mut parts);
        let mut lines = vec![];
        for (i, p) in parts.into_iter().enumerate() {
            lines.push(format!("{} {}", if i == 0 { mark } else { "&" }, self.expr(p)));
        }
        lines.join("\n")
    }

    pub(super) fn tight_statement(&self, s: &Statement) -> String {
        match s {
            Statement::Query(q) => self.tight_query(q),
            Statement::Insert(i) => self.tight_insert(i),
            Statement::Update(u) => {
                let mut l =
                    vec![format!("update {}", self.tight_table_factor(&u.table)), self.assignments(&u.assignments)];
                l.extend(self.tight_from(&u.from));
                if let Some(w) = &u.selection {
                    l.push(self.tight_filter("?", w));
                }
                l.extend(self.returning(&u.returning));
                l.join("\n")
            }
            Statement::Delete(d) => {
                let mut l = vec![format!("delete {}", self.tight_table_factor(&d.table))];
                if let Some(w) = &d.selection {
                    l.push(self.tight_filter("?", w));
                }
                l.extend(self.returning(&d.returning));
                l.join("\n")
            }
            Statement::CreateTable(c) => self.tight_create_table(c),
            Statement::CreateIndex(c) => {
                let cols = c.columns.iter().map(|o| self.order_by(o)).collect::<Vec<_>>().join(", ");
                let mut s = format!(
                    "{}index {}{} on {}({cols})",
                    if c.unique { "unique " } else { "" },
                    if c.if_not_exists { "if not exists " } else { "" },
                    self.name(&c.name),
                    self.name(&c.table)
                );
                if let Some(w) = &c.selection {
                    s.push_str(&format!("\n{}", self.tight_filter("?", w)));
                }
                s
            }
            Statement::Drop(_) => self.statement(s),
        }
    }

    fn tight_insert(&self, i: &Insert) -> String {
        let or = i.or.as_ref().map(|o| format!(" or {}", o.to_lowercase())).unwrap_or_default();
        let cols = if i.columns.is_empty() { String::new() } else { format!("({})", self.ids(&i.columns)) };
        let mut l = vec![format!("insert{or} {}{cols}", self.name(&i.table))];
        match &i.source {
            InsertSource::DefaultValues => l.push("default values".into()),
            InsertSource::Query(q) => l.push(self.tight_query(q)),
        }
        if let Some(oc) = &i.on_conflict {
            let target = if oc.target.is_empty() { String::new() } else { format!(" ({})", self.ids(&oc.target)) };
            match &oc.action {
                OnConflictAction::DoNothing => l.push(format!("on conflict{target} nothing")),
                OnConflictAction::DoUpdate { assignments, selection } => {
                    let set = self.assignments(assignments);
                    match set.strip_prefix("set ") {
                        Some(one) => l.push(format!("on conflict{target} update {one}")),
                        None => {
                            l.push(format!("on conflict{target} update"));
                            l.push(set.strip_prefix("set\n").unwrap_or(&set).to_string());
                        }
                    }
                    if let Some(w) = selection {
                        l.push(indent(&self.tight_filter("?", w), 2));
                    }
                }
            }
        }
        l.extend(self.returning(&i.returning));
        l.join("\n")
    }

    fn tight_create_table(&self, c: &CreateTable) -> String {
        let head = format!(
            "table {}{}{} {{",
            if c.temporary { "temp " } else { "" },
            if c.if_not_exists { "if not exists " } else { "" },
            self.name(&c.name)
        );
        let rows: Vec<(String, String, String)> = c
            .columns
            .iter()
            .map(|col| {
                let ty = col.data_type.as_ref().map(|t| self.ty(t)).unwrap_or_default();
                let opts = col.options.iter().map(|o| self.tight_column_option(o)).collect::<Vec<_>>().join(" ");
                (self.id(&col.name), ty, opts)
            })
            .collect();
        let w0 = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
        let w1 = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        let mut body: Vec<String> =
            rows.iter().map(|(n, t, o)| format!("{n:<w0$}  {t:<w1$}  {o}").trim_end().to_string()).collect();
        body.extend(c.constraints.iter().map(|con| self.tight_table_constraint(con)));
        format!("{head}\n{}\n}}", indent(&body.join("\n"), 2))
    }

    fn tight_fk(&self, r: &ForeignKeyRef) -> String {
        let cols = if r.columns.is_empty() { String::new() } else { format!("({})", self.ids(&r.columns)) };
        let mut s = format!("-> {}{cols}", self.name(&r.table));
        if let Some(a) = &r.on_delete {
            s.push_str(&format!(" on delete {}", a.to_lowercase()));
        }
        if let Some(a) = &r.on_update {
            s.push_str(&format!(" on update {}", a.to_lowercase()));
        }
        s
    }

    fn tight_column_option(&self, o: &ColumnOptionDef) -> String {
        let body = match &o.option {
            ColumnOption::Null => "null".to_string(),
            ColumnOption::NotNull => "!null".into(),
            ColumnOption::Default(e) => format!("= {}", self.expr(e)),
            ColumnOption::PrimaryKey => "pk".into(),
            ColumnOption::Unique => "unique".into(),
            ColumnOption::Autoincrement => "autoinc".into(),
            ColumnOption::Identity { always } => format!("identity {}", if *always { "always" } else { "default" }),
            ColumnOption::References(r) => self.tight_fk(r),
            ColumnOption::Check(e) => format!("check({})", self.expr(e)),
            ColumnOption::Collate(c) => format!("collate {}", self.id(c)),
        };
        match &o.name {
            Some(n) => format!("constraint {} {body}", self.id(n)),
            None => body,
        }
    }

    fn tight_table_constraint(&self, c: &TableConstraint) -> String {
        let body = match &c.kind {
            TableConstraintKind::PrimaryKey(cols) => format!("pk({})", self.ids(cols)),
            TableConstraintKind::Unique(cols) => format!("unique({})", self.ids(cols)),
            TableConstraintKind::ForeignKey { columns, reference } => {
                format!("fk({}) {}", self.ids(columns), self.tight_fk(reference))
            }
            TableConstraintKind::Check(e) => format!("check({})", self.expr(e)),
        };
        match &c.name {
            Some(n) => format!("constraint {} {body}", self.id(n)),
            None => body,
        }
    }

    // ---------------------------------------------------------- queries

    pub(super) fn tight_query(&self, q: &Query) -> String {
        let mut lines = vec![];
        if let Some(w) = &q.with {
            for cte in &w.ctes {
                let cols = if cte.columns.is_empty() { String::new() } else { format!("({})", self.ids(&cte.columns)) };
                lines.push(format!(
                    "let {}{}{cols} = {}",
                    if w.recursive { "rec " } else { "" },
                    self.id(&cte.name),
                    Self::block(&self.tight_query(&cte.query))
                ));
            }
        }
        lines.push(self.tight_set_expr(&q.body));
        if !q.order_by.is_empty() {
            let items: Vec<String> = q.order_by.iter().map(|o| self.order_by(o)).collect();
            lines.push(list_block("~", &items));
        }
        let offset = q.offset.as_ref().map(|e| self.expr(e));
        let mut limit = q.limit.as_ref().map(|e| self.expr(e));
        if let Some(f) = &q.fetch {
            let n = f.quantity.as_ref().map_or("1".to_string(), |e| self.expr(e));
            limit = Some(format!("{n}{}{}", if f.percent { "%" } else { "" }, if f.with_ties { " ties" } else { "" }));
        }
        if offset.is_some() || limit.is_some() {
            lines.push(format!("[{}:{}]", offset.unwrap_or_default(), limit.unwrap_or_default()));
        }
        lines.join("\n")
    }

    fn tight_set_expr(&self, e: &SetExpr) -> String {
        match e {
            SetExpr::Select(s) => self.tight_select(s),
            SetExpr::Query(q) => Self::block(&self.tight_query(q)),
            SetExpr::Values(v) => {
                let rows = v.rows.iter().map(|r| format!("({})", self.exprs(r))).collect::<Vec<_>>().join("\n");
                format!("values\n{}", indent(&rows, 2))
            }
            SetExpr::SetOperation { op, quantifier, left, right } => {
                let op = match op {
                    SetOperator::Union => "union",
                    SetOperator::Except => "except",
                    SetOperator::Intersect => "intersect",
                };
                let q = match quantifier {
                    SetQuantifier::All => " all",
                    SetQuantifier::Distinct => " distinct",
                    SetQuantifier::None => "",
                };
                format!(
                    "{}\n{op}{q}\n{}",
                    Self::block(&self.tight_set_expr(left)),
                    Self::block(&self.tight_set_expr(right))
                )
            }
        }
    }

    fn tight_from(&self, from: &[TableWithJoins]) -> Vec<String> {
        let mut l = vec![];
        for (i, t) in from.iter().enumerate() {
            l.push(format!("{} {}", if i == 0 { "@" } else { "@," }, self.tight_table_factor(&t.relation)));
            l.extend(t.joins.iter().map(|j| self.tight_join(j)));
        }
        l
    }

    fn tight_select(&self, s: &Select) -> String {
        let mut l = self.tight_from(&s.from);
        if let Some(w) = &s.selection {
            l.push(self.tight_filter("?", w));
        }
        if !s.group_by.is_empty() {
            let items: Vec<String> = s.group_by.iter().map(|e| self.expr(e)).collect();
            l.push(list_block("by", &items));
        }
        if let Some(h) = &s.having {
            l.push(self.tight_filter("??", h));
        }
        let mark = match &s.distinct {
            Some(Distinct::Distinct) => ">!".to_string(),
            Some(Distinct::On(e)) => format!(">! on ({})", self.exprs(e)),
            None => ">".into(),
        };
        let items: Vec<String> = s.projection.iter().map(|i| self.select_item(i)).collect();
        l.push(list_block(&mark, &items));
        l.join("\n")
    }

    fn tight_table_factor(&self, t: &TableFactor) -> String {
        let alias = |a: &Option<TableAlias>| match a {
            None => String::new(),
            Some(a) if a.columns.is_empty() => format!(" {}", self.id(&a.name)),
            Some(a) => format!(" {}({})", self.id(&a.name), self.ids(&a.columns)),
        };
        match t {
            TableFactor::Table { name, alias: a } => format!("{}{}", self.name(name), alias(a)),
            TableFactor::Derived { subquery, alias: a } => {
                format!("{}{}", Self::block(&self.tight_query(subquery)), alias(a))
            }
            TableFactor::Nested(t) => {
                let mut parts = vec![self.tight_table_factor(&t.relation)];
                parts.extend(t.joins.iter().map(|j| self.tight_join(j)));
                Self::block(&parts.join("\n"))
            }
        }
    }

    fn tight_join(&self, j: &Join) -> String {
        let sym = match j.kind {
            JoinKind::Inner => "+",
            JoinKind::Left => "<+",
            JoinKind::Right => "+>",
            JoinKind::Full => "<+>",
            JoinKind::Cross => "x",
        };
        let natural = if j.constraint == JoinConstraint::Natural { "nat " } else { "" };
        let mut s = format!("{natural}{sym} {}", self.tight_table_factor(&j.relation));
        match &j.constraint {
            JoinConstraint::On(e) => s.push_str(&format!(" on {}", self.expr(e))),
            JoinConstraint::Using(c) => s.push_str(&format!(" using ({})", self.ids(c))),
            JoinConstraint::Natural | JoinConstraint::None => {}
        }
        s
    }
}
