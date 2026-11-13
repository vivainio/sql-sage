//! Oracle scripts translated to SQLite: literal date conversions, SYS_GUID, views, transaction control.

use sql_sage::check_syntax;
use sql_sage::dialect::DialectKind::*;
use sql_sage::transpile;

fn lite(sql: &str) -> String {
    let out = transpile(Oracle, Sqlite, sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    assert!(check_syntax(Sqlite, &out).is_ok(), "not SQLite: {out}");
    out
}

#[test]
fn to_date_and_to_timestamp_with_literals_are_evaluated() {
    let cases = [
        ("TO_DATE('01/02/2020 13:05:09', 'MM/DD/YYYY HH24:MI:SS')", "'2020-01-02 13:05:09'"),
        ("TO_DATE('2020-01-02', 'YYYY-MM-DD')", "'2020-01-02'"),
        ("TO_DATE('2020/1/2 03:04:05', 'yyyy/mm/dd hh24:mi:ss')", "'2020-01-02 03:04:05'"),
        ("TO_DATE('02-01-20', 'DD-MM-RR')", "'2020-01-02'"),
        ("TO_DATE('02-01-75', 'DD-MM-RR')", "'1975-01-02'"),
        ("TO_DATE('15-JAN-21', 'dd-mon-rr')", "'2021-01-15'"),
        ("TO_DATE('02 01 2020', 'MM DD YYYY')", "'2020-02-01'"),
        ("TO_DATE('31122020', 'ddmmyyyy')", "'2020-12-31'"),
        ("TO_TIMESTAMP('02-01-20 01.02.03.500000 PM', 'dd-mm-rr hh.mi.ss.ff am')", "'2020-01-02 13:02:03.500000'"),
        ("TO_TIMESTAMP('02-01-20 12.00.00.000000 AM', 'dd-mm-rr hh.mi.ss.ff am')", "'2020-01-02 00:00:00'"),
        ("TO_TIMESTAMP('15-JAN-21 10.20.30.123456 AM', 'dd-mon-rr hh.mi.ssxff am')", "'2021-01-15 10:20:30.123456'"),
        ("TO_TIMESTAMP('2020-01-02 03:04:05', 'YYYY-MM-DD HH24:MI:SS')", "'2020-01-02 03:04:05'"),
    ];
    for (oracle, expected) in cases {
        assert_eq!(lite(&format!("SELECT {oracle} FROM dual")), format!("SELECT {expected};"), "{oracle}");
    }
    // Leap days are checked. A value that does not fit is not folded: with one of the two simple
    // masks it becomes date()/datetime() (NULL at run time, where Oracle raises an error);
    // with any other mask there is no translation.
    assert_eq!(lite("SELECT TO_DATE('2020-02-29', 'YYYY-MM-DD') FROM dual"), "SELECT '2020-02-29';");
    assert_eq!(lite("SELECT TO_DATE('2021-02-29', 'YYYY-MM-DD') FROM dual"), "SELECT date('2021-02-29');");
    assert_eq!(
        lite("SELECT TO_DATE('2020-01-01 25:00:00', 'YYYY-MM-DD HH24:MI:SS') FROM dual"),
        "SELECT datetime('2020-01-01 25:00:00');"
    );
    // otherwise the call is left as written, which SQLite does not have
    for bad in ["TO_DATE('31/02/2020', 'DD/MM/YYYY')", "TO_DATE('2020', 'YYYY')", "TO_DATE('x', 'MM/DD/YYYY')"] {
        let out = transpile(Oracle, Sqlite, &format!("SELECT {bad} FROM dual")).unwrap();
        assert!(out.contains("TO_DATE("), "{out}");
    }
    // Not literals: the two simple masks still map to date() / datetime().
    assert_eq!(lite("SELECT TO_DATE(a, 'YYYY-MM-DD') FROM t"), "SELECT date(a) FROM t;");
    assert_eq!(lite("SELECT TO_TIMESTAMP(a, 'YYYY-MM-DD HH24:MI:SS') FROM t"), "SELECT datetime(a) FROM t;");
}

#[test]
fn inserts_with_date_literals_translate() {
    assert_eq!(
        lite("INSERT INTO t (id, d) VALUES (1, TO_DATE('06/30/2020 00:00:00', 'MM/DD/YYYY HH24:MI:SS'))"),
        "INSERT INTO t (id, d) VALUES (1, '2020-06-30 00:00:00');"
    );
}

#[test]
fn sys_guid_and_sys_extract_utc() {
    assert_eq!(lite("SELECT SYS_GUID() FROM dual"), "SELECT hex(randomblob(16));");
    assert_eq!(lite("SELECT SYS_EXTRACT_UTC(SYSTIMESTAMP) FROM dual"), "SELECT datetime('now');");
    assert_eq!(lite("SELECT SYS_EXTRACT_UTC(CURRENT_TIMESTAMP) FROM dual"), "SELECT datetime('now');");
    assert_eq!(lite("SELECT SYS_EXTRACT_UTC(created) FROM t"), "SELECT datetime(created) FROM t;");
    assert_eq!(
        lite("CREATE TABLE t (id VARCHAR2(32) DEFAULT SYS_GUID() NOT NULL)"),
        "CREATE TABLE t (id VARCHAR2(32) DEFAULT hex(randomblob(16)) NOT NULL);"
    );
}

#[test]
fn create_or_replace_view_becomes_drop_and_create() {
    assert_eq!(
        lite("CREATE OR REPLACE VIEW v AS SELECT a FROM t"),
        "DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT a FROM t;"
    );
    assert_eq!(
        lite("CREATE OR REPLACE FORCE VIEW v (x) AS SELECT NVL(a, 0) FROM t"),
        "DROP VIEW IF EXISTS v; CREATE VIEW v (x) AS SELECT COALESCE(a, 0) FROM t;"
    );
    assert_eq!(lite("CREATE VIEW v AS SELECT 1 FROM dual"), "CREATE VIEW v AS SELECT 1;");
    // PostgreSQL has OR REPLACE but no FORCE.
    assert!(transpile(Oracle, Postgres, "CREATE OR REPLACE FORCE VIEW v AS SELECT 1 FROM dual").is_err());
}

#[test]
fn transaction_control_translates_everywhere() {
    for target in [Sqlite, Postgres] {
        assert_eq!(transpile(Oracle, target, "COMMIT").unwrap(), "COMMIT;");
        assert_eq!(transpile(Oracle, target, "ROLLBACK").unwrap(), "ROLLBACK;");
        assert_eq!(transpile(Oracle, target, "SAVEPOINT s1").unwrap(), "SAVEPOINT s1;");
    }
    // Other unmodeled statements still do not.
    assert!(transpile(Oracle, Sqlite, "GRANT SELECT ON t TO r").is_err());
}
