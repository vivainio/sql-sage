//! Catalogs of built-in function names for the latest release of each engine.
//!
//! The data lives in `data/functions/*.txt` and is regenerated from the engines'
//! own sources by `tools/gen_function_catalogs.py` (Postgres `pg_proc`, SQLite
//! `PRAGMA function_list`, the Oracle SQL Language Reference). Only core
//! built-ins are listed: extension and user-defined functions are *unknown*,
//! not missing.

use crate::dialect::DialectKind;
use std::collections::HashSet;
use std::sync::OnceLock;

static POSTGRES: &str = include_str!("../data/functions/postgres.txt");
static SQLITE: &str = include_str!("../data/functions/sqlite.txt");
static ORACLE: &str = include_str!("../data/functions/oracle.txt");

fn load(text: &'static str) -> HashSet<&'static str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

fn catalog(kind: DialectKind) -> &'static HashSet<&'static str> {
    static PG: OnceLock<HashSet<&'static str>> = OnceLock::new();
    static LITE: OnceLock<HashSet<&'static str>> = OnceLock::new();
    static ORA: OnceLock<HashSet<&'static str>> = OnceLock::new();
    match kind {
        DialectKind::Postgres => PG.get_or_init(|| load(POSTGRES)),
        DialectKind::Sqlite => LITE.get_or_init(|| load(SQLITE)),
        DialectKind::Oracle => ORA.get_or_init(|| load(ORACLE)),
    }
}

/// Is `lower_name` a built-in function of `kind`?
pub fn contains(kind: DialectKind, lower_name: &str) -> bool {
    catalog(kind).contains(lower_name)
}

/// Names of the dialects (`oracle`, `postgres`, `sqlite`) that have `lower_name` built in.
pub fn dialects_with(lower_name: &str) -> Vec<&'static str> {
    DialectKind::ALL.into_iter().filter(|k| contains(*k, lower_name)).map(|k| k.name()).collect()
}
