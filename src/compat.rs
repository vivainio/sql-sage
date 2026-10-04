//! Compatibility analysis: "what would have to change to run this SQL on
//! another engine?"
//!
//! The analysis runs the same [`Emitter`](crate::emit::Emitter) used by
//! [`transpile`](crate::transpile) in a collecting mode, so the report and the
//! real translation share one set of rules.

use crate::dialect::Dialect;
use crate::emit::Emitter;
use crate::error::{Location, ParseError};
use crate::parser::Parser;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Handled automatically by [`transpile`](crate::transpile); worth a glance.
    Rewritten,
    /// Cannot be verified: a function that is not a built-in of any supported
    /// dialect, assumed user-defined or from an extension.
    Unverified,
    /// Translates, but behaves differently on the target.
    Warning,
    /// Cannot be expressed on the target; needs a manual change.
    Incompatible,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Incompatible => "incompatible",
            Severity::Warning => "warning",
            Severity::Unverified => "unverified",
            Severity::Rewritten => "rewritten",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// 1-based statement number.
    pub statement: usize,
    /// Where that statement starts in the input.
    pub location: Option<Location>,
    pub message: String,
    /// The construct concerned, when known.
    pub at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub source: &'static str,
    pub target: &'static str,
    pub statements: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn count(&self, severity: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == severity).count()
    }

    /// `true` when nothing needs a manual change (warnings and automatic
    /// rewrites do not count).
    pub fn is_compatible(&self) -> bool {
        self.count(Severity::Incompatible) == 0
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} -> {}: {} statement{}",
            self.source,
            self.target,
            self.statements,
            if self.statements == 1 { "" } else { "s" }
        )?;
        writeln!(
            f,
            "  {} incompatible, {} warning{}, {} unverified, {} rewritten automatically",
            self.count(Severity::Incompatible),
            self.count(Severity::Warning),
            if self.count(Severity::Warning) == 1 { "" } else { "s" },
            self.count(Severity::Unverified),
            self.count(Severity::Rewritten)
        )?;
        if self.findings.is_empty() {
            return writeln!(f, "  compatible: no changes needed");
        }
        let mut current = 0;
        for fi in &self.findings {
            if fi.statement != current {
                current = fi.statement;
                match &fi.location {
                    Some(l) => write!(f, "\nstatement {current} (line {})\n", l.line)?,
                    None => write!(f, "\nstatement {current}\n")?,
                }
            }
            write!(f, "  [{}] {}", fi.severity.label(), fi.message)?;
            if let Some(at) = &fi.at {
                write!(f, "\n      at: {}", at.replace('\n', " "))?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

/// Whether a statement runs on a target without manual changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Compatible,
    /// Nothing is known to be wrong, but it calls functions that cannot be
    /// verified on the target (not built-ins of any supported dialect).
    Unverified,
    Incompatible,
}

/// Verdict for `stmt` (written for `source`) on `target`. Warnings and automatic
/// rewrites do not count against compatibility.
pub fn verdict(stmt: &crate::ast::Statement, source: &dyn Dialect, target: &dyn Dialect) -> Verdict {
    let emitter = Emitter::for_analysis(source, target);
    let _ = emitter.statement(stmt);
    let findings = emitter.take_findings();
    if findings.iter().any(|f| f.severity == Severity::Incompatible) {
        Verdict::Incompatible
    } else if findings.iter().any(|f| f.severity == Severity::Unverified) {
        Verdict::Unverified
    } else {
        Verdict::Compatible
    }
}

/// `true` unless `stmt` needs a manual change to run on `target`.
pub fn is_compatible(stmt: &crate::ast::Statement, source: &dyn Dialect, target: &dyn Dialect) -> bool {
    verdict(stmt, source, target) != Verdict::Incompatible
}

/// Parses `sql` as `source` and reports what would need to change to run it on
/// `target`. Findings are ordered by statement, then most severe first.
pub fn analyze(source: &dyn Dialect, target: &dyn Dialect, sql: &str) -> Result<Report, ParseError> {
    let stmts = Parser::new(source, sql)?.parse_statements_located()?;
    let emitter = Emitter::for_analysis(source, target);
    let mut findings: Vec<Finding> = vec![];
    for (i, (stmt, loc)) in stmts.iter().enumerate() {
        emitter.set_statement(i + 1);
        // collecting mode never fails; findings are recorded instead
        let _ = emitter.statement(stmt);
        let mut batch = emitter.take_findings();
        for f in &mut batch {
            f.location = Some(loc.clone());
        }
        batch.sort_by(|a, b| b.severity.cmp(&a.severity));
        for f in batch {
            if !findings.contains(&f) {
                findings.push(f);
            }
        }
    }
    Ok(Report { source: source.name(), target: target.name(), statements: stmts.len(), findings })
}
