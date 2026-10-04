use sql_sage::check_compatibility;
use sql_sage::compat::{Report, Severity};
use sql_sage::dialect::DialectKind::{self, *};

fn report(from: DialectKind, to: DialectKind, sql: &str) -> Report {
    check_compatibility(from, to, sql).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn messages(r: &Report, sev: Severity) -> Vec<String> {
    r.findings.iter().filter(|f| f.severity == sev).map(|f| f.message.clone()).collect()
}

fn has(r: &Report, sev: Severity, needle: &str) -> bool {
    messages(r, sev).iter().any(|m| m.contains(needle))
}

#[test]
fn same_dialect_is_clean() {
    for k in [Postgres, Sqlite, Oracle] {
        let r = report(k, k, "SELECT a, b FROM t WHERE c = 1 ORDER BY a");
        assert!(r.is_compatible());
        assert!(r.findings.is_empty(), "{k:?}: {:?}", r.findings);
    }
}

#[test]
fn oracle_to_postgres() {
    let r = report(
        Oracle,
        Postgres,
        "SELECT NVL(a, 0), SYSDATE, DECODE(b, 1, 'x', 'y'), s || 't', a / 2, ROWNUM, :id \
         FROM t WHERE c <> '' MINUS SELECT 1, 2, 3, 4, 5, 6, 7 FROM dual",
    );
    assert!(!r.is_compatible());
    assert!(has(&r, Severity::Incompatible, "function NVL does not exist in postgres: use COALESCE"));
    assert!(has(&r, Severity::Incompatible, "SYSDATE"));
    assert!(has(&r, Severity::Incompatible, "DECODE"));
    assert!(has(&r, Severity::Incompatible, "ROWNUM"));
    assert!(has(&r, Severity::Incompatible, "bind parameter ':id'"));
    assert!(has(&r, Severity::Warning, "|| treats NULL as an empty string in oracle"));
    assert!(has(&r, Severity::Warning, "'' is NULL in oracle"));
    assert!(has(&r, Severity::Warning, "integer / integer truncates in postgres"));
    assert!(has(&r, Severity::Rewritten, "set difference spelled EXCEPT"));
    assert!(has(&r, Severity::Rewritten, "FROM dual removed"));
}

#[test]
fn postgres_to_oracle() {
    let r = report(
        Postgres,
        Oracle,
        "SELECT now(), a ILIKE 'x', data->>'k', true, a % 2, $1 FROM t LIMIT 5; \
         INSERT INTO t VALUES (1) RETURNING id; DROP TABLE IF EXISTS t; \
         CREATE TABLE t (id bigint, tags text[], b boolean)",
    );
    assert!(has(&r, Severity::Incompatible, "function NOW does not exist in oracle"));
    assert!(has(&r, Severity::Incompatible, "JSON operators"));
    assert!(has(&r, Severity::Incompatible, "RETURNING"));
    assert!(has(&r, Severity::Incompatible, "IF EXISTS"));
    assert!(has(&r, Severity::Incompatible, "array"));
    assert!(has(&r, Severity::Rewritten, "ILIKE -> LOWER"));
    assert!(has(&r, Severity::Rewritten, "boolean literal -> 1"));
    assert!(has(&r, Severity::Rewritten, "a % b -> MOD"));
    assert!(has(&r, Severity::Rewritten, "bind parameter $1 -> :1"));
    assert!(has(&r, Severity::Rewritten, "row limiting rewritten"));
    assert!(has(&r, Severity::Rewritten, "type BIGINT -> NUMBER(19)"));
}

#[test]
fn quoted_identifier_case_warning() {
    let r = report(Postgres, Oracle, r#"SELECT "Name" FROM t"#);
    assert!(has(&r, Severity::Warning, "quoted identifier is case-sensitive"));
    // already in the target's folded form: nothing to warn about
    let r = report(Postgres, Oracle, r#"SELECT "NAME" FROM t"#);
    assert!(messages(&r, Severity::Warning).is_empty());
    // SQLite is case-insensitive
    let r = report(Postgres, Sqlite, r#"SELECT "Name" FROM t"#);
    assert!(messages(&r, Severity::Warning).is_empty());
}

#[test]
fn findings_carry_statement_and_line() {
    let r = report(Oracle, Postgres, "SELECT 1 FROM dual;\n\nSELECT NVL(a, 0) FROM t");
    let f = r.findings.iter().find(|f| f.message.contains("NVL")).unwrap();
    assert_eq!(f.statement, 2);
    assert_eq!(f.location.as_ref().unwrap().line, 3);
    assert_eq!(f.at.as_deref(), Some("NVL(a, 0)"));
    let text = r.to_string();
    assert!(text.contains("statement 2 (line 3)"), "{text}");
    assert!(text.contains("[incompatible]"), "{text}");
}

#[test]
fn analysis_agrees_with_transpile() {
    // everything the analysis calls incompatible must fail in transpile, and vice versa
    let corpus = [
        (Postgres, Oracle, "SELECT a FROM t RETURNING x"),
        (Postgres, Oracle, "INSERT INTO t VALUES (1) RETURNING id"),
        (Postgres, Sqlite, "CREATE TABLE t (a int GENERATED ALWAYS AS IDENTITY)"),
        (Sqlite, Postgres, "CREATE TABLE t (a, b)"),
        (Postgres, Oracle, "SELECT a FROM t LIMIT 5"),
        (Oracle, Sqlite, "SELECT a FROM t FETCH FIRST 5 ROWS WITH TIES"),
        (Sqlite, Postgres, "SELECT a GLOB 'x*' FROM t"),
        (Sqlite, Postgres, "INSERT OR REPLACE INTO t VALUES (1)"),
    ];
    for (from, to, sql) in corpus {
        let Ok(r) = check_compatibility(from, to, sql) else { continue };
        let transpiled = sql_sage::transpile(from, to, sql);
        assert_eq!(r.is_compatible(), transpiled.is_ok(), "{from:?}->{to:?} {sql}\n{r}\n{transpiled:?}");
    }
}

#[test]
fn function_catalogs_decide_availability() {
    // exists in another dialect but not on the target
    let r = report(Postgres, Sqlite, "SELECT initcap(a), lpad(a, 5) FROM t");
    assert!(has(&r, Severity::Incompatible, "INITCAP does not exist in sqlite (available in oracle/postgres)"));
    assert!(has(&r, Severity::Incompatible, "LPAD does not exist in sqlite: no built-in"));
    // present on the target: nothing to report, even where an old hint existed
    let r = report(Postgres, Sqlite, "SELECT string_agg(a, ','), mod(a, 2), concat_ws('-', a, b) FROM t");
    assert!(r.findings.iter().all(|f| f.severity != Severity::Incompatible), "{r}");
    // Oracle-only analytic function used in a Postgres query
    let r = report(Postgres, Postgres, "SELECT ratio_to_report(a) OVER () FROM t");
    assert!(has(&r, Severity::Incompatible, "RATIO_TO_REPORT does not exist in postgres (available in oracle)"));
}

#[test]
fn unknown_functions_are_unverified_not_incompatible() {
    let r = report(Postgres, Oracle, "SELECT my_udf(a), pg_catalog.lower(a), app.helper(a) FROM t");
    assert!(r.is_compatible());
    assert_eq!(r.count(Severity::Unverified), 1, "{r}");
    assert!(has(&r, Severity::Unverified, "MY_UDF is not a built-in of any supported dialect"));
    // trusted in the dialect it was written for
    let r = report(Postgres, Postgres, "SELECT my_udf(a) FROM t");
    assert!(r.findings.is_empty());
}

#[test]
fn same_name_different_meaning() {
    let r = report(Oracle, Postgres, "SELECT DECODE(a, 1, 'x', 'y') FROM t");
    assert!(has(&r, Severity::Incompatible, "DECODE exists in postgres but means something else"));
    let r = report(Oracle, Postgres, "SELECT NVL(a, 0) FROM t");
    assert!(has(&r, Severity::Incompatible, "function NVL does not exist in postgres: use COALESCE"));
    let r = report(Sqlite, Oracle, "SELECT max(a, b) FROM t");
    assert!(has(&r, Severity::Incompatible, "MAX exists in oracle but means something else"));
    let r = report(Oracle, Oracle, "SELECT concat(a, b, c) FROM t");
    assert!(has(&r, Severity::Incompatible, "CONCAT exists in oracle but means something else"));
}
