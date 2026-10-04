use sql_sage::ast::*;
use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::{check_syntax, parse_sql};

fn norm(kind: DialectKind, sql: &str) -> String {
    let stmts = parse_sql(kind.dialect().as_ref(), sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    stmts.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("; ")
}

fn ast(kind: DialectKind, sql: &str) -> Vec<Statement> {
    parse_sql(kind.dialect().as_ref(), sql).unwrap()
}

#[test]
fn postgres_features() {
    assert_eq!(
        norm(Postgres, "SELECT a::int, b FROM t WHERE c ILIKE '%x%' AND d = $1 LIMIT 10 OFFSET 5"),
        "SELECT CAST(a AS INT), b FROM t WHERE c ILIKE '%x%' AND d = $1 LIMIT 10 OFFSET 5"
    );
    assert_eq!(
        norm(Postgres, "SELECT DISTINCT ON (a) a, b FROM t ORDER BY a, b DESC NULLS LAST"),
        "SELECT DISTINCT ON (a) a, b FROM t ORDER BY a, b DESC NULLS LAST"
    );
    assert_eq!(
        norm(Postgres, "SELECT $$it's$$, E'a\\nb', data->>'k' FROM t"),
        "SELECT 'it''s', 'a\nb', data ->> 'k' FROM t"
    );
    assert_eq!(
        norm(Postgres, "INSERT INTO t (a) VALUES (1) ON CONFLICT (a) DO UPDATE SET a = excluded.a RETURNING id"),
        "INSERT INTO t (a) VALUES (1) ON CONFLICT (a) DO UPDATE SET a = excluded.a RETURNING id"
    );
    assert_eq!(
        norm(Postgres, "CREATE TABLE t (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, ts timestamp(3) with time zone, tags text[])"),
        "CREATE TABLE t (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, ts TIMESTAMP(3) WITH TIME ZONE, tags TEXT[])"
    );
    // nested block comments
    assert_eq!(norm(Postgres, "SELECT /* a /* b */ c */ 1"), "SELECT 1");
}

#[test]
fn sqlite_features() {
    assert_eq!(norm(Sqlite, "SELECT * FROM t LIMIT 5, 10"), "SELECT * FROM t LIMIT 10 OFFSET 5");
    assert_eq!(
        norm(Sqlite, "SELECT [a b], `c` FROM t WHERE x = ?1 AND y GLOB :p OR z = @q"),
        "SELECT [a b], `c` FROM t WHERE x = ?1 AND y GLOB :p OR z = @q"
    );
    assert_eq!(norm(Sqlite, "INSERT OR REPLACE INTO t VALUES (1, 'a')"), "INSERT OR REPLACE INTO t VALUES (1, 'a')");
    assert_eq!(
        norm(Sqlite, "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, blob_col, name TEXT NOT NULL DEFAULT 'x')"),
        "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, blob_col, name TEXT NOT NULL DEFAULT 'x')"
    );
}

#[test]
fn oracle_features() {
    assert_eq!(
        norm(Oracle, "SELECT e.name FROM emp e WHERE e.id = :id ORDER BY e.name OFFSET 5 ROWS FETCH NEXT 10 ROWS ONLY"),
        "SELECT e.name FROM emp AS e WHERE e.id = :id ORDER BY e.name OFFSET 5 FETCH FIRST 10 ROWS ONLY"
    );
    assert_eq!(norm(Oracle, "SELECT q'[it's]' FROM dual"), "SELECT 'it''s' FROM dual");
    assert_eq!(
        norm(Oracle, "SELECT 1 FROM dual MINUS SELECT 2 FROM dual"),
        "SELECT 1 FROM dual EXCEPT SELECT 2 FROM dual"
    );
    assert_eq!(
        norm(Oracle, "CREATE TABLE t (id NUMBER(10,2) NOT NULL, name VARCHAR2(100 BYTE), d DATE DEFAULT SYSDATE)"),
        "CREATE TABLE t (id NUMBER(10, 2) NOT NULL, name VARCHAR2(100 BYTE), d DATE DEFAULT SYSDATE)"
    );
    assert_eq!(norm(Oracle, "SELECT DATE '2020-01-01' FROM dual"), "SELECT DATE '2020-01-01' FROM dual");
    assert_eq!(norm(Oracle, "DELETE emp WHERE id = 1"), "DELETE FROM emp WHERE id = 1");
}

#[test]
fn shared_ast_across_dialects() {
    // same logical query => identical AST regardless of engine-specific spelling
    assert_eq!(
        ast(Oracle, "SELECT a FROM t MINUS SELECT a FROM u"),
        ast(Postgres, "SELECT a FROM t EXCEPT SELECT a FROM u")
    );
    assert_eq!(ast(Sqlite, "SELECT a FROM t LIMIT 5, 10"), ast(Postgres, "SELECT a FROM t LIMIT 10 OFFSET 5"));
    assert_eq!(ast(Postgres, "SELECT a::int FROM t"), ast(Sqlite, "SELECT CAST(a AS int) FROM t"));
}

#[test]
fn common_sql() {
    let sql = "WITH RECURSIVE c (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 5) \
               SELECT t.*, count(*) OVER (PARTITION BY t.k ORDER BY t.v), \
               CASE WHEN x IS NULL THEN 0 ELSE 1 END \
               FROM t LEFT JOIN u ON t.id = u.id JOIN (SELECT 1 AS id) s USING (id) \
               WHERE x NOT IN (1, 2) AND y BETWEEN 1 AND 2 AND NOT EXISTS (SELECT 1 FROM z) \
               GROUP BY t.k HAVING count(DISTINCT t.v) > 1";
    for k in [Postgres, Sqlite, Oracle] {
        let once = norm(k, sql);
        // normalized output must re-parse to the same AST
        assert_eq!(ast(k, sql), ast(k, &once), "{k:?}: {once}");
    }
}

#[test]
fn precedence() {
    let Statement::Query(q) = &ast(Postgres, "SELECT a OR b AND c")[0] else { panic!() };
    let SetExpr::Select(s) = &q.body else { panic!() };
    let SelectItem::UnnamedExpr(Expr::BinaryOp { op, right, .. }) = &s.projection[0] else { panic!() };
    assert_eq!(*op, BinaryOperator::Or);
    assert!(matches!(**right, Expr::BinaryOp { op: BinaryOperator::And, .. }));
    assert_eq!(norm(Postgres, "SELECT 1 + 2 * 3 - -4"), "SELECT 1 + 2 * 3 - -4");
}

#[test]
fn dialect_specific_syntax_is_rejected_elsewhere() {
    assert!(check_syntax(Oracle, "SELECT * FROM t LIMIT 5").is_err());
    assert!(check_syntax(Postgres, "SELECT 1 FROM t MINUS SELECT 2 FROM t").is_err());
    assert!(check_syntax(Oracle, "SELECT a::int FROM t").is_err());
    assert!(check_syntax(Oracle, "INSERT INTO t VALUES (1) RETURNING id").is_err());
    assert!(check_syntax(Postgres, "SELECT ?1").is_err());
    assert!(check_syntax(Postgres, "CREATE TABLE t (a, b)").is_err());
    assert!(check_syntax(Sqlite, "SELECT a ILIKE 'x'").is_err());
}

#[test]
fn errors_have_locations() {
    let e = check_syntax(Postgres, "SELECT a\nFROM t\nWHERE").unwrap_err();
    assert_eq!(e.location.as_ref().unwrap().line, 3, "{e}");
    let e = check_syntax(Postgres, "SELECT 'abc").unwrap_err();
    assert!(e.message.contains("unterminated"), "{e}");
    assert!(check_syntax(Postgres, "SELECT FROM").is_err());
    assert!(check_syntax(Postgres, "SELEC 1").is_err());
    assert!(check_syntax(Postgres, "SELECT (1").is_err());
}

#[test]
fn multiple_statements() {
    assert_eq!(ast(Sqlite, "SELECT 1;; SELECT 2; DROP TABLE IF EXISTS t CASCADE;").len(), 3);
    assert_eq!(ast(Postgres, "  -- nothing\n").len(), 0);
}
