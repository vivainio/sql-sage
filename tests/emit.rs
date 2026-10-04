use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::{check_syntax, transpile, TranspileError};

fn t(from: DialectKind, to: DialectKind, sql: &str) -> String {
    transpile(from, to, sql).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn err(from: DialectKind, to: DialectKind, sql: &str) -> String {
    match transpile(from, to, sql) {
        Err(TranspileError::Emit(e)) => e.message,
        other => panic!("expected emit error for {sql}, got {other:?}"),
    }
}

#[test]
fn pagination() {
    assert_eq!(
        t(Postgres, Oracle, "SELECT a FROM t LIMIT 10 OFFSET 5"),
        "SELECT a FROM t OFFSET 5 ROWS FETCH FIRST 10 ROWS ONLY;"
    );
    assert_eq!(
        t(Oracle, Postgres, "SELECT a FROM t OFFSET 5 ROWS FETCH NEXT 10 ROWS ONLY"),
        "SELECT a FROM t LIMIT 10 OFFSET 5;"
    );
    assert_eq!(t(Oracle, Sqlite, "SELECT a FROM t FETCH FIRST ROW ONLY"), "SELECT a FROM t LIMIT 1;");
    assert_eq!(t(Postgres, Sqlite, "SELECT a FROM t OFFSET 5"), "SELECT a FROM t LIMIT -1 OFFSET 5;");
    assert_eq!(
        t(Sqlite, Oracle, "SELECT a FROM t LIMIT 5, 10"),
        "SELECT a FROM t OFFSET 5 ROWS FETCH FIRST 10 ROWS ONLY;"
    );
    assert!(err(Oracle, Sqlite, "SELECT a FROM t FETCH FIRST 5 ROWS WITH TIES").contains("sqlite"));
}

#[test]
fn set_ops_and_dual() {
    assert_eq!(t(Postgres, Oracle, "SELECT 1 EXCEPT SELECT 2"), "SELECT 1 FROM DUAL MINUS SELECT 2 FROM DUAL;");
    assert_eq!(t(Oracle, Postgres, "SELECT 1 FROM dual MINUS SELECT 2 FROM dual"), "SELECT 1 EXCEPT SELECT 2;");
    assert_eq!(t(Postgres, Oracle, "SELECT 1 UNION DISTINCT SELECT 2"), "SELECT 1 FROM DUAL UNION SELECT 2 FROM DUAL;");
}

#[test]
fn placeholders() {
    assert_eq!(t(Postgres, Oracle, "SELECT $1, $2, $1"), "SELECT :1, :2, :1 FROM DUAL;");
    assert_eq!(t(Sqlite, Postgres, "SELECT ?, ?, ?3"), "SELECT $1, $2, $3;");
    assert_eq!(t(Oracle, Sqlite, "SELECT :id FROM dual"), "SELECT :id;");
    assert!(err(Oracle, Postgres, "SELECT :id FROM dual").contains("bind parameter"));
}

#[test]
fn expressions() {
    assert_eq!(
        t(Postgres, Sqlite, "SELECT a ILIKE 'x%', b::int FROM t"),
        "SELECT LOWER(a) LIKE LOWER('x%'), CAST(b AS INT) FROM t;"
    );
    assert_eq!(t(Postgres, Oracle, "SELECT a % 3, true FROM t"), "SELECT MOD(a, 3), 1 FROM t;");
    assert_eq!(t(Postgres, Sqlite, "SELECT DATE '2020-01-01'"), "SELECT '2020-01-01';");
    assert_eq!(t(Sqlite, Oracle, "SELECT a FROM t x WHERE [my col] = 1"), "SELECT a FROM t x WHERE \"my col\" = 1;");
    assert!(err(Sqlite, Postgres, "SELECT a GLOB 'x*' FROM t").contains("GLOB"));
    assert!(err(Postgres, Oracle, "SELECT data->>'k' FROM t").contains("JSON"));
    assert!(err(Postgres, Sqlite, "SELECT INTERVAL '1 day'").contains("INTERVAL"));
}

#[test]
fn dml() {
    assert_eq!(
        t(Sqlite, Postgres, "INSERT OR IGNORE INTO t VALUES (1)"),
        "INSERT INTO t VALUES (1) ON CONFLICT DO NOTHING;"
    );
    assert!(err(Sqlite, Postgres, "INSERT OR REPLACE INTO t VALUES (1)").contains("INSERT OR REPLACE"));
    assert!(err(Postgres, Oracle, "INSERT INTO t VALUES (1) RETURNING id").contains("RETURNING"));
    assert!(err(Postgres, Oracle, "INSERT INTO t VALUES (1), (2)").contains("multi-row"));
    assert!(err(Postgres, Oracle, "UPDATE t SET a = 1 FROM u WHERE t.id = u.id").contains("UPDATE ... FROM"));
}

#[test]
fn ddl_and_types() {
    assert_eq!(
        t(
            Oracle,
            Postgres,
            "CREATE TABLE t (id NUMBER(10,2) NOT NULL, n VARCHAR2(100 BYTE), c CLOB, d DATE DEFAULT SYSDATE)"
        ),
        "CREATE TABLE t (id NUMERIC(10, 2) NOT NULL, n VARCHAR(100), c TEXT, d DATE DEFAULT SYSDATE);"
    );
    assert_eq!(
        t(Postgres, Oracle, "CREATE TABLE t (id bigint NOT NULL DEFAULT 0, name text, ok boolean, v varchar)"),
        "CREATE TABLE t (id NUMBER(19) DEFAULT 0 NOT NULL, name CLOB, ok NUMBER(1), v VARCHAR2(4000));"
    );
    assert_eq!(t(Postgres, Oracle, "CREATE TEMP TABLE t (a int)"), "CREATE GLOBAL TEMPORARY TABLE t (a INT);");
    assert_eq!(
        t(Postgres, Oracle, "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM c WHERE n<3) SELECT n FROM c"),
        "WITH c (n) AS (SELECT 1 FROM DUAL UNION ALL SELECT n + 1 FROM c WHERE n < 3) SELECT n FROM c;"
    );
    assert!(err(Postgres, Oracle, "DROP TABLE IF EXISTS t").contains("IF EXISTS"));
    assert!(err(Sqlite, Postgres, "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT)").contains("AUTOINCREMENT"));
    assert!(err(Postgres, Sqlite, "CREATE TABLE t (id int GENERATED ALWAYS AS IDENTITY)").contains("identity"));
    assert!(err(Sqlite, Postgres, "CREATE TABLE t (a, b)").contains("without a type"));
    assert!(err(Postgres, Sqlite, "CREATE TABLE t (a int[])").contains("array"));
    assert!(err(Postgres, Oracle, "CREATE INDEX i ON t (a) WHERE a > 0").contains("partial"));
}

/// Whatever we emit for a dialect must be valid syntax in that dialect.
#[test]
fn emitted_sql_reparses_in_target() {
    let corpus = [
        "SELECT a, b FROM t WHERE c = 1 ORDER BY a LIMIT 5",
        "SELECT t.*, count(*) OVER (PARTITION BY t.k ORDER BY t.v) FROM t LEFT JOIN u ON t.id = u.id",
        "WITH c AS (SELECT 1) SELECT * FROM c",
        "SELECT CASE WHEN x IS NULL THEN 0 ELSE 1 END, y FROM (SELECT 1 AS x, 2 AS y) s",
        "SELECT a FROM t WHERE a NOT IN (1, 2) AND b BETWEEN 1 AND 2 AND NOT EXISTS (SELECT 1 FROM z)",
        "INSERT INTO t (a, b) VALUES (1, 'x')",
        "UPDATE t SET a = a + 1 WHERE b = 2",
        "DELETE FROM t WHERE a = 1",
        "CREATE TABLE t (id INTEGER NOT NULL DEFAULT 1, name VARCHAR(10) UNIQUE, PRIMARY KEY (id))",
        "CREATE UNIQUE INDEX i ON t (a, b DESC)",
        "DROP TABLE t",
        "SELECT 1 UNION ALL SELECT 2",
    ];
    let all = [Postgres, Sqlite, Oracle];
    let mut emitted = 0;
    for from in all {
        for sql in corpus {
            if check_syntax(from, sql).is_err() {
                continue;
            }
            for to in all {
                let out = transpile(from, to, sql).unwrap_or_else(|e| panic!("{from:?}->{to:?} {sql}: {e}"));
                check_syntax(to, &out).unwrap_or_else(|e| panic!("{from:?}->{to:?}\n in: {sql}\nout: {out}\n{e}"));
                // and it is a fixed point: re-emitting for the same target is stable
                assert_eq!(transpile(to, to, &out).unwrap(), out, "{to:?}: {out}");
                emitted += 1;
            }
        }
    }
    assert!(emitted > 30);
}
