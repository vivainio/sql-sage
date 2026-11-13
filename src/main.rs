use sql_sage::dialect::DialectKind;
use sql_sage::outline::Syntax;
use sql_sage::{check_compatibility, fingerprint_sql, outline_sql, parse_sql, transpile};
use std::io::Read;
use std::process::ExitCode;

const USAGE: &str = "usage: sql-sage [-d postgres|sqlite|oracle] [--to postgres|sqlite|oracle|ordered|tight] [--compat TARGET] [--fingerprint] [--schema PATH...] [--check] [--ast | --ast-compact | --ast-debug] [-f FILE | SQL]
  reads SQL from FILE, the argument, or stdin
  --to     re-emit for another dialect (errors if inexpressible), or ordered / tight for
           readable non-SQL syntaxes (ordered: words, tight: symbols)
  --compat report what would need to change to run the SQL on TARGET
           (exit code 1 if anything is incompatible)
  --fingerprint  print each statement's tables and operations, with a stable id
  --schema simulate the DDL in the given files/directories (applied in sorted order; positional
           arguments are paths) and print the resulting schema; --to picks the output dialect.
           Statements that cannot apply are reported on stderr (exit code 1)
  -v       with --schema: list every statement that was skipped because it did not parse
  --check  only validate syntax (exit code 1 on error)
  --ast    print the AST as YAML instead of normalized SQL
  --ast-compact  shorter YAML-like text for reading (not valid YAML)
  --ast-debug    Rust Debug output (verbose)";

fn main() -> ExitCode {
    let mut kind = DialectKind::Postgres;
    let (mut check, mut ast, mut ast_compact, mut ast_debug) = (false, false, false, false);
    let mut to: Option<DialectKind> = None;
    let mut to_syntax: Option<Syntax> = None;
    let mut compat: Option<DialectKind> = None;
    let mut fingerprint = false;
    let mut schema = false;
    let mut verbose = false;
    let mut files: Vec<String> = vec![];
    let mut positional: Vec<String> = vec![];
    let sql: Option<String>;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-d" | "--dialect" => match args.next().map(|v| v.parse()) {
                Some(Ok(k)) => kind = k,
                Some(Err(e)) => return fail(&e),
                None => return fail(USAGE),
            },
            "--to" => match args.next().map(|v| {
                if let Ok(syn) = v.parse::<Syntax>() {
                    to_syntax = Some(syn);
                    Ok(DialectKind::Postgres)
                } else {
                    v.parse()
                }
            }) {
                Some(Ok(k)) => to = Some(k),
                Some(Err(e)) => return fail(&e),
                None => return fail(USAGE),
            },
            "--compat" => match args.next().map(|v| v.parse()) {
                Some(Ok(k)) => compat = Some(k),
                Some(Err(e)) => return fail(&e),
                None => return fail(USAGE),
            },
            "--fingerprint" => fingerprint = true,
            "--schema" => schema = true,
            "-v" | "--verbose" => verbose = true,
            "--check" => check = true,
            "--ast" => ast = true,
            "--ast-compact" => ast_compact = true,
            "--ast-debug" => ast_debug = true,
            "-f" => match args.next() {
                Some(path) => files.push(path),
                None => return fail(USAGE),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => positional.push(a),
        }
    }
    if schema {
        files.extend(positional);
        return run_schema(kind, to, &files, verbose);
    }
    if let Some(path) = files.last() {
        match std::fs::read_to_string(path) {
            Ok(s) => sql = Some(s),
            Err(e) => return fail(&e.to_string()),
        }
    } else {
        sql = positional.pop();
    }
    let sql = sql.unwrap_or_else(|| {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok();
        s
    });
    if fingerprint {
        return match fingerprint_sql(kind, &sql) {
            Ok(fps) => {
                for f in fps {
                    println!("{} [{}] {f}", f.id(), f.compat_code());
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("syntax error: {e}")),
        };
    }
    if let Some(target) = compat {
        return match check_compatibility(kind, target, &sql) {
            Ok(report) => {
                print!("{report}");
                if report.is_compatible() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(e) => fail(&format!("syntax error: {e}")),
        };
    }
    if let Some(syn) = to_syntax {
        return match outline_sql(kind, syn, &sql) {
            Ok(out) => {
                println!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("syntax error: {e}")),
        };
    }
    if let Some(to) = to {
        return match transpile(kind, to, &sql) {
            Ok(out) => {
                println!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e.to_string()),
        };
    }
    match parse_sql(kind.dialect().as_ref(), &sql) {
        Ok(stmts) => {
            if !check {
                for (i, s) in stmts.iter().enumerate() {
                    if (ast || ast_compact) && !ast_debug && i > 0 {
                        println!("---");
                    }
                    if ast_debug {
                        println!("{s:#?}")
                    } else if ast_compact {
                        println!("{}", sql_sage::ast_dump::compact(s))
                    } else if ast {
                        println!("{}", sql_sage::ast_dump::yaml(s))
                    } else {
                        println!("{s};")
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("syntax error: {e}")),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::FAILURE
}

/// Expands directories to their `*.sql` files (sorted) and keeps plain files as given.
fn expand_paths(paths: &[String]) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = vec![];
    for p in paths {
        let path = std::path::PathBuf::from(p);
        if path.is_dir() {
            let mut found: Vec<_> = std::fs::read_dir(&path)
                .map_err(|e| format!("{p}: {e}"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("sql")))
                .collect();
            found.sort();
            out.extend(found);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

fn run_schema(kind: DialectKind, to: Option<DialectKind>, paths: &[String], verbose: bool) -> ExitCode {
    if paths.is_empty() {
        return fail("--schema needs at least one file or directory");
    }
    let files = match expand_paths(paths) {
        Ok(f) => f,
        Err(e) => return fail(&e),
    };
    let mut schema = sql_sage::schema::Schema::new(kind);
    let mut failed = false;
    for f in &files {
        let sql = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => return fail(&format!("{}: {e}", f.display())),
        };
        match schema.apply_sql(&sql) {
            Ok(applied) => {
                for e in &applied.errors {
                    eprintln!("{}: {e}", f.display());
                    failed = true;
                }
                if verbose {
                    for e in &applied.plsql {
                        eprintln!("{}: {e}", f.display());
                    }
                    for e in &applied.skipped {
                        eprintln!("{}: skipped: {e}", f.display());
                    }
                    for e in &applied.dynamic_errors {
                        eprintln!("{}: not applied (EXECUTE IMMEDIATE, probably guarded): {e}", f.display());
                    }
                } else {
                    if !applied.plsql.is_empty() {
                        eprintln!(
                            "{}: {} PL/SQL unit(s) not analyzed (only SQL in EXECUTE IMMEDIATE strings is); -v lists them",
                            f.display(),
                            applied.plsql.len()
                        );
                    }
                    if let Some(first) = applied.skipped.first() {
                        eprintln!(
                            "{}: skipped {} statement(s) that could not be parsed (first: {first}); -v lists them",
                            f.display(),
                            applied.skipped.len()
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: syntax error: {e}", f.display());
                failed = true;
            }
        }
    }
    match schema.to_sql(to.unwrap_or(kind)) {
        Ok(out) => println!("{out}"),
        Err(e) => return fail(&format!("cannot translate: {e}")),
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
