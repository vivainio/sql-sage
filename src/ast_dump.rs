//! A compact, YAML-like text dump of the AST.
//!
//! ```text
//! Query
//!   body: Select
//!     projection:
//!       - Identifier a
//!     from:
//!       - TableWithJoins {relation: Table {name: t}}
//!     selection: BinaryOp {left: Identifier b, op: Gt, right: Number 1}
//!   limit: Number 10
//! ```
//!
//! Empty fields (`None`, `[]`, `false`) are left out, wrapper nodes such as
//! `Some`, `Value` and `UnnamedExpr` are collapsed, identifiers print as their
//! (dotted) name, and small nodes are inlined as `Name {key: value}`.
//!
//! It is generic: the AST's `Debug` output is parsed into a tree and re-rendered,
//! so new node types need no code here. Meant for people; the shape is not a
//! stable interchange format.

use crate::ast::Statement;

enum Node {
    Struct(String, Vec<(String, Node)>),
    Tuple(String, Vec<Node>),
    List(Vec<Node>),
    Str(String),
    Atom(String),
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// A `"string"` or a `'c'` char literal, as printed by `Debug`.
    fn string(&mut self) -> Option<String> {
        let mut out = Vec::new();
        let quote = self.s[self.i];
        self.i += 1; // opening quote
        loop {
            match *self.s.get(self.i)? {
                c if c == quote => {
                    self.i += 1;
                    return String::from_utf8(out).ok();
                }
                b'\\' => {
                    self.i += 1;
                    match *self.s.get(self.i)? {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'0' => out.push(0),
                        b'u' => {
                            // \u{XXXX}
                            let end = self.s[self.i..].iter().position(|&b| b == b'}')? + self.i;
                            let hex = std::str::from_utf8(&self.s[self.i + 2..end]).ok()?;
                            let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                            out.extend_from_slice(ch.to_string().as_bytes());
                            self.i = end;
                        }
                        c => out.push(c),
                    }
                    self.i += 1;
                }
                c => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn items(&mut self, close: u8) -> Option<Vec<Node>> {
        let mut v = vec![];
        while !self.eat(close) {
            v.push(self.value()?);
            self.eat(b',');
        }
        Some(v)
    }

    fn value(&mut self) -> Option<Node> {
        self.ws();
        match *self.s.get(self.i)? {
            b'"' | b'\'' => Some(Node::Str(self.string()?)),
            b'[' => {
                self.i += 1;
                Some(Node::List(self.items(b']')?))
            }
            b'(' => {
                self.i += 1;
                Some(Node::Tuple(String::new(), self.items(b')')?))
            }
            c if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'+' | b'.') => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'-' | b'+' | b'.')) {
                    self.i += 1;
                }
                let name = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                if self.s.get(self.i) == Some(&b'(') {
                    self.i += 1;
                    Some(Node::Tuple(name, self.items(b')')?))
                } else {
                    self.ws();
                    if self.s.get(self.i) == Some(&b'{') {
                        self.i += 1;
                        let mut fields = vec![];
                        while !self.eat(b'}') {
                            self.ws();
                            let k = self.i;
                            while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                                self.i += 1;
                            }
                            let key = String::from_utf8_lossy(&self.s[k..self.i]).into_owned();
                            if !self.eat(b':') {
                                return None;
                            }
                            fields.push((key, self.value()?));
                            self.eat(b',');
                        }
                        Some(Node::Struct(name, fields))
                    } else {
                        Some(Node::Atom(name))
                    }
                }
            }
            _ => None,
        }
    }
}

/// A node rendered for output: a header plus optional children.
struct R {
    head: String,
    /// `None` key: list item.
    children: Vec<(Option<String>, R)>,
}

fn leaf(head: String) -> R {
    R { head, children: vec![] }
}

const ELIDE: &[&str] = &["Value", "UnnamedExpr"];
const FLOW_WIDTH: usize = 84;

fn scalar(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | ':' | '?' | '@' | '*' | '-' | '+'));
    if plain { s.to_string() } else { format!("{s:?}") }
}

fn ident_text(fields: &[(String, Node)]) -> Option<String> {
    let value = fields.iter().find(|(k, _)| k == "value").and_then(|(_, n)| if let Node::Str(s) = n { Some(s.clone()) } else { None })?;
    let quote = fields.iter().find(|(k, _)| k == "quote_style").and_then(|(_, n)| match n {
        Node::Tuple(name, v) if name == "Some" => v.first().and_then(|n| if let Node::Atom(a) | Node::Str(a) = n { Some(a.clone()) } else { None }),
        _ => None,
    });
    Some(match quote {
        Some(_) => format!("{value:?}"),
        None => value,
    })
}

