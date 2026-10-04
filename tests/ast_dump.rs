use sql_sage::ast_dump::{compact, try_compact, try_yaml, yaml};
use yaml_rust2::{Yaml, YamlLoader};
use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::parse_sql;

const CORPUS: &[&str] = &[
    "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 5) SELECT t.*, count(*) OVER (PARTITION BY t.k ORDER BY t.v), CASE WHEN x IS NULL THEN 0 ELSE 1 END FROM t LEFT JOIN u ON t.id = u.id JOIN (SELECT 1 AS id) s USING (id) WHERE x NOT IN (1, 2) AND y BETWEEN 1 AND 2 AND NOT EXISTS (SELECT 1 FROM z) GROUP BY t.k HAVING count(DISTINCT t.v) > 1 ORDER BY 1 DESC NULLS LAST LIMIT 5 OFFSET 2",
    "SELECT a FROM t UNION SELECT a FROM u INTERSECT SELECT a FROM v",
    r#"SELECT "Mixed Case", "say ""hi""", [br acket], `tick`, 'it''s' FROM "S"."T""#,
    "SELECT DISTINCT ON (a) a, DATE '2020-01-01', a::int[], data->>'k', $1, E'x\\ny', $$q$$ FROM t",
    "INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL) ON CONFLICT (a) DO UPDATE SET b = excluded.b WHERE t.b IS NULL RETURNING a, b",
    "INSERT INTO t DEFAULT VALUES",
    "UPDATE t SET a = 1, b = b + 1 FROM u WHERE t.id = u.id RETURNING *",
    "DELETE FROM t WHERE id IN (SELECT id FROM u)",
    "CREATE TEMP TABLE IF NOT EXISTS t (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, n varchar(10) NOT NULL DEFAULT 'x' CHECK (n <> ''), r int REFERENCES u(id) ON DELETE CASCADE, CONSTRAINT pk UNIQUE (n), FOREIGN KEY (r) REFERENCES u(id))",
    "CREATE UNIQUE INDEX IF NOT EXISTS i ON t (a, b DESC) WHERE a > 0",
    "DROP TABLE IF EXISTS a, b CASCADE",
    "SELECT * FROM ((a JOIN b ON a.id = b.id)) NATURAL JOIN c, d",
];

fn dump(kind: DialectKind, sql: &str) -> String {
    let stmts = parse_sql(kind.dialect().as_ref(), sql).unwrap();
    stmts.iter().map(compact).collect::<Vec<_>>().join("\n---\n")
}

#[test]
fn compact_output() {
    assert_eq!(
        dump(Sqlite, "SELECT a FROM t WHERE b > 1 LIMIT 5, 10"),
        "Query\n\
         \x20 body: Select\n\
         \x20   projection: [Identifier a]\n\
         \x20   from: [TableWithJoins {relation: Table {name: t}}]\n\
         \x20   selection: BinaryOp {left: Identifier b, op: Gt, right: Number 1}\n\
         \x20 limit: Number 10\n\
         \x20 offset: Number 5"
    );
}

#[test]
fn empty_fields_and_wrappers_are_dropped() {
    let out = dump(Postgres, "SELECT a::int FROM public.t x");
    assert!(!out.contains("None"), "{out}");
    assert!(!out.contains("Some"), "{out}");
    assert!(!out.contains("UnnamedExpr"), "{out}");
    assert!(!out.contains("[]"), "{out}");
    assert!(!out.contains("array_dims"), "{out}");
    assert!(out.contains("Table {name: public.t, alias: TableAlias {name: x}}"), "{out}");
}

