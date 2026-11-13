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

#[test]
fn trim_with_side_or_characters() {
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM(LEADING '0' FROM a) FROM t"), "SELECT LTRIM(a, '0') FROM t;");
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM(TRAILING ' ' FROM a) FROM t"), "SELECT RTRIM(a, ' ') FROM t;");
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM(BOTH 'x' FROM a) FROM t"), "SELECT TRIM(a, 'x') FROM t;");
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM('x' FROM a) FROM t"), "SELECT TRIM(a, 'x') FROM t;");
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM(LEADING FROM a) FROM t"), "SELECT LTRIM(a) FROM t;");
    // the operands are translated too, and may hold calls and nested trims
    assert_eq!(
        t(Oracle, Sqlite, "SELECT TRIM(LEADING '0' FROM TRIM(NVL(a, ' '))) FROM t"),
        "SELECT LTRIM(TRIM(COALESCE(a, ' ')), '0') FROM t;"
    );
    // other targets keep the standard spelling
    assert_eq!(
        t(Oracle, Postgres, "SELECT trim(leading '0' from a) FROM t"),
        "SELECT TRIM(LEADING '0' FROM a) FROM t;"
    );
    assert_eq!(t(Oracle, Oracle, "SELECT TRIM('x' FROM a) FROM t"), "SELECT TRIM('x' FROM a) FROM t;");
}

#[test]
fn plain_trim_is_an_ordinary_call() {
    assert_eq!(t(Oracle, Sqlite, "SELECT TRIM(a) FROM t"), "SELECT TRIM(a) FROM t;");
    assert_eq!(t(Sqlite, Sqlite, "SELECT trim(a, 'x') FROM t"), "SELECT trim(a, 'x') FROM t;");
    assert!(check_syntax(Oracle, "SELECT TRIM(LEADING '0' FROM a) FROM t").is_ok());
    assert!(check_syntax(Oracle, "SELECT TRIM(LEADING '0' a) FROM t").is_err());
}

#[test]
fn rownum_becomes_limit() {
    assert_eq!(t(Oracle, Sqlite, "SELECT a FROM t WHERE x = 1 AND ROWNUM = 1"), "SELECT a FROM t WHERE x = 1 LIMIT 1;");
    assert_eq!(
        t(Oracle, Sqlite, "SELECT a FROM t WHERE ROWNUM <= 5 AND b = 2"),
        "SELECT a FROM t WHERE b = 2 LIMIT 5;"
    );
    assert_eq!(t(Oracle, Postgres, "SELECT a FROM t WHERE rownum < 3"), "SELECT a FROM t LIMIT 2;");
    assert_eq!(t(Oracle, Sqlite, "SELECT a FROM t WHERE 10 >= ROWNUM"), "SELECT a FROM t LIMIT 10;");
    // a ROWNUM of a subquery limits the subquery
    assert_eq!(
        t(Oracle, Sqlite, "SELECT a FROM (SELECT b AS a FROM u WHERE ROWNUM = 1) WHERE a = 2"),
        "SELECT a FROM (SELECT b AS a FROM u LIMIT 1) WHERE a = 2;"
    );
    assert_eq!(
        t(Oracle, Sqlite, "SELECT 1 FROM dual WHERE EXISTS (SELECT 1 FROM t WHERE ROWNUM = 1)"),
        "SELECT 1 WHERE EXISTS (SELECT 1 FROM t LIMIT 1);"
    );
    // Oracle itself keeps it; ROWNUM = 2 never matches and is not a limit; an OR is not a conjunct
    assert_eq!(t(Oracle, Oracle, "SELECT a FROM t WHERE ROWNUM = 1"), "SELECT a FROM t WHERE ROWNUM = 1;");
    assert_eq!(t(Oracle, Sqlite, "SELECT a FROM t WHERE ROWNUM = 2"), "SELECT a FROM t WHERE ROWNUM = 2;");
    assert_eq!(
        t(Oracle, Sqlite, "SELECT a FROM t WHERE b = 1 OR ROWNUM = 1"),
        "SELECT a FROM t WHERE b = 1 OR ROWNUM = 1;"
    );
    // a query that has a limit already keeps its own
    assert_eq!(
        t(Oracle, Sqlite, "SELECT a FROM t WHERE ROWNUM <= 5 FETCH FIRST 2 ROWS ONLY"),
        "SELECT a FROM t WHERE ROWNUM <= 5 LIMIT 2;"
    );
}

