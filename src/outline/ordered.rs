use crate::ast::*;
use super::{flatten_and, hang, indent, list_block, neg, Outline};

pub(super) const WIDTH: usize = 72;

impl Outline {

    pub(super) fn id(&self, i: &Ident) -> String {
        self.e.ident(i)
    }

    pub(super) fn name(&self, n: &ObjectName) -> String {
        self.e.object_name(n)
    }

    pub(super) fn ty(&self, dt: &DataType) -> String {
        self.e.data_type(dt).unwrap_or_default().to_lowercase()
    }

    pub(super) fn ids(&self, v: &[Ident]) -> String {
        v.iter().map(|i| self.id(i)).collect::<Vec<_>>().join(", ")
    }

    // ------------------------------------------------------- statements

    pub(super) fn statement(&self, s: &Statement) -> String {
        match s {
            Statement::Query(q) => self.query(q),
            Statement::Insert(i) => self.insert(i),
            Statement::Update(u) => self.update(u),
            Statement::Delete(d) => self.delete(d),
            Statement::CreateTable(c) => self.create_table(c),
            Statement::CreateIndex(c) => self.create_index(c),
            Statement::Drop(d) => self.drop_stmt(d),
        }
    }

    pub(super) fn filter(&self, keyword: &str, cond: &Expr) -> String {
        let mut parts = vec![];
        flatten_and(cond, &mut parts);
        let parts: Vec<String> = parts.into_iter().map(|p| self.expr(p)).collect();
        let one = format!("{keyword} {}", parts.join(" and "));
        if parts.len() == 1 || (one.len() <= WIDTH && !one.contains('\n')) {
            return one;
        }
        // `and` is right-aligned under the keyword so conditions line up;
        // multi-line conditions (subqueries) are shifted by that same pad.
        let pad = (keyword.len() + 1).saturating_sub(4);
        let mut out = format!("{keyword} {}", parts[0]);
        for p in &parts[1..] {
            out.push_str(&format!("\n{}and {}", " ".repeat(pad), hang(p, pad)));
        }
        out
    }

    pub(super) fn returning(&self, r: &[SelectItem]) -> Option<String> {
        (!r.is_empty()).then(|| list_block("returning", &r.iter().map(|i| self.select_item(i)).collect::<Vec<_>>()))
    }

    pub(super) fn insert(&self, i: &Insert) -> String {
        let mut head = String::from("insert");
        if let Some(or) = &i.or {
            head.push_str(&format!(" or {}", or.to_lowercase()));
        }
        head.push_str(&format!(" into {}", self.name(&i.table)));
        if !i.columns.is_empty() {
            head.push_str(&format!(" ({})", self.ids(&i.columns)));
        }
        let mut lines = vec![head];
        match &i.source {
            InsertSource::DefaultValues => lines.push("default values".into()),
            InsertSource::Query(q) => match &q.body {
                SetExpr::Values(v) if q.with.is_none() => {
                    lines.push("values".into());
                    for row in &v.rows {
                        lines.push(indent(&format!("({})", self.exprs(row)), 2));
                    }
                }
                _ => lines.push(self.query(q)),
            },
        }
        if let Some(oc) = &i.on_conflict {
            let target = if oc.target.is_empty() { String::new() } else { format!(" ({})", self.ids(&oc.target)) };
            match &oc.action {
                OnConflictAction::DoNothing => lines.push(format!("on conflict{target} do nothing")),
                OnConflictAction::DoUpdate { assignments, selection } => {
                    lines.push(format!("on conflict{target} do update"));
                    lines.push(indent(&self.assignments(assignments), 2));
                    if let Some(s) = selection {
                        lines.push(indent(&self.filter("filter", s), 2));
                    }
                }
            }
        }
        lines.extend(self.returning(&i.returning));
        lines.join("\n")
    }

    pub(super) fn assignments(&self, a: &[Assignment]) -> String {
        let items: Vec<String> = a.iter().map(|x| format!("{} = {}", self.name(&x.column), self.expr(&x.value))).collect();
        match items.as_slice() {
            [one] if !one.contains('\n') => format!("set {one}"),
            _ => format!("set\n{}", indent(&items.join("\n"), 2)),
        }
    }

