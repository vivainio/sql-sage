//! Dialects describe *only* what differs between SQL engines. Everything they
//! accept is parsed into the same AST in [`crate::ast`].

use crate::ast::DataType;
use std::fmt::Debug;
use std::str::FromStr;

/// How a dialect resolves *unquoted* identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierFold {
    Upper,
    Lower,
    /// Case-insensitive; the original spelling is kept.
    Insensitive,
}

impl IdentifierFold {
    pub fn fold(self, s: &str) -> String {
        match self {
            IdentifierFold::Upper => s.to_uppercase(),
            IdentifierFold::Lower => s.to_lowercase(),
            IdentifierFold::Insensitive => s.to_string(),
        }
    }
}

/// A bind parameter, independent of how a dialect spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceholderKind {
    Numbered(usize),
    Named(String),
}

pub trait Dialect: Debug {
    fn name(&self) -> &'static str;

    // ---- lexical ----
    fn is_identifier_start(&self, ch: char) -> bool {
        ch.is_alphabetic() || ch == '_'
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_'
    }
    /// Characters that open a quoted identifier (`"`, `` ` ``, `[`).
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        ch == '"'
    }
    fn supports_nested_block_comments(&self) -> bool {
        false
    }
    /// `E'...\n'` strings.
    fn supports_escape_strings(&self) -> bool {
        false
    }
    /// `q'[...]'` strings.
    fn supports_q_quote_strings(&self) -> bool {
        false
    }
    /// `$$...$$` and `$tag$...$tag$` strings.
    fn supports_dollar_quoted_strings(&self) -> bool {
        false
    }
    /// `$1` / `$name`
    fn supports_dollar_placeholders(&self) -> bool {
        false
    }
    /// `?` / `?1`
    fn supports_question_placeholders(&self) -> bool {
        false
    }
    /// `:name` / `:1`
    fn supports_colon_placeholders(&self) -> bool {
        false
    }
    /// `@name`
    fn supports_at_placeholders(&self) -> bool {
        false
    }

    // ---- expressions ----
    fn supports_double_colon_cast(&self) -> bool {
        false
    }
    fn supports_ilike(&self) -> bool {
        false
    }
    fn supports_glob(&self) -> bool {
        false
    }
    fn supports_json_arrows(&self) -> bool {
        false
    }

    // ---- queries ----
    fn supports_limit(&self) -> bool {
        false
    }
    /// `LIMIT offset, count`
    fn supports_limit_comma(&self) -> bool {
        false
    }
    /// `MINUS` as a synonym of `EXCEPT`
    fn supports_minus_operator(&self) -> bool {
        false
    }
    fn supports_distinct_on(&self) -> bool {
        false
    }

    // ---- DML / DDL ----
    fn supports_returning(&self) -> bool {
        false
    }
    fn supports_insert_or(&self) -> bool {
        false
    }
    fn supports_on_conflict(&self) -> bool {
        false
    }
    fn supports_update_from(&self) -> bool {
        false
    }
    /// `CREATE TABLE t (a, b)` — column types are optional.
    fn supports_untyped_columns(&self) -> bool {
        false
    }

    // ---- emission (see `crate::emit`) ----
    /// Character used to quote identifiers when emitting.
    fn identifier_quote(&self) -> char {
        '"'
    }
    /// `FETCH FIRST n ROWS ONLY`
    fn supports_fetch_first(&self) -> bool {
        false
    }
    /// `OFFSET` is only valid after `LIMIT` (SQLite).
    fn requires_limit_for_offset(&self) -> bool {
        false
    }
    /// `OFFSET n ROWS` (Oracle) instead of `OFFSET n`.
    fn offset_requires_rows(&self) -> bool {
        false
    }
    /// `FROM t AS x` (Oracle only accepts `FROM t x`).
    fn supports_table_alias_as(&self) -> bool {
        true
    }
    fn supports_boolean_literals(&self) -> bool {
        true
    }
    fn supports_autoincrement(&self) -> bool {
        false
    }
    fn supports_identity_columns(&self) -> bool {
        true
    }
    fn temporary_table_keyword(&self) -> &'static str {
        "TEMPORARY"
    }
    /// `IF [NOT] EXISTS` in DDL.
    fn supports_if_exists(&self) -> bool {
        true
    }
    fn supports_multirow_values(&self) -> bool {
        true
    }
    /// `SELECT` must have a `FROM` clause (Oracle: `FROM dual`).
    fn requires_from_clause(&self) -> bool {
        false
    }
    fn supports_modulo_operator(&self) -> bool {
        true
    }
    fn supports_recursive_keyword(&self) -> bool {
        true
    }
    /// `DATE '2020-01-01'`
    fn supports_typed_literals(&self) -> bool {
        true
    }
    fn supports_partial_index(&self) -> bool {
        true
    }
    /// Spell a bind parameter; `None` if the dialect cannot express it.
    fn format_placeholder(&self, _kind: &PlaceholderKind) -> Option<String> {
        None
    }

    // ---- compatibility analysis (see `crate::compat`) ----
    /// Unquoted identifier resolution.
    fn identifier_fold(&self) -> IdentifierFold {
        IdentifierFold::Insensitive
    }
    /// `''` is NULL (Oracle).
    fn empty_string_is_null(&self) -> bool {
        false
    }
    /// `a / b` truncates for integer operands.
    fn integer_division_truncates(&self) -> bool {
        true
    }
    /// `'a' || NULL` yields `'a'` rather than NULL (Oracle).
    fn concat_null_is_empty(&self) -> bool {
        false
    }
    /// Is `lower_name` a built-in function of this dialect (latest release)?
    /// See [`crate::functions`]. Custom dialects default to "yes".
    fn has_function(&self, _lower_name: &str) -> bool {
        true
    }
    /// A function that exists here under the same name but means something else
    /// when called with `nargs` arguments (e.g. Postgres `decode`).
    fn function_collision(&self, _upper_name: &str, _nargs: usize) -> Option<&'static str> {
        None
    }
    /// If `upper_name` (a function or pseudo-column, upper-cased) does **not**
    /// exist in this dialect, a short hint on what to use instead.
    fn function_hint(&self, _upper_name: &str) -> Option<&'static str> {
        None
    }

    /// Translate a data type into this dialect's spelling.
    fn map_data_type(&self, dt: &DataType) -> Result<DataType, String> {
        Ok(strip_length_units(dt))
    }
}

