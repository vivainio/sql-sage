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
fn to_date_without_a_mask_and_round_trips_through_to_char() {
    assert_eq!(lite("SELECT TO_DATE('01-JAN-1900') FROM dual"), "SELECT '1900-01-01';");
    assert_eq!(lite("SELECT TO_DATE('15-MAR-21') FROM dual"), "SELECT '2021-03-15';");
    assert_eq!(
        lite("CREATE INDEX i ON t (TO_DATE(TO_CHAR(valid_from, 'MM/DD/YYYY'), 'MM/DD/YYYY'))"),
        "CREATE INDEX i ON t (date(valid_from));"
    );
    // with a time of day in the mask the round trip keeps it, so it is not a plain date()
    assert!(transpile(Oracle, Sqlite, "SELECT TO_DATE(TO_CHAR(d, 'MM/DD/YYYY HH24:MI'), 'MM/DD/YYYY HH24:MI') FROM t")
        .unwrap()
        .contains("TO_DATE("));
}

#[test]
fn storage_clauses_are_dropped_and_index_rebuilds_become_reindex() {
    assert_eq!(
        lite("ALTER TABLE t MOVE ONLINE ROW STORE COMPRESS ADVANCED"),
        "-- ALTER TABLE t (storage options only: nothing to do in SQLite);"
    );
    assert_eq!(
        lite("ALTER TABLE t ENABLE ROW MOVEMENT"),
        "-- ALTER TABLE t (storage options only: nothing to do in SQLite);"
    );
    assert_eq!(lite("ALTER TABLE t ADD c NUMBER, LOGGING, PARALLEL 4"), "ALTER TABLE t ADD COLUMN c NUMBER;");
    assert_eq!(lite("ALTER INDEX ix REBUILD ONLINE"), "REINDEX ix;");
    assert!(lite("ALTER INDEX ix NOPARALLEL").starts_with("-- ALTER INDEX ix NOPARALLEL"));
    // anything with a meaning of its own is still refused
    assert!(transpile(Oracle, Sqlite, "ALTER TABLE t DISABLE CONSTRAINT c").is_err());
    assert!(transpile(Oracle, Sqlite, "ALTER TABLE t TRUNCATE PARTITION p").is_err());
    assert!(transpile(Oracle, Sqlite, "ALTER INDEX ix UNUSABLE").is_err());
    assert!(transpile(Oracle, Postgres, "ALTER TABLE t MOVE ONLINE").is_err());
    // between the same dialect nothing changes
    assert_eq!(transpile(Oracle, Oracle, "ALTER TABLE t MOVE ONLINE").unwrap(), "ALTER TABLE t MOVE ONLINE;");
}

#[test]
fn multi_table_insert_from_dual_becomes_plain_inserts() {
    assert_eq!(
        lite("INSERT ALL INTO a (x, y) VALUES (1, 'p') INTO b VALUES (2) SELECT * FROM dual"),
        "INSERT INTO a (x, y) VALUES (1, 'p'); INSERT INTO b VALUES (2);"
    );
    // conditional or row-dependent inserts have no such equivalent
    assert!(transpile(Oracle, Sqlite, "INSERT ALL INTO a VALUES (1) SELECT * FROM src").is_err());
    assert!(transpile(Oracle, Sqlite, "INSERT FIRST WHEN 1 = 1 THEN INTO a VALUES (1) SELECT * FROM dual").is_err());
    assert!(transpile(Oracle, Sqlite, "INSERT ALL INTO a SELECT * FROM dual").is_err());
}

#[test]
fn rr_accepts_four_digit_years() {
    assert_eq!(lite("SELECT TO_DATE('31-12-9999', 'DD-MM-RR') FROM dual"), "SELECT '9999-12-31';");
    assert_eq!(lite("SELECT TO_DATE('01-04-16', 'DD-MM-RR') FROM dual"), "SELECT '2016-04-01';");
    assert_eq!(lite("SELECT TO_DATE('010420', 'DDMMRR') FROM dual"), "SELECT '2020-04-01';");
}

#[test]
fn hextoraw_and_case_insensitive_sort_keys() {
    assert_eq!(
        lite("CREATE TABLE t (id RAW(16) DEFAULT HEXTORAW('00000000000000000000000000000000') NOT NULL)"),
        "CREATE TABLE t (id RAW(16) DEFAULT X'00000000000000000000000000000000' NOT NULL);"
    );
    assert_eq!(lite("CREATE INDEX i ON t (NLSSORT(name, 'NLS_SORT=BINARY_CI'))"), "CREATE INDEX i ON t (lower(name));");
    // not a hex literal, or another collation: left as written
    assert!(transpile(Oracle, Sqlite, "SELECT HEXTORAW(x) FROM t").unwrap().contains("HEXTORAW("));
    assert!(transpile(Oracle, Sqlite, "SELECT NLSSORT(a, 'NLS_SORT=GERMAN') FROM t").unwrap().contains("NLSSORT("));
}

#[test]
fn a_derived_schema_can_be_written_for_sqlite() {
    use sql_sage::schema::Schema;
    let mut s = Schema::new(Oracle);
    let applied = s
        .apply_sql(
            "CREATE SEQUENCE seq1;
             CREATE TABLE t (id VARCHAR2(32 CHAR) DEFAULT SYS_GUID() NOT NULL, n NUMBER, d DATE DEFAULT SYSDATE);
             ALTER TABLE t MODIFY (n NUMBER(10, 2));
             ALTER TABLE t ADD CONSTRAINT pk_t PRIMARY KEY (id);
             CREATE INDEX i ON t (n);
             CREATE OR REPLACE VIEW v AS SELECT id FROM t;",
        )
        .unwrap();
    assert_eq!(applied.errors, vec![]);
    // The strict form refuses because SQLite has no sequences ...
    assert!(s.to_sql(Sqlite).is_err());
    // ... the lossy one leaves the sequence out and says so. The ALTERs are already folded in.
    let (sql, skipped) = s.to_sql_lossy(Sqlite);
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].message.starts_with("sequence seq1"), "{}", skipped[0].message);
    assert_eq!(
        sql,
        "CREATE TABLE t (id VARCHAR2(32) DEFAULT hex(randomblob(16)) NOT NULL, n NUMBER(10, 2), d DATE DEFAULT datetime('now', 'localtime'), CONSTRAINT pk_t PRIMARY KEY (id));\n\
         CREATE INDEX i ON t (n);\n\
         DROP VIEW IF EXISTS v; CREATE VIEW v AS SELECT id FROM t;"
    );
    assert!(check_syntax(Sqlite, &sql).is_ok());
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