    pub(super) fn update(&self, u: &Update) -> String {
        let mut lines = vec![format!("update {}", self.table_factor(&u.table)), self.assignments(&u.assignments)];
        if !u.from.is_empty() {
            lines.push(self.table_block(&u.from));
        }
        if let Some(s) = &u.selection {
            lines.push(self.filter("filter", s));
        }
        lines.extend(self.returning(&u.returning));
        lines.join("\n")
    }

    pub(super) fn delete(&self, d: &Delete) -> String {
        let mut lines = vec![format!("delete from {}", self.table_factor(&d.table))];
        if let Some(s) = &d.selection {
            lines.push(self.filter("filter", s));
        }
        lines.extend(self.returning(&d.returning));
        lines.join("\n")
    }

    pub(super) fn create_table(&self, c: &CreateTable) -> String {
        let mut head = String::from("create ");
        if c.temporary {
            head.push_str("temporary ");
        }
        head.push_str(&format!("table {}", self.name(&c.name)));
        if c.if_not_exists {
            head.push_str(" if not exists");
        }
        let rows: Vec<(String, String, String)> = c
            .columns
            .iter()
            .map(|col| {
                let ty = col.data_type.as_ref().map(|t| self.ty(t)).unwrap_or_default();
                let opts = col.options.iter().map(|o| self.column_option(o)).collect::<Vec<_>>().join("  ");
                (self.id(&col.name), ty, opts)
            })
            .collect();
        let w0 = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
        let w1 = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        let mut lines = vec![head];
        for (n, t, o) in rows {
            lines.push(indent(format!("{n:<w0$}  {t:<w1$}  {o}").trim_end(), 2));
        }
        for con in &c.constraints {
            lines.push(indent(&self.table_constraint(con), 2));
        }
        lines.join("\n")
    }

    pub(super) fn column_option(&self, o: &ColumnOptionDef) -> String {
        let body = match &o.option {
            ColumnOption::Null => "null".to_string(),
            ColumnOption::NotNull => "not null".into(),
            ColumnOption::Default(e) => format!("default {}", self.expr(e)),
            ColumnOption::PrimaryKey => "primary key".into(),
            ColumnOption::Unique => "unique".into(),
            ColumnOption::Autoincrement => "autoincrement".into(),
            ColumnOption::Identity { always } => {
                format!("identity {}", if *always { "always" } else { "by default" })
            }
            ColumnOption::References(r) => format!("references {}", self.fk(r)),
            ColumnOption::Check(e) => format!("check ({})", self.expr(e)),
            ColumnOption::Collate(c) => format!("collate {}", self.id(c)),
        };
        match &o.name {
            Some(n) => format!("constraint {} {body}", self.id(n)),
            None => body,
        }
    }

    pub(super) fn fk(&self, r: &ForeignKeyRef) -> String {
        let mut s = self.name(&r.table);
        if !r.columns.is_empty() {
            s.push_str(&format!(" ({})", self.ids(&r.columns)));
        }
        if let Some(a) = &r.on_delete {
            s.push_str(&format!(" on delete {}", a.to_lowercase()));
        }
        if let Some(a) = &r.on_update {
            s.push_str(&format!(" on update {}", a.to_lowercase()));
        }
        s
    }

    pub(super) fn table_constraint(&self, c: &TableConstraint) -> String {
        let body = match &c.kind {
            TableConstraintKind::PrimaryKey(cols) => format!("primary key ({})", self.ids(cols)),
            TableConstraintKind::Unique(cols) => format!("unique ({})", self.ids(cols)),
            TableConstraintKind::ForeignKey { columns, reference } => {
                format!("foreign key ({}) references {}", self.ids(columns), self.fk(reference))
            }
            TableConstraintKind::Check(e) => format!("check ({})", self.expr(e)),
        };
        match &c.name {
            Some(n) => format!("constraint {} {body}", self.id(n)),
            None => body,
        }
    }

    pub(super) fn create_index(&self, c: &CreateIndex) -> String {
        let cols = c.columns.iter().map(|o| self.order_by(o)).collect::<Vec<_>>().join(", ");
        let mut s = format!(
            "create {}index {}{} on {} ({cols})",
            if c.unique { "unique " } else { "" },
            self.name(&c.name),
            if c.if_not_exists { " if not exists" } else { "" },
            self.name(&c.table)
        );
        if let Some(w) = &c.selection {
            s.push_str(&format!("\n{}", indent(&self.filter("filter", w), 2)));
        }
        s
    }

