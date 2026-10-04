//! Human-oriented renderings of the shared AST. Neither is SQL and neither is
//! meant to be parsed back.
//!
//! * [`Syntax::Ordered`] reads top-down in execution order, one step per line,
//!   with lowercase keywords and little punctuation. Best for code review.
//!
//!   ```text
//!   from orders as o
//!   left join customers as c on o.customer_id = c.id
//!   filter o.status = 'open' and o.total > 100
//!   group c.name
//!   select c.name, sum(o.total) as revenue
//!   sort revenue desc
//!   take 10
//!   ```
//!
//! * [`Syntax::Tight`] keeps the same one-stage-per-line layout but marks each
//!   stage with a symbol, so the shape can be scanned down the left margin:
//!   `@` from, `+ <+ +> <+> x` joins, `?` filter (`&` for more conditions),
//!   `by` group, `??` having, `>` select, `~` sort, `[offset:limit]`, and
//!   `{ }` blocks for subqueries.
//!
//!   ```text
//!   @ orders o
//!   <+ customers c on o.customer_id = c.id
//!   ? o.status = 'open'
//!   & o.total > 100
//!   by c.name
//!   > c.name, sum(o.total) as revenue
//!   ~ revenue desc
//!   [:10]
//!   ```

mod ordered;
mod tight;

use crate::ast::{Expr, BinaryOperator, Statement};
use crate::emit::Emitter;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Ordered,
    Tight,
}

impl FromStr for Syntax {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "ordered" | "review" => Ok(Self::Ordered),
            "tight" => Ok(Self::Tight),
            other => Err(format!("unknown syntax '{other}' (expected ordered or tight)")),
        }
    }
}

/// Renders statements, separated by a blank line.
pub fn render(stmts: &[Statement], syntax: Syntax) -> String {
    stmts.iter().map(|s| s.to_outline(syntax)).collect::<Vec<_>>().join("\n\n")
}

impl Statement {
    pub fn to_outline(&self, syntax: Syntax) -> String {
        let o = Outline::new(syntax == Syntax::Tight);
        if o.tight { o.tight_statement(self) } else { o.statement(self) }
    }
}

pub(super) struct Outline {
    pub(super) e: Emitter<'static>,
    pub(super) tight: bool,
}

impl Outline {
    fn new(tight: bool) -> Self {
        Self { e: Emitter::neutral(), tight }
    }
}

pub(super) fn indent(text: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    text.lines().map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") }).collect::<Vec<_>>().join("\n")
}

/// Indent every line after the first.
pub(super) fn hang(text: &str, n: usize) -> String {
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    for l in lines {
        out.push('\n');
        out.push_str(&" ".repeat(n));
        out.push_str(l);
    }
    out
}

/// `prefix a, b, c` on one line when short, otherwise one item per line.
pub(super) fn list_block(prefix: &str, items: &[String]) -> String {
    let joined = items.join(", ");
    if prefix.len() + 1 + joined.len() <= ordered::WIDTH && !joined.contains('\n') {
        format!("{prefix} {joined}")
    } else {
        let body = items.iter().map(|i| format!("{i},")).collect::<Vec<_>>().join("\n");
        let body = body.strip_suffix(',').unwrap_or(&body).to_string();
        format!("{prefix}\n{}", indent(&body, 2))
    }
}

pub(super) fn flatten_and<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::BinaryOp { left, op: BinaryOperator::And, right } => {
            flatten_and(left, out);
            flatten_and(right, out);
        }
        other => out.push(other),
    }
}

pub(super) fn neg(negated: bool) -> &'static str {
    if negated { "not " } else { "" }
}