#[test]
fn oracle_functions_become_sqlite_constructs() {
    assert_eq!(t(Oracle, Sqlite, "SELECT NVL(a, 0) FROM t"), "SELECT COALESCE(a, 0) FROM t;");
    assert_eq!(
        t(Oracle, Sqlite, "SELECT NVL2(a, 'y', 'n') FROM t"),
        "SELECT CASE WHEN a IS NOT NULL THEN 'y' ELSE 'n' END FROM t;"
    );
    assert_eq!(
        t(Oracle, Sqlite, "SELECT DECODE(a, 1, 'x', 2, 'z', 'o') FROM t"),
        "SELECT CASE a WHEN 1 THEN 'x' WHEN 2 THEN 'z' ELSE 'o' END FROM t;"
    );
    // DECODE matches NULL with NULL
    assert_eq!(
        t(Oracle, Sqlite, "SELECT DECODE(a, NULL, 'n', 'o') FROM t"),
        "SELECT CASE WHEN a IS NULL THEN 'n' ELSE 'o' END FROM t;"
    );
    assert_eq!(t(Oracle, Sqlite, "SELECT SYSDATE FROM dual"), "SELECT datetime('now', 'localtime');");
    assert_eq!(
        t(Oracle, Sqlite, "SELECT LISTAGG(n, ', ') WITHIN GROUP (ORDER BY n) FROM t"),
        "SELECT group_concat(n, ', ' ORDER BY n) FROM t;"
    );
    // Oracle's default separator is empty, SQLite's is a comma
    assert_eq!(
        t(Oracle, Sqlite, "SELECT LISTAGG(n) WITHIN GROUP (ORDER BY n) FROM t"),
        "SELECT group_concat(n, '' ORDER BY n) FROM t;"
    );
    assert_eq!(
        t(Postgres, Sqlite, "SELECT STRING_AGG(n, ',' ORDER BY n), NOW() FROM t"),
        "SELECT group_concat(n, ',' ORDER BY n), datetime('now') FROM t;"
    );
    // other targets are untouched
    assert_eq!(t(Oracle, Postgres, "SELECT NVL(a, 0) FROM t"), "SELECT NVL(a, 0) FROM t;");
}

#[test]
fn more_oracle_functions_become_sqlite_constructs() {
    let o = |sql: &str| t(Oracle, Sqlite, sql);
    assert_eq!(o("SELECT GREATEST(a, b, 3), LEAST(a, b) FROM t"), "SELECT max(a, b, 3), min(a, b) FROM t;");
    assert_eq!(o("SELECT CHR(65), ASCII(x) FROM t"), "SELECT char(65), unicode(x) FROM t;");
    assert_eq!(o("SELECT a FROM t WHERE LNNVL(a > 1)"), "SELECT a FROM t WHERE (a > 1) IS NOT TRUE;");
    assert_eq!(o("SELECT TO_NUMBER(x) FROM t"), "SELECT CAST(x AS NUMERIC) FROM t;");
    assert_eq!(o("SELECT LAST_DAY(d) FROM t"), "SELECT date(d, 'start of month', '+1 month', '-1 day') FROM t;");
    assert_eq!(
        o("SELECT LPAD(x, 5, 'ab') FROM t"),
        "SELECT substr(substr(replace(hex(zeroblob(5)), '00', 'ab'), 1, max(5 - length(x), 0)) || x, 1, 5) FROM t;"
    );
    assert_eq!(
        o("SELECT RPAD(x, 5) FROM t"),
        "SELECT substr(x || substr(replace(hex(zeroblob(5)), '00', ' '), 1, max(5 - length(x), 0)), 1, 5) FROM t;"
    );
    assert_eq!(
        o("SELECT INSTR(x, 'a', 2) FROM t"),
        "SELECT CASE WHEN instr(substr(x, 2), 'a') = 0 THEN 0 ELSE instr(substr(x, 2), 'a') + 2 - 1 END FROM t;"
    );
    assert_eq!(
        o("SELECT TO_CHAR(d, 'YYYY-MM-DD HH24:MI:SS'), TO_CHAR(d, 'DDD \"of\" YYYY') FROM t"),
        "SELECT strftime('%Y-%m-%d %H:%M:%S', d), strftime('%j of %Y', d) FROM t;"
    );
    assert_eq!(
        o("SELECT TO_DATE(x, 'YYYY-MM-DD'), TO_DATE(x, 'YYYY-MM-DD HH24:MI:SS') FROM t"),
        "SELECT date(x), datetime(x) FROM t;"
    );
    assert_eq!(o("SELECT LOCALTIMESTAMP FROM dual"), "SELECT datetime('now', 'localtime');");
    // masks and forms without an exact equivalent are left alone
    assert_eq!(o("SELECT TO_CHAR(d, 'Month DD') FROM t"), "SELECT TO_CHAR(d, 'Month DD') FROM t;");
    assert_eq!(o("SELECT TO_DATE(x, 'DD.MM.YYYY') FROM t"), "SELECT TO_DATE(x, 'DD.MM.YYYY') FROM t;");
    assert_eq!(o("SELECT TO_CHAR(n, '999.99') FROM t"), "SELECT TO_CHAR(n, '999.99') FROM t;");
    assert_eq!(o("SELECT INSTR(x, 'a', -1) FROM t"), "SELECT INSTR(x, 'a', -1) FROM t;");
    assert_eq!(o("SELECT GREATEST(a) FROM t"), "SELECT GREATEST(a) FROM t;");
}
