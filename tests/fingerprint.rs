use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::fingerprint::{Access, Operation};
use sql_sage::fingerprint_sql;

fn fp(kind: DialectKind, sql: &str) -> String {
    let v = fingerprint_sql(kind, sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    v.iter().map(|f| f.canonical()).collect::<Vec<_>>().join("; ")
}

#[test]
fn select_with_everything() {
    assert_eq!(
        fp(
            Postgres,
            "WITH recent AS (SELECT * FROM orders WHERE placed > $1) \
             SELECT c.name, count(*), lower(c.region) FROM recent r LEFT JOIN customers c ON c.id = r.cid \
             WHERE EXISTS (SELECT 1 FROM blocked b WHERE b.cid = c.id) GROUP BY c.name ORDER BY 2 LIMIT 5"
        ),
        "select | blocked:read customers:read orders:read | filter join:left group sort limit aggregate subquery cte \
         | count() lower()"
    );
}

#[test]
fn ignores_literals_columns_aliases_and_dialect() {
    let a = fp(Postgres, "SELECT a, b FROM t x WHERE a = 1 ORDER BY b LIMIT 10");
    let b = fp(Sqlite, "SELECT z FROM t WHERE q = 'other' AND 1 = 1 ORDER BY q LIMIT ?1 OFFSET 7");
    assert_eq!(a, b);
    assert_eq!(
        fp(Oracle, "SELECT a FROM t MINUS SELECT a FROM u FETCH FIRST 3 ROWS ONLY"),
        fp(Postgres, "SELECT a FROM t EXCEPT SELECT a FROM u LIMIT 3")
    );
    assert_eq!(fp(Oracle, "SELECT SYSDATE FROM dual"), "select | - | - | -");
    // different shape => different fingerprint
    assert_ne!(a, fp(Postgres, "SELECT a FROM t"));
}

#[test]
fn functions_are_listed() {
    assert_eq!(
        fp(Oracle, "SELECT NVL(a, 0), UPPER(b), row_number() OVER (ORDER BY c), sum(d) FROM t"),
        "select | t:read | aggregate window | nvl() row_number() sum() upper()"
    );
    assert_eq!(fp(Postgres, "SELECT pg_catalog.now(), Now()"), "select | - | - | now() pg_catalog.now()");
    // functions inside subqueries, CASE and DML count too
    assert_eq!(
        fp(Postgres, "UPDATE t SET a = CASE WHEN x > 1 THEN lower(y) ELSE (SELECT max(z) FROM u) END"),
        "update | t:update u:read | aggregate subquery | lower() max()"
    );
}

#[test]
fn dml_ddl() {
    assert_eq!(
        fp(Postgres, "INSERT INTO a SELECT * FROM b ON CONFLICT (id) DO NOTHING RETURNING id"),
        "insert | a:insert b:read | upsert returning | -"
    );
    assert_eq!(fp(Sqlite, "DELETE FROM t WHERE id IN (SELECT id FROM u)"), "delete | t:delete u:read | filter subquery | -");
    assert_eq!(
        fp(Postgres, "CREATE TABLE t (id int, c int REFERENCES customers(id), FOREIGN KEY (id) REFERENCES t(id))"),
        "create_table | customers:read t:create | foreign_key | -"
    );
    assert_eq!(fp(Postgres, "CREATE UNIQUE INDEX i ON t (a) WHERE a > 0"), "create_index | t:index | filter | -");
    assert_eq!(fp(Postgres, "DROP TABLE IF EXISTS a, b"), "drop_table | a:drop b:drop | - | -");
}

#[test]
fn ctes_are_not_tables_and_names_are_normalized() {
    assert_eq!(
        fp(Postgres, "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c) SELECT n FROM c"),
        "select | - | union_all cte recursive | -"
    );
    assert_eq!(
        fp(Postgres, r#"SELECT 1 FROM Public.Orders, "MixedCase", "plain""#),
        r#"select | "MixedCase":read plain:read public.orders:read | - | -"#
    );
}

#[test]
fn read_write_helpers_and_stable_id() {
    let f = &fingerprint_sql(Postgres, "UPDATE a SET x = 1 FROM b WHERE a.id = b.id").unwrap()[0];
    assert_eq!(f.writes(), vec!["a"]);
    assert_eq!(f.reads(), vec!["b"]);
    assert!(!f.is_read_only());
    assert!(f.tables["a"].contains(&Access::Update));
    assert!(f.operations.contains(&Operation::Filter));
    let g = &fingerprint_sql(Sqlite, "update a set y = 2 from b where a.k = b.k").unwrap()[0];
    assert_eq!(f.id(), g.id());
    assert_eq!(f.id().len(), 16);
}

fn code(kind: DialectKind, sql: &str) -> String {
    fingerprint_sql(kind, sql).unwrap()[0].compat_code()
}

#[test]
fn compat_code() {
    // portable SQL runs everywhere, whichever dialect it was written in
    assert_eq!(code(Postgres, "SELECT a FROM t WHERE b = 1 ORDER BY a LIMIT 5"), "ora,pg,lite");
    assert_eq!(code(Oracle, "SELECT a FROM t ORDER BY a FETCH FIRST 5 ROWS ONLY"), "ora,pg,lite");
    assert_eq!(code(Oracle, "SELECT 1 FROM dual"), "ora,pg,lite");
    // rewrites and warnings do not reduce compatibility
    assert_eq!(code(Postgres, "SELECT a::int, a ILIKE 'x' FROM t"), "ora,pg,lite");
    // engine-specific functions / syntax narrow it
    assert_eq!(code(Oracle, "SELECT NVL(a, 0) FROM t"), "ora");
    assert_eq!(code(Postgres, "SELECT now()"), "pg");
    assert_eq!(code(Postgres, "INSERT INTO t VALUES (1) RETURNING id"), "pg,lite");
    assert_eq!(code(Postgres, "SELECT data->>'k' FROM t"), "pg,lite");
    assert_eq!(code(Sqlite, "SELECT a GLOB 'x*' FROM t"), "lite");
    // a function the *source* dialect lacks is not claimed as compatible with it
    assert_eq!(code(Postgres, "SELECT NVL(a, 0) FROM t"), "ora");
    // nothing fits
    assert_eq!(code(Postgres, "SELECT a FROM t GROUP BY a HAVING sysdate > 1 AND now() > 1"), "-");
}

#[test]
fn compat_code_does_not_change_the_id() {
    let with = &fingerprint_sql(Postgres, "SELECT NVL(a, 0) FROM t").unwrap()[0];
    let bare = sql_sage::parse_sql(Postgres.dialect().as_ref(), "SELECT NVL(a, 0) FROM t").unwrap()[0].fingerprint();
    assert_eq!(with.id(), bare.id());
    assert_eq!(with.canonical(), bare.canonical());
    assert_eq!(bare.compat_code(), "-");
}
