use sql_sage::dialect::DialectKind;
use sql_sage::outline::Syntax;
use sql_sage::{check_compatibility, fingerprint_sql, outline_sql, parse_sql, transpile};
use std::io::Read;
use std::process::ExitCode;

const USAGE: &str = "usage: sql-sage [-d postgres|sqlite|oracle] [--to postgres|sqlite|oracle|ordered|tight] [--compat TARGET] [--fingerprint] [--check] [--ast] [-f FILE | SQL]
  reads SQL from FILE, the argument, or stdin
  --to     re-emit for another dialect (errors if inexpressible), or ordered / tight for
           readable non-SQL syntaxes (ordered: words, tight: symbols)
  --compat report what would need to change to run the SQL on TARGET
           (exit code 1 if anything is incompatible)
  --fingerprint  print each statement's tables and operations, with a stable id
  --check  only validate syntax (exit code 1 on error)
  --ast    print the debug AST instead of normalized SQL";

fn main() -> ExitCode {
    let mut kind = DialectKind::Postgres;
    let (mut check, mut ast) = (false, false);
    let mut to: Option<DialectKind> = None;
    let mut to_syntax: Option<Syntax> = None;
    let mut compat: Option<DialectKind> = None;
    let mut fingerprint = false;
    let mut sql: Option<String> = None;
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
            "--check" => check = true,
            "--ast" => ast = true,
            "-f" => match args.next().map(std::fs::read_to_string) {
                Some(Ok(s)) => sql = Some(s),
                Some(Err(e)) => return fail(&e.to_string()),
                None => return fail(USAGE),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => sql = Some(a),
        }
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
                    println!("{} {f}", f.id());
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
                if report.is_compatible() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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
                for s in &stmts {
                    if ast { println!("{s:#?}") } else { println!("{s};") }
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