fn name_of(n: &Node) -> Option<&str> {
    match n {
        Node::Struct(name, _) | Node::Tuple(name, _) => Some(name),
        _ => None,
    }
}

fn render(n: &Node) -> Option<R> {
    match n {
        Node::Atom(a) => match a.as_str() {
            "None" | "false" | "0" => None,
            a => Some(leaf(a.to_string())),
        },
        Node::Str(s) => Some(leaf(scalar(s))),
        Node::List(items) => {
            let children: Vec<_> = items.iter().filter_map(render).map(|r| (None, r)).collect();
            if children.is_empty() {
                return None;
            }
            if children.iter().all(|(_, c)| c.children.is_empty()) {
                let flow = format!("[{}]", children.iter().map(|(_, c)| c.head.as_str()).collect::<Vec<_>>().join(", "));
                if flow.len() <= FLOW_WIDTH {
                    return Some(leaf(flow));
                }
            }
            Some(R { head: String::new(), children })
        }
        Node::Struct(name, fields) if name == "Ident" => ident_text(fields).map(leaf),
        Node::Tuple(name, args) if name == "ObjectName" => match args.first() {
            Some(Node::List(parts)) => {
                let names: Vec<String> = parts.iter().filter_map(render).map(|r| r.head).collect();
                Some(leaf(names.join(".")))
            }
            _ => None,
        },
        Node::Tuple(name, args) if name == "CompoundIdentifier" => match args.first() {
            Some(Node::List(parts)) => {
                let names: Vec<String> = parts.iter().filter_map(render).map(|r| r.head).collect();
                Some(leaf(format!("{name} {}", names.join("."))))
            }
            _ => None,
        },
        Node::Tuple(name, args) if (name == "Some" || ELIDE.contains(&name.as_str())) && args.len() == 1 => render(&args[0]),
        Node::Tuple(name, args) if args.len() == 1 => {
            let r = render(&args[0])?;
            if name_of(&args[0]) == Some(name) {
                Some(r)
            } else if r.children.is_empty() {
                Some(leaf(format!("{name} {}", r.head)))
            } else if r.head.is_empty() {
                Some(R { head: name.clone(), children: r.children })
            } else {
                Some(R { head: format!("{name} > {}", r.head), children: r.children })
            }
        }
        Node::Tuple(name, args) => {
            let children: Vec<_> = args.iter().filter_map(render).map(|r| (None, r)).collect();
            if children.is_empty() {
                return Some(leaf(name.clone()));
            }
            Some(R { head: name.clone(), children })
        }
        Node::Struct(name, fields) => {
            let children: Vec<(Option<String>, R)> =
                fields.iter().filter_map(|(k, v)| render(v).map(|r| (Some(k.clone()), r))).collect();
            if children.iter().all(|(_, c)| c.children.is_empty()) {
                let flow = children
                    .iter()
                    .map(|(k, c)| format!("{}: {}", k.as_deref().unwrap_or(""), c.head))
                    .collect::<Vec<_>>()
                    .join(", ");
                let text = if flow.is_empty() { name.clone() } else { format!("{name} {{{flow}}}") };
                if text.len() <= FLOW_WIDTH {
                    return Some(leaf(text));
                }
            }
            Some(R { head: name.clone(), children })
        }
    }
}

fn emit(r: &R, indent: usize, out: &mut Vec<String>) {
    let pad = " ".repeat(indent);
    for (key, c) in &r.children {
        match key {
            Some(k) if c.head.is_empty() => {
                out.push(format!("{pad}{k}:"));
                emit(c, indent + 2, out);
            }
            Some(k) => {
                out.push(format!("{pad}{k}: {}", c.head));
                emit(c, indent + 2, out);
            }
            None => {
                out.push(format!("{pad}- {}", c.head).trim_end().to_string());
                emit(c, indent + 2, out);
            }
        }
    }
}

/// Renders a statement as compact YAML-like text, or `None` if the AST's debug
/// output could not be parsed (should not happen).
pub fn try_yamlish(stmt: &Statement) -> Option<String> {
    let debug = format!("{stmt:?}");
    let mut p = Parser { s: debug.as_bytes(), i: 0 };
    let node = p.value()?;
    p.ws();
    if p.i != debug.len() {
        return None;
    }
    let r = render(&node)?;
    let mut lines = vec![r.head.clone()];
    emit(&r, 2, &mut lines);
    Some(lines.join("\n"))
}

/// Like [`try_yamlish`], falling back to the pretty `Debug` output.
pub fn yamlish(stmt: &Statement) -> String {
    try_yamlish(stmt).unwrap_or_else(|| format!("{stmt:#?}"))
}