/// `VARCHAR2(100 BYTE)` -> `VARCHAR2(100)`
fn strip_length_units(dt: &DataType) -> DataType {
    let mut out = dt.clone();
    out.args = dt
        .args
        .iter()
        .map(|a| {
            let parts: Vec<&str> = a.split_whitespace().collect();
            match parts.as_slice() {
                [n, u] if u.eq_ignore_ascii_case("BYTE") || u.eq_ignore_ascii_case("CHAR") => n.to_string(),
                _ => a.clone(),
            }
        })
        .collect();
    out
}

fn retype(dt: &DataType, name: &str, args: Option<Vec<&str>>) -> DataType {
    let mut out = strip_length_units(dt);
    out.name = name.to_string();
    if let Some(a) = args {
        out.args = a.into_iter().map(String::from).collect();
    }
    out
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PostgreSqlDialect;

impl Dialect for PostgreSqlDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '$'
    }
    fn supports_nested_block_comments(&self) -> bool {
        true
    }
    fn supports_escape_strings(&self) -> bool {
        true
    }
    fn supports_dollar_quoted_strings(&self) -> bool {
        true
    }
    fn supports_dollar_placeholders(&self) -> bool {
        true
    }
    fn supports_double_colon_cast(&self) -> bool {
        true
    }
    fn supports_ilike(&self) -> bool {
        true
    }
    fn supports_json_arrows(&self) -> bool {
        true
    }
    fn supports_limit(&self) -> bool {
        true
    }
    fn supports_distinct_on(&self) -> bool {
        true
    }
    fn supports_returning(&self) -> bool {
        true
    }
    fn supports_on_conflict(&self) -> bool {
        true
    }
    fn supports_update_from(&self) -> bool {
        true
    }
    fn supports_fetch_first(&self) -> bool {
        true
    }
    fn identifier_fold(&self) -> IdentifierFold {
        IdentifierFold::Lower
    }
    fn has_function(&self, name: &str) -> bool {
        crate::functions::contains(DialectKind::Postgres, name)
    }
    fn function_collision(&self, name: &str, nargs: usize) -> Option<&'static str> {
        match name {
            "DECODE" if nargs >= 3 => Some(
                "postgres decode(text, format) decodes base64/hex; Oracle's DECODE(x, search, result, ...) needs CASE",
            ),
            "MIN" | "MAX" if nargs >= 2 => {
                Some("with several arguments this is SQLite's scalar form; use LEAST / GREATEST")
            }
            _ => None,
        }
    }
    fn function_hint(&self, name: &str) -> Option<&'static str> {
        Some(match name {
            "NVL" | "IFNULL" => "use COALESCE",
            "NVL2" => "use CASE WHEN x IS NOT NULL THEN a ELSE b END",
            "DECODE" => "use CASE",
            "SYSDATE" | "SYSTIMESTAMP" => "use CURRENT_TIMESTAMP (or now())",
            "ROWNUM" => "use LIMIT, or row_number() OVER ()",
            "ROWID" => "use a real key column (ctid is not stable)",
            "LISTAGG" | "GROUP_CONCAT" => "use string_agg",
            "STRFTIME" => "use to_char",
            "INSTR" => "use position() or strpos()",
            "ADD_MONTHS" => "use date + n * interval '1 month'",
            "MONTHS_BETWEEN" => "use age() or date arithmetic",
            "LAST_DAY" => "use date_trunc('month', d) + interval '1 month - 1 day'",
            "SYS_GUID" => "use gen_random_uuid()",
            _ => return None,
        })
    }
    fn format_placeholder(&self, kind: &PlaceholderKind) -> Option<String> {
        match kind {
            PlaceholderKind::Numbered(n) => Some(format!("${n}")),
            PlaceholderKind::Named(_) => None,
        }
    }
    fn map_data_type(&self, dt: &DataType) -> Result<DataType, String> {
        Ok(match dt.name.as_str() {
            "VARCHAR2" | "NVARCHAR2" | "NVARCHAR" => retype(dt, "VARCHAR", None),
            "NUMBER" => retype(dt, "NUMERIC", None),
            "CLOB" | "NCLOB" | "LONG" => retype(dt, "TEXT", Some(vec![])),
            "BLOB" | "RAW" | "LONG RAW" => retype(dt, "BYTEA", Some(vec![])),
            "BINARY_DOUBLE" => retype(dt, "DOUBLE PRECISION", Some(vec![])),
            "BINARY_FLOAT" => retype(dt, "REAL", Some(vec![])),
            "DATETIME" => retype(dt, "TIMESTAMP", None),
            "TINYINT" => retype(dt, "SMALLINT", Some(vec![])),
            _ => strip_length_units(dt),
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        matches!(ch, '"' | '`' | '[')
    }
    fn supports_dollar_placeholders(&self) -> bool {
        true
    }
    fn supports_question_placeholders(&self) -> bool {
        true
    }
    fn supports_colon_placeholders(&self) -> bool {
        true
    }
    fn supports_at_placeholders(&self) -> bool {
        true
    }
    fn supports_glob(&self) -> bool {
        true
    }
    fn supports_json_arrows(&self) -> bool {
        true
    }
    fn supports_limit(&self) -> bool {
        true
    }
    fn supports_limit_comma(&self) -> bool {
        true
    }
    fn supports_returning(&self) -> bool {
        true
    }
    fn supports_insert_or(&self) -> bool {
        true
    }
    fn supports_on_conflict(&self) -> bool {
        true
    }
    fn supports_update_from(&self) -> bool {
        true
    }
    fn supports_untyped_columns(&self) -> bool {
        true
    }
    fn requires_limit_for_offset(&self) -> bool {
        true
    }
    fn supports_autoincrement(&self) -> bool {
        true
    }
    fn supports_identity_columns(&self) -> bool {
        false
    }
    fn supports_typed_literals(&self) -> bool {
        false
    }
    fn has_function(&self, name: &str) -> bool {
        crate::functions::contains(DialectKind::Sqlite, name)
    }
    fn function_hint(&self, name: &str) -> Option<&'static str> {
        Some(match name {
            "NVL" => "use IFNULL or COALESCE",
            "NVL2" | "DECODE" => "use CASE",
            "SYSDATE" | "SYSTIMESTAMP" | "NOW" => "use datetime('now')",
            "STRING_AGG" | "LISTAGG" => "use group_concat",
            "TO_CHAR" | "EXTRACT" => "use strftime",
            "TO_DATE" => "use date() / datetime()",
            "DATE_TRUNC" => "use strftime or date modifiers",
            "ARRAY_AGG" => "no equivalent (aggregate to JSON with json_group_array)",
            "LPAD" | "RPAD" => "no built-in; use substr/printf",
            "REGEXP_REPLACE" => "no built-in",
            "GEN_RANDOM_UUID" => "use lower(hex(randomblob(16)))",
            "ROWNUM" => "use LIMIT, or row_number() OVER ()",
            "MOD" => "use the % operator",
            "ADD_MONTHS" | "MONTHS_BETWEEN" => "use date modifiers",
            "LEFT" | "RIGHT" => "use substr",
            _ => return None,
        })
    }
    fn format_placeholder(&self, kind: &PlaceholderKind) -> Option<String> {
        Some(match kind {
            PlaceholderKind::Numbered(n) => format!("?{n}"),
            PlaceholderKind::Named(n) => format!(":{n}"),
        })
    }
    fn map_data_type(&self, dt: &DataType) -> Result<DataType, String> {
        if dt.array_dims > 0 {
            return Err("array types are not supported".into());
        }
        Ok(strip_length_units(dt))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OracleDialect;

impl Dialect for OracleDialect {
    fn name(&self) -> &'static str {
        "oracle"
    }
    fn is_identifier_part(&self, ch: char) -> bool {
        ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#')
    }
    fn supports_q_quote_strings(&self) -> bool {
        true
    }
    fn supports_colon_placeholders(&self) -> bool {
        true
    }
    fn supports_minus_operator(&self) -> bool {
        true
    }
    fn supports_fetch_first(&self) -> bool {
        true
    }
    fn offset_requires_rows(&self) -> bool {
        true
    }
    fn supports_table_alias_as(&self) -> bool {
        false
    }
    fn supports_boolean_literals(&self) -> bool {
        false
    }
    fn temporary_table_keyword(&self) -> &'static str {
        "GLOBAL TEMPORARY"
    }
    fn supports_if_exists(&self) -> bool {
        false
    }
    fn supports_multirow_values(&self) -> bool {
        false
    }
    fn requires_from_clause(&self) -> bool {
        true
    }
    fn supports_modulo_operator(&self) -> bool {
        false
    }
    fn supports_recursive_keyword(&self) -> bool {
        false
    }
    fn supports_partial_index(&self) -> bool {
        false
    }
    fn identifier_fold(&self) -> IdentifierFold {
        IdentifierFold::Upper
    }
    fn empty_string_is_null(&self) -> bool {
        true
    }
    fn integer_division_truncates(&self) -> bool {
        false
    }
    fn concat_null_is_empty(&self) -> bool {
        true
    }
    fn has_function(&self, name: &str) -> bool {
        crate::functions::contains(DialectKind::Oracle, name)
    }
    fn function_collision(&self, name: &str, nargs: usize) -> Option<&'static str> {
        match name {
            "CONCAT" if nargs != 2 => Some("Oracle CONCAT takes exactly two arguments; chain with ||"),
            "MIN" | "MAX" if nargs >= 2 => {
                Some("with several arguments this is SQLite's scalar form; use LEAST / GREATEST")
            }
            _ => None,
        }
    }
    fn function_hint(&self, name: &str) -> Option<&'static str> {
        Some(match name {
            "NOW" => "use SYSTIMESTAMP or CURRENT_TIMESTAMP",
            "STRING_AGG" | "GROUP_CONCAT" => "use LISTAGG(...) WITHIN GROUP (ORDER BY ...)",
            "IFNULL" => "use NVL or COALESCE",
            "GENERATE_SERIES" => "use CONNECT BY LEVEL <= n",
            "ARRAY_AGG" => "no direct equivalent (use a collection type)",
            "RANDOM" => "use DBMS_RANDOM.VALUE",
            "STRFTIME" => "use TO_CHAR",
            "STRPOS" | "POSITION" => "use INSTR",
            "GEN_RANDOM_UUID" => "use SYS_GUID()",
            "LEFT" => "use SUBSTR(s, 1, n)",
            "RIGHT" => "use SUBSTR(s, -n)",
            "CONCAT_WS" => "use || with NVL",
            "DATE_TRUNC" => "use TRUNC",
            "SUBSTRING" => "use SUBSTR",
            _ => return None,
        })
    }
    fn format_placeholder(&self, kind: &PlaceholderKind) -> Option<String> {
        Some(match kind {
            PlaceholderKind::Numbered(n) => format!(":{n}"),
            PlaceholderKind::Named(n) => format!(":{n}"),
        })
    }
    fn map_data_type(&self, dt: &DataType) -> Result<DataType, String> {
        if dt.array_dims > 0 {
            return Err("array types are not supported".into());
        }
        Ok(match dt.name.as_str() {
            "VARCHAR" | "CHARACTER VARYING" | "NVARCHAR" => {
                let args = if dt.args.is_empty() { Some(vec!["4000"]) } else { None };
                retype(dt, "VARCHAR2", args)
            }
            "TEXT" | "STRING" => retype(dt, "CLOB", Some(vec![])),
            "BIGINT" => retype(dt, "NUMBER", Some(vec!["19"])),
            "TINYINT" => retype(dt, "NUMBER", Some(vec!["3"])),
            "BOOLEAN" => retype(dt, "NUMBER", Some(vec!["1"])),
            "BYTEA" => retype(dt, "BLOB", Some(vec![])),
            "UUID" => retype(dt, "RAW", Some(vec!["16"])),
            "NUMERIC" => retype(dt, "NUMBER", None),
            "DATETIME" => retype(dt, "TIMESTAMP", None),
            _ => strip_length_units(dt),
        })
    }
}

