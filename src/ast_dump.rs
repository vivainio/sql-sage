//! Text dumps of the AST.
//!
//! * [`yaml`]: real YAML (loads with any YAML parser). The node type is a key:
//!
//!   ```text
//!   Query:
//!     body:
//!       Select:
//!         projection: [{Identifier: a}]
//!         from: [{TableWithJoins: {relation: {Table: {name: t}}}}]
//!         selection: {BinaryOp: {left: {Identifier: b}, op: Gt, right: {Number: 1}}}
//!     limit: {Number: 10}
//!   ```
//!
//! * [`compact`]: a shorter *YAML-like* form for reading. **Not valid YAML**
//!   (`Select` is a scalar with children).
//!
//!   ```text
//!   Query
//!     body: Select
//!       projection: [Identifier a]
//!       selection: BinaryOp {left: Identifier b, op: Gt, right: Number 1}
//!     limit: Number 10
//!   ```
//!
//! Both leave out empty fields (`None`, `[]`, `false`, zero counts), collapse
//! wrapper nodes such as `Some`, `Value` and `UnnamedExpr`, and print identifiers
//! as their (dotted) name.
//!
//! They are generic: the AST's `Debug` output is parsed into a tree and
//! re-rendered, so new node types need no code here. The shape follows the AST
//! and is not a stable interchange format.

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
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'-' | b'+' | b'.'))
                {
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
                            while self.i < self.s.len()
                                && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                            {
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
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | ':' | '?' | '@' | '*' | '-' | '+'));
    if plain {
        s.to_string()
    } else {
        format!("{s:?}")
    }
}