    pub(super) fn drop_stmt(&self, d: &Drop) -> String {
        let t = match d.object_type {
            ObjectType::Table => "table",
            ObjectType::View => "view",
            ObjectType::Index => "index",
        };
        let names = d.names.iter().map(|n| self.name(n)).collect::<Vec<_>>().join(", ");
        let mut s = format!("drop {t} {names}");
        if d.if_exists {
            s.push_str(" if exists");
        }
        match d.behavior {
            Some(DropBehavior::Cascade) => s.push_str(" cascade"),
            Some(DropBehavior::Restrict) => s.push_str(" restrict"),
            None => {}
        }
        s
    }

    // ---------------------------------------------------------- queries

    pub(super) fn query(&self, q: &Query) -> String {
        let mut lines = vec![];
        if let Some(w) = &q.with {
            for cte in &w.ctes {
                let cols = if cte.columns.is_empty() { String::new() } else { format!(" ({})", self.ids(&cte.columns)) };
                lines.push(format!("with {}{}{cols} as", if w.recursive { "recursive " } else { "" }, self.id(&cte.name)));
                lines.push(indent(&self.query(&cte.query), 2));
            }
        }
        lines.push(self.set_expr(&q.body));
        if !q.order_by.is_empty() {
            let items: Vec<String> = q.order_by.iter().map(|o| self.order_by(o)).collect();
            lines.push(list_block("sort", &items));
        }
        if let Some(o) = &q.offset {
            lines.push(format!("skip {}", self.expr(o)));
        }
        if let Some(l) = &q.limit {
            lines.push(format!("take {}", self.expr(l)));
        }
        if let Some(f) = &q.fetch {
            let mut s = format!("take {}", f.quantity.as_ref().map_or("1".to_string(), |e| self.expr(e)));
            if f.percent {
                s.push_str(" percent");
            }
            if f.with_ties {
                s.push_str(" with ties");
            }
            lines.push(s);
        }
        lines.join("\n")
    }