/// Convenience selector, e.g. for CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialectKind {
    Postgres,
    Sqlite,
    Oracle,
}

impl DialectKind {
    /// Every supported dialect, in the order used by compatibility codes.
    pub const ALL: [DialectKind; 3] = [DialectKind::Oracle, DialectKind::Postgres, DialectKind::Sqlite];

    pub fn name(self) -> &'static str {
        match self {
            DialectKind::Oracle => "oracle",
            DialectKind::Postgres => "postgres",
            DialectKind::Sqlite => "sqlite",
        }
    }

    /// Short code used in fingerprints: `ora`, `pg`, `lite`.
    pub fn code(self) -> &'static str {
        match self {
            DialectKind::Oracle => "ora",
            DialectKind::Postgres => "pg",
            DialectKind::Sqlite => "lite",
        }
    }

    pub fn dialect(self) -> Box<dyn Dialect> {
        match self {
            DialectKind::Postgres => Box::new(PostgreSqlDialect),
            DialectKind::Sqlite => Box::new(SqliteDialect),
            DialectKind::Oracle => Box::new(OracleDialect),
        }
    }
}

impl FromStr for DialectKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Ok(Self::Postgres),
            "sqlite" | "sqlite3" | "lite" => Ok(Self::Sqlite),
            "oracle" | "plsql" | "ora" => Ok(Self::Oracle),
            other => Err(format!("unknown dialect '{other}' (expected postgres, sqlite or oracle)")),
        }
    }
}