fn ident_text(fields: &[(String, Node)]) -> Option<String> {
    let value = fields.iter().find(|(k, _)| k == "value").and_then(|(_, n)| {
        if let Node::Str(s) = n {
            Some(s.clone())
        } else {
            None
        }
    })?;
    let quote = fields.iter().find(|(k, _)| k == "quote_style").and_then(|(_, n)| match n {
        Node::Tuple(name, v) if name == "Some" => {
            v.first().and_then(|n| if let Node::Atom(a) | Node::Str(a) = n { Some(a.clone()) } else { None })
        }
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
                let flow =
                    format!("[{}]", children.iter().map(|(_, c)| c.head.as_str()).collect::<Vec<_>>().join(", "));
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
        Node::Tuple(name, args) if (name == "Some" || ELIDE.contains(&name.as_str())) && args.len() == 1 => {
            render(&args[0])
        }
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

fn parse_debug(stmt: &Statement) -> Option<Node> {
    let debug = format!("{stmt:?}");
    let mut p = Parser { s: debug.as_bytes(), i: 0 };
    let node = p.value()?;
    p.ws();
    (p.i == debug.len()).then_some(node)
}

/// Renders a statement as the compact YAML-*like* text (not valid YAML), or
/// `None` if the AST's debug output could not be parsed (should not happen).
pub fn try_compact(stmt: &Statement) -> Option<String> {
    let node = parse_debug(stmt)?;
    let r = render(&node)?;
    let mut lines = vec![r.head.clone()];
    emit(&r, 2, &mut lines);
    Some(lines.join("\n"))
}

/// Like [`try_compact`], falling back to the pretty `Debug` output.
pub fn compact(stmt: &Statement) -> String {
    try_compact(stmt).unwrap_or_else(|| format!("{stmt:#?}"))
}

// ------------------------------------------------------------- real YAML

enum Y {
    Scalar(String),
    Seq(Vec<Y>),
    Map(Vec<(String, Y)>),
}

/// A plain YAML scalar only when no YAML 1.1 / 1.2 loader could read it as
/// anything but a string; otherwise a double-quoted (JSON-style) string.
fn yaml_str(s: &str) -> String {
    let mut chars = s.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '-' | '+' | '*' | '?' | '@'))
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null"
        );
    if plain {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn plain_number(s: &str) -> bool {
    let (int, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    !int.is_empty()
        && int.chars().all(|c| c.is_ascii_digit())
        && (int == "0" || !int.starts_with('0'))
        && frac.is_none_or(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()))
}

fn ident_raw(n: &Node) -> Option<(String, bool)> {
    let Node::Struct(name, fields) = n else { return None };
    if name != "Ident" {
        return None;
    }
    let value = fields.iter().find(|(k, _)| k == "value").and_then(|(_, n)| {
        if let Node::Str(s) = n {
            Some(s.clone())
        } else {
            None
        }
    })?;
    let quoted = fields.iter().any(|(k, n)| k == "quote_style" && matches!(n, Node::Tuple(t, _) if t == "Some"));
    Some((value, quoted))
}

fn ident_y(raw: (String, bool)) -> Y {
    let (value, quoted) = raw;
    if quoted {
        Y::Map(vec![("Quoted".into(), Y::Scalar(yaml_str(&value)))])
    } else {
        Y::Scalar(yaml_str(&value))
    }
}

/// `a.b.c` as one scalar, or a sequence when a part is quoted.
fn dotted_y(parts: &[Node]) -> Option<Y> {
    let raws: Vec<(String, bool)> = parts.iter().filter_map(ident_raw).collect();
    if raws.is_empty() {
        return None;
    }
    if raws.iter().any(|(_, q)| *q) {
        return Some(Y::Seq(raws.into_iter().map(ident_y).collect()));
    }
    Some(Y::Scalar(yaml_str(&raws.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join("."))))
}

fn to_y(n: &Node) -> Option<Y> {
    match n {
        Node::Atom(a) => match a.as_str() {
            "None" | "false" | "0" => None,
            "true" => Some(Y::Scalar("true".into())),
            a if a.chars().all(|c| c.is_ascii_digit()) => Some(Y::Scalar(a.to_string())),
            a => Some(Y::Scalar(yaml_str(a))),
        },
        Node::Str(s) => Some(Y::Scalar(yaml_str(s))),
        Node::List(items) => {
            let v: Vec<Y> = items.iter().filter_map(to_y).collect();
            (!v.is_empty()).then_some(Y::Seq(v))
        }
        Node::Struct(name, fields) if name == "Ident" => {
            ident_raw(&Node::Struct(name.clone(), fields.iter().map(|(k, v)| (k.clone(), clone_node(v))).collect()))
                .map(ident_y)
        }
        Node::Tuple(name, args) if name == "ObjectName" => match args.first() {
            Some(Node::List(parts)) => dotted_y(parts),
            _ => None,
        },
        Node::Tuple(name, args) if name == "CompoundIdentifier" => match args.first() {
            Some(Node::List(parts)) => Some(Y::Map(vec![(name.clone(), dotted_y(parts)?)])),
            _ => None,
        },
        Node::Tuple(name, args) if name == "Number" => match args.first() {
            Some(Node::Str(s)) => {
                Some(Y::Map(vec![(name.clone(), Y::Scalar(if plain_number(s) { s.clone() } else { yaml_str(s) }))]))
            }
            _ => None,
        },
        Node::Tuple(name, args) if (name == "Some" || ELIDE.contains(&name.as_str())) && args.len() == 1 => {
            to_y(&args[0])
        }
        Node::Tuple(name, args) if args.len() == 1 => {
            if name_of(&args[0]) == Some(name) {
                return to_y(&args[0]);
            }
            Some(match to_y(&args[0]) {
                Some(y) => Y::Map(vec![(name.clone(), y)]),
                None => Y::Scalar(yaml_str(name)),
            })
        }
        Node::Tuple(name, args) => {
            let v: Vec<Y> = args.iter().filter_map(to_y).collect();
            Some(if v.is_empty() { Y::Scalar(yaml_str(name)) } else { Y::Map(vec![(name.clone(), Y::Seq(v))]) })
        }
        Node::Struct(name, fields) => {
            let f: Vec<(String, Y)> = fields.iter().filter_map(|(k, v)| to_y(v).map(|y| (k.clone(), y))).collect();
            Some(if f.is_empty() { Y::Scalar(yaml_str(name)) } else { Y::Map(vec![(name.clone(), Y::Map(f))]) })
        }
    }
}

fn clone_node(n: &Node) -> Node {
    match n {
        Node::Struct(a, f) => Node::Struct(a.clone(), f.iter().map(|(k, v)| (k.clone(), clone_node(v))).collect()),
        Node::Tuple(a, v) => Node::Tuple(a.clone(), v.iter().map(clone_node).collect()),
        Node::List(v) => Node::List(v.iter().map(clone_node).collect()),
        Node::Str(s) => Node::Str(s.clone()),
        Node::Atom(s) => Node::Atom(s.clone()),
    }
}

fn flow(y: &Y) -> String {
    match y {
        Y::Scalar(s) => s.clone(),
        Y::Seq(v) => format!("[{}]", v.iter().map(flow).collect::<Vec<_>>().join(", ")),
        Y::Map(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, v)| format!("{}: {}", yaml_str(k), flow(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn emit_y(y: &Y, indent: usize, out: &mut Vec<String>) {
    let pad = " ".repeat(indent);
    match y {
        Y::Scalar(s) => out.push(format!("{pad}{s}")),
        Y::Map(entries) => {
            for (k, v) in entries {
                let key = yaml_str(k);
                let f = flow(v);
                if matches!(v, Y::Scalar(_)) || indent + key.len() + 2 + f.len() <= FLOW_WIDTH {
                    out.push(format!("{pad}{key}: {f}"));
                } else {
                    out.push(format!("{pad}{key}:"));
                    emit_y(v, indent + 2, out);
                }
            }
        }
        Y::Seq(items) => {
            for it in items {
                let f = flow(it);
                if matches!(it, Y::Scalar(_)) || indent + 2 + f.len() <= FLOW_WIDTH {
                    out.push(format!("{pad}- {f}"));
                } else {
                    let mut sub = vec![];
                    emit_y(it, indent + 2, &mut sub);
                    out.push(format!("{pad}- {}", &sub[0][indent + 2..]));
                    out.extend(sub.into_iter().skip(1));
                }
            }
        }
    }
}

/// Renders a statement as YAML (one document), or `None` if the AST's debug
/// output could not be parsed (should not happen).
pub fn try_yaml(stmt: &Statement) -> Option<String> {
    let y = to_y(&parse_debug(stmt)?)?;
    let mut lines = vec![];
    emit_y(&y, 0, &mut lines);
    Some(lines.join("\n"))
}

/// Like [`try_yaml`], falling back to the pretty `Debug` output (not YAML).
pub fn yaml(stmt: &Statement) -> String {
    try_yaml(stmt).unwrap_or_else(|| format!("{stmt:#?}"))
}
