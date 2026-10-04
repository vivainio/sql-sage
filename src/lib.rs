//! sql-sage: parse and syntax-check SQL for PostgreSQL, SQLite and Oracle.
//!
//! All dialects produce the same AST ([`ast`]); dialect differences live in
//! [`dialect::Dialect`].
//!
//! ```
//! use sql_sage::{check_syntax, parse_sql, dialect::{DialectKind, OracleDialect}};
//!
//! assert!(check_syntax(DialectKind::Postgres, "SELECT a::int FROM t").is_ok());
//! assert!(check_syntax(DialectKind::Oracle, "SELECT * FROM t LIMIT 5").is_err());
//! let stmts = parse_sql(&OracleDialect, "SELECT 1 FROM dual").unwrap();
//! assert_eq!(stmts.len(), 1);
//! ```

pub mod ast;
pub mod compat;
pub mod dialect;
pub mod emit;
pub mod error;
pub mod fingerprint;
pub mod lexer;
pub mod parser;
pub mod outline;

pub use emit::EmitError;
pub use error::{Location, ParseError};

use ast::Statement;
use dialect::{Dialect, DialectKind};

/// Parses one or more `;`-separated statements into the shared AST.
pub fn parse_sql(dialect: &dyn Dialect, sql: &str) -> Result<Vec<Statement>, ParseError> {
    parser::Parser::new(dialect, sql)?.parse_statements()
}

/// Checks that `sql` is syntactically valid for the given engine.
pub fn check_syntax(kind: DialectKind, sql: &str) -> Result<(), ParseError> {
    parse_sql(kind.dialect().as_ref(), sql).map(|_| ())
}

/// Failure of [`transpile`]: either the input did not parse or it cannot be
/// expressed in the target dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranspileError {
    Parse(ParseError),
    Emit(EmitError),
}

impl std::fmt::Display for TranspileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranspileError::Parse(e) => write!(f, "syntax error: {e}"),
            TranspileError::Emit(e) => write!(f, "cannot translate: {e}"),
        }
    }
}

impl std::error::Error for TranspileError {}

/// Parses `sql` as `from` and re-emits every statement for `to`
/// (`;`-terminated, one per line).
pub fn transpile(from: DialectKind, to: DialectKind, sql: &str) -> Result<String, TranspileError> {
    let stmts = parse_sql(from.dialect().as_ref(), sql).map_err(TranspileError::Parse)?;
    let target = to.dialect();
    let mut out = Vec::new();
    for s in &stmts {
        out.push(format!("{};", s.to_sql(target.as_ref()).map_err(TranspileError::Emit)?));
    }
    Ok(out.join("\n"))
}

/// Parses `sql` and renders it in one of the readable, non-SQL syntaxes
/// (see [`outline`]).
pub fn outline_sql(from: DialectKind, syntax: outline::Syntax, sql: &str) -> Result<String, ParseError> {
    Ok(outline::render(&parse_sql(from.dialect().as_ref(), sql)?, syntax))
}

/// Reports what would need to change to run `sql` (written for `from`) on `to`.
/// See [`compat`].
pub fn check_compatibility(from: DialectKind, to: DialectKind, sql: &str) -> Result<compat::Report, ParseError> {
    compat::analyze(from.dialect().as_ref(), to.dialect().as_ref(), sql)
}

/// Fingerprints every statement in `sql` (see [`fingerprint`]).
pub fn fingerprint_sql(from: DialectKind, sql: &str) -> Result<Vec<fingerprint::Fingerprint>, ParseError> {
    Ok(parse_sql(from.dialect().as_ref(), sql)?.iter().map(|s| s.fingerprint_for(from)).collect())
}