#[test]
fn identifiers_and_strings() {
    let out = dump(Postgres, r#"SELECT "Mixed Case", 'it''s "q" é 日本', a.b.c FROM t"#);
    assert!(out.contains(r#""Mixed Case""#), "{out}");
    assert!(out.contains(r#"String "it's \"q\" é 日本""#), "{out}");
    assert!(out.contains("CompoundIdentifier a.b.c"), "{out}");
}

#[test]
fn nested_blocks_use_yaml_style_lists() {
    let out = dump(Postgres, "SELECT 1 FROM t WHERE EXISTS (SELECT 1 FROM u WHERE u.id = t.id)");
    assert!(out.contains("\n      right: Exists") || out.contains("selection: Exists"), "{out}");
    assert!(out.contains("subquery: Query"), "{out}");
    assert_eq!(dump(Sqlite, "SELECT 1; SELECT 2").matches("\n---\n").count(), 1);
}

/// The dump is derived from `Debug` output; it must never fail to parse it.
#[test]
fn every_statement_shape_dumps_without_fallback() {
    for sql in CORPUS {
        for kind in [Postgres, Sqlite, Oracle] {
            let Ok(stmts) = parse_sql(kind.dialect().as_ref(), sql) else { continue };
            for s in &stmts {
                let out = try_compact(s).unwrap_or_else(|| panic!("{kind:?}: could not dump {sql}"));
                assert!(!out.is_empty());
            }
        }
    }
}


fn load(doc: &str) -> Vec<Yaml> {
    YamlLoader::load_from_str(doc).unwrap_or_else(|e| panic!("not valid YAML: {e}\n{doc}"))
}

/// `--ast` output must load with a real YAML parser, for every statement shape.
#[test]
fn yaml_is_valid_for_every_statement_shape() {
    let mut checked = 0;
    for sql in CORPUS {
        for kind in [Postgres, Sqlite, Oracle] {
            let Ok(stmts) = parse_sql(kind.dialect().as_ref(), sql) else { continue };
            for s in &stmts {
                let out = try_yaml(s).unwrap_or_else(|| panic!("{kind:?}: could not dump {sql}"));
                let docs = load(&out);
                assert_eq!(docs.len(), 1, "{out}");
                assert!(docs[0].as_hash().is_some_and(|h| h.len() == 1), "root must be one `Type:` key\n{out}");
                checked += 1;
            }
        }
    }
    assert!(checked > 20);
    // several statements form a multi-document stream
    let stmts = parse_sql(Sqlite.dialect().as_ref(), "SELECT 1; DELETE FROM t; DROP TABLE t").unwrap();
    let stream = stmts.iter().map(yaml).collect::<Vec<_>>().join("\n---\n");
    assert_eq!(load(&stream).len(), 3);
}

#[test]
fn yaml_output() {
    let stmts = parse_sql(Sqlite.dialect().as_ref(), "SELECT a FROM t WHERE b > 1 LIMIT 5, 10").unwrap();
    assert_eq!(
        yaml(&stmts[0]),
        "Query:\n\
         \x20 body:\n\
         \x20   Select:\n\
         \x20     projection: [{Identifier: a}]\n\
         \x20     from: [{TableWithJoins: {relation: {Table: {name: t}}}}]\n\
         \x20     selection: {BinaryOp: {left: {Identifier: b}, op: Gt, right: {Number: 1}}}\n\
         \x20 limit: {Number: 10}\n\
         \x20 offset: {Number: 5}"
    );
}

/// Strings that YAML would reinterpret must stay strings; numbers and booleans stay typed.
#[test]
fn yaml_preserves_types() {
    let sql = "SELECT 'true', 'no', 'null', 'a: b', '1', '', '2020-01-01', '#x', 'it''s \"q\" é 日本', 007, 1.5, TRUE, \
               \"Mixed Case\", s.t.c, :id FROM u JOIN v ON u.id = v.id";
    let stmts = parse_sql(Sqlite.dialect().as_ref(), sql).unwrap();
    let docs = load(&yaml(&stmts[0]));
    let select = &docs[0]["Query"]["body"]["Select"];
    let p = select["projection"].as_vec().unwrap();
    let text = |i: usize| p[i]["String"].as_str().map(str::to_string);
    assert_eq!(text(0).as_deref(), Some("true"));
    assert_eq!(text(1).as_deref(), Some("no"));
    assert_eq!(text(2).as_deref(), Some("null"));
    assert_eq!(text(3).as_deref(), Some("a: b"));
    assert_eq!(text(4).as_deref(), Some("1"));
    assert_eq!(text(5).as_deref(), Some(""));
    assert_eq!(text(6).as_deref(), Some("2020-01-01"));
    assert_eq!(text(7).as_deref(), Some("#x"));
    assert_eq!(text(8).as_deref(), Some("it's \"q\" é 日本"));
    assert_eq!(p[9]["Number"].as_str(), Some("007")); // not a plain decimal: kept as text
    assert_eq!(p[10]["Number"].as_f64(), Some(1.5));
    assert_eq!(p[11]["Boolean"].as_bool(), Some(true));
    assert_eq!(p[12]["Identifier"]["Quoted"].as_str(), Some("Mixed Case"));
    assert_eq!(p[13]["CompoundIdentifier"].as_str(), Some("s.t.c"));
    assert_eq!(p[14]["Placeholder"].as_str(), Some(":id"));
    // `On` must stay a string key (YAML 1.1 reads a bare `On` as a boolean)
    let join = &select["from"][0]["TableWithJoins"]["joins"][0]["Join"];
    assert!(join["constraint"]["On"]["BinaryOp"]["op"].as_str() == Some("Eq"), "{join:?}");
}
