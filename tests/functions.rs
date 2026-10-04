use sql_sage::dialect::DialectKind::*;
use sql_sage::functions::{contains, dialects_with};

#[test]
fn catalogs_are_populated_from_the_real_sources() {
    // sizes are a sanity check on the generated data, not exact counts
    let lines = |t: &str| t.lines().filter(|l| !l.starts_with('#')).count();
    assert!(lines(include_str!("../data/functions/postgres.txt")) > 2000, "postgres catalog looks truncated");
    assert!(lines(include_str!("../data/functions/oracle.txt")) > 300, "oracle catalog looks truncated");
    assert!(lines(include_str!("../data/functions/sqlite.txt")) > 100, "sqlite catalog looks truncated");
    for (kind, names) in [
        (Postgres, ["now", "string_agg", "date_trunc", "coalesce", "count", "row_number", "generate_series", "to_char"]),
        (Oracle, ["nvl", "decode", "listagg", "sysdate", "add_months", "regexp_like", "to_char", "row_number"]),
        (Sqlite, ["ifnull", "group_concat", "strftime", "json_extract", "coalesce", "count", "row_number", "random"]),
    ] {
        for n in names {
            assert!(contains(kind, n), "{kind:?} should have {n}");
        }
    }
}

#[test]
fn engine_specific_names_are_not_cross_listed() {
    assert!(!contains(Postgres, "nvl"));
    assert!(!contains(Sqlite, "nvl"));
    assert!(!contains(Oracle, "now"));
    assert!(!contains(Sqlite, "now"));
    assert!(!contains(Oracle, "string_agg"));
    assert!(!contains(Sqlite, "initcap"));
    assert!(!contains(Oracle, "similarity")); // pg_trgm is an extension, not core
    assert_eq!(dialects_with("initcap"), vec!["oracle", "postgres"]);
    assert_eq!(dialects_with("nvl"), vec!["oracle"]);
    assert!(dialects_with("my_udf").is_empty());
}

#[test]
fn data_files_are_clean() {
    for (name, text) in [
        ("postgres", include_str!("../data/functions/postgres.txt")),
        ("sqlite", include_str!("../data/functions/sqlite.txt")),
        ("oracle", include_str!("../data/functions/oracle.txt")),
    ] {
        for l in text.lines().filter(|l| !l.starts_with('#')) {
            assert_eq!(l, l.trim(), "{name}: {l:?}");
            assert_eq!(l, l.to_lowercase(), "{name}: {l:?} must be lower-case");
            assert!(!l.is_empty() && !l.contains(' '), "{name}: {l:?}");
        }
    }
}