    pub(super) fn set_expr(&self, e: &SetExpr) -> String {
        match e {
            SetExpr::Select(s) => self.select(s),
            SetExpr::Query(q) => format!("(\n{}\n)", indent(&self.query(q), 2)),
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
                format!("{}\n{op}{q}\n{}", self.set_expr(left), self.set_expr(right))
            }
        }
    }

    pub(super) fn table_block(&self, from: &[TableWithJoins]) -> String {
        let mut lines = vec![];
        for (i, t) in from.iter().enumerate() {
            let prefix = if i == 0 { "from" } else { "," };
            lines.push(format!("{prefix} {}", self.table_factor(&t.relation)));
            for j in &t.joins {
                lines.push(self.join(j));
            }
        }
        lines.join("\n")
    }

    pub(super) fn select(&self, s: &Select) -> String {
        let mut lines = vec![];
        if !s.from.is_empty() {
            lines.push(self.table_block(&s.from));
        }
        if let Some(w) = &s.selection {
            lines.push(self.filter("filter", w));
        }
        if !s.group_by.is_empty() {
            let items: Vec<String> = s.group_by.iter().map(|e| self.expr(e)).collect();
            lines.push(list_block("group", &items));
        }
        if let Some(h) = &s.having {
            lines.push(self.filter("having", h));
        }
        let prefix = match &s.distinct {
            Some(Distinct::Distinct) => "select distinct".to_string(),
            Some(Distinct::On(e)) => format!("select distinct on ({})", self.exprs(e)),
            None => "select".into(),
        };
        let items: Vec<String> = s.projection.iter().map(|i| self.select_item(i)).collect();
        lines.push(list_block(&prefix, &items));
        lines.join("\n")
    }

    pub(super) fn select_item(&self, i: &SelectItem) -> String {
        match i {
            SelectItem::UnnamedExpr(e) => self.expr(e),
            SelectItem::ExprWithAlias { expr, alias } => format!("{} as {}", self.expr(expr), self.id(alias)),
            SelectItem::QualifiedWildcard(n) => format!("{}.*", self.name(n)),
            SelectItem::Wildcard => "*".into(),
        }
    }

    pub(super) fn alias(&self, a: &Option<TableAlias>) -> String {
        match a {
            None => String::new(),
            Some(a) if a.columns.is_empty() => format!(" as {}", self.id(&a.name)),
            Some(a) => format!(" as {} ({})", self.id(&a.name), self.ids(&a.columns)),
        }
    }

    pub(super) fn table_factor(&self, t: &TableFactor) -> String {
        match t {
            TableFactor::Table { name, alias } => format!("{}{}", self.name(name), self.alias(alias)),
            TableFactor::Derived { subquery, alias } => {
                format!("(\n{}\n){}", indent(&self.query(subquery), 2), self.alias(alias))
            }
            TableFactor::Nested(t) => {
                let mut parts = vec![self.table_factor(&t.relation)];
                parts.extend(t.joins.iter().map(|j| self.join(j)));
                format!("(\n{}\n)", indent(&parts.join("\n"), 2))
            }
        }
    }

    pub(super) fn join(&self, j: &Join) -> String {
        let kind = match j.kind {
            JoinKind::Inner => "join",
            JoinKind::Left => "left join",
            JoinKind::Right => "right join",
            JoinKind::Full => "full join",
            JoinKind::Cross => "cross join",
        };
        let natural = if j.constraint == JoinConstraint::Natural { "natural " } else { "" };
        let mut s = format!("{natural}{kind} {}", self.table_factor(&j.relation));
        match &j.constraint {
            JoinConstraint::On(e) => s.push_str(&format!(" on {}", self.expr(e))),
            JoinConstraint::Using(c) => s.push_str(&format!(" using ({})", self.ids(c))),
            JoinConstraint::Natural | JoinConstraint::None => {}
        }
        s
    }

    pub(super) fn order_by(&self, o: &OrderByExpr) -> String {
        let mut s = self.expr(&o.expr);
        match o.asc {
            Some(true) => s.push_str(" asc"),
            Some(false) => s.push_str(" desc"),
            None => {}
        }
        match o.nulls_first {
            Some(true) => s.push_str(" nulls first"),
            Some(false) => s.push_str(" nulls last"),
            None => {}
        }
        s
    }

    // ------------------------------------------------------ expressions

    pub(super) fn exprs(&self, v: &[Expr]) -> String {
        v.iter().map(|e| self.expr(e)).collect::<Vec<_>>().join(", ")
    }

    pub(super) fn sub(&self, q: &Query) -> String {
        if self.tight {
            Self::block(&self.tight_query(q))
        } else {
            format!("(\n{}\n)", indent(&self.query(q), 2))
        }
    }

    pub(super) fn expr(&self, e: &Expr) -> String {
        match e {
            Expr::Identifier(i) => self.id(i),
            Expr::CompoundIdentifier(p) => self.ids_dot(p),
            Expr::Value(Value::Null) => "null".into(),
            Expr::Value(Value::Boolean(b)) => b.to_string(),
            Expr::Value(v) => self.e.value(v),
            Expr::Placeholder(p) => p.clone(),
            Expr::TypedString { data_type, value } => {
                format!("{} {}", data_type.name.to_lowercase(), self.e.value(&Value::String(value.clone())))
            }
            Expr::BinaryOp { left, op, right } => {
                let sym = match op {
                    BinaryOperator::Plus => "+",
                    BinaryOperator::Minus => "-",
                    BinaryOperator::Multiply => "*",
                    BinaryOperator::Divide => "/",
                    BinaryOperator::Modulo => "%",
                    BinaryOperator::Concat => "||",
                    BinaryOperator::Eq => "=",
                    BinaryOperator::NotEq => "!=",
                    BinaryOperator::Lt => "<",
                    BinaryOperator::LtEq => "<=",
                    BinaryOperator::Gt => ">",
                    BinaryOperator::GtEq => ">=",
                    BinaryOperator::And => "and",
                    BinaryOperator::Or => "or",
                    BinaryOperator::JsonGet => "->",
                    BinaryOperator::JsonGetText => "->>",
                };
                format!("{} {sym} {}", self.expr(left), self.expr(right))
            }
            Expr::UnaryOp { op, expr } => {
                let inner = self.expr(expr);
                match op {
                    UnaryOperator::Not => format!("not {inner}"),
                    UnaryOperator::Plus => format!("+{inner}"),
                    UnaryOperator::Minus if inner.starts_with('-') => format!("- {inner}"),
                    UnaryOperator::Minus => format!("-{inner}"),
                }
            }
            Expr::Nested(e) => format!("({})", self.expr(e)),
            Expr::Tuple(t) => format!("({})", self.exprs(t)),
            Expr::IsNull(e) => format!("{} is null", self.expr(e)),
            Expr::IsNotNull(e) => format!("{} is not null", self.expr(e)),
            Expr::Between { expr, negated, low, high } if self.tight => {
                format!("{} {}in {}..{}", self.expr(expr), if *negated { "!" } else { "" }, self.expr(low), self.expr(high))
            }
            Expr::Between { expr, negated, low, high } => {
                format!("{} {}between {} and {}", self.expr(expr), neg(*negated), self.expr(low), self.expr(high))
            }
            Expr::InList { expr, list, negated } if self.tight => {
                format!("{} {}in [{}]", self.expr(expr), if *negated { "!" } else { "" }, self.exprs(list))
            }
            Expr::InList { expr, list, negated } => {
                format!("{} {}in ({})", self.expr(expr), neg(*negated), self.exprs(list))
            }
            Expr::InSubquery { expr, subquery, negated } => {
                let not = if self.tight { if *negated { "!" } else { "" } } else { neg(*negated) };
                format!("{} {not}in {}", self.expr(expr), self.sub(subquery))
            }
            Expr::Like { expr, negated, kind, pattern, escape } => {
                let kind = match kind {
                    LikeKind::Like => "like",
                    LikeKind::ILike => "ilike",
                    LikeKind::Glob => "glob",
                };
                let mut s = format!("{} {}{kind} {}", self.expr(expr), neg(*negated), self.expr(pattern));
                if let Some(e) = escape {
                    s.push_str(&format!(" escape {}", self.expr(e)));
                }
                s
            }
            Expr::Exists { subquery, negated } => {
                let not = if self.tight { if *negated { "!" } else { "" } } else { neg(*negated) };
                format!("{not}exists {}", self.sub(subquery))
            }
            Expr::Subquery(q) => self.sub(q),
            Expr::Cast { expr, data_type } => {
                let simple = matches!(
                    **expr,
                    Expr::Identifier(_) | Expr::CompoundIdentifier(_) | Expr::Value(_) | Expr::Placeholder(_)
                        | Expr::Function(_) | Expr::Nested(_)
                );
                if simple {
                    format!("{}::{}", self.expr(expr), self.ty(data_type))
                } else {
                    format!("cast({} as {})", self.expr(expr), self.ty(data_type))
                }
            }
            Expr::Case { operand, branches, else_result } => {
                let mut s = String::from("case");
                if let Some(o) = operand {
                    s.push_str(&format!(" {}", self.expr(o)));
                }
                for b in branches {
                    s.push_str(&format!(" when {} then {}", self.expr(&b.condition), self.expr(&b.result)));
                }
                if let Some(e) = else_result {
                    s.push_str(&format!(" else {}", self.expr(e)));
                }
                s.push_str(" end");
                s
            }
            Expr::Function(f) => {
                let args = f
                    .args
                    .iter()
                    .map(|a| match a {
                        FunctionArg::Wildcard => "*".to_string(),
                        FunctionArg::Expr(e) => self.expr(e),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut s = format!("{}({}{args})", self.name(&f.name), if f.distinct { "distinct " } else { "" });
                if let Some(w) = &f.over {
                    let mut parts = vec![];
                    if !w.partition_by.is_empty() {
                        parts.push(format!("partition by {}", self.exprs(&w.partition_by)));
                    }
                    if !w.order_by.is_empty() {
                        let o: Vec<String> = w.order_by.iter().map(|o| self.order_by(o)).collect();
                        parts.push(format!("order by {}", o.join(", ")));
                    }
                    s.push_str(&format!(" over ({})", parts.join(" ")));
                }
                s
            }
        }
    }

    pub(super) fn ids_dot(&self, v: &[Ident]) -> String {
        v.iter().map(|i| self.id(i)).collect::<Vec<_>>().join(".")
    }
}
