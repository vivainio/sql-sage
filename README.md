# sql-sage

Parse, check, translate and review SQL for **PostgreSQL**, **SQLite** and **Oracle**.
No dependencies. Every dialect parses into one shared AST, and everything else works on that AST.

```
SQL (any dialect) --parse--> shared AST --+--> SQL for another dialect   (transpile)
                                          +--> compatibility report      (--compat)
                                          +--> readable outline          (ordered / tight)
                                          +--> query fingerprint         (--fingerprint)
```

## Install

The easiest way is [uv](https://docs.astral.sh/uv/), which installs the command into its own isolated
environment and puts `sql-sage` on your `PATH`:

```sh
uv tool install sql-sage       # install
sql-sage --help

uvx sql-sage -d oracle --check "SELECT 1 FROM dual"   # or run once without installing

uv tool upgrade sql-sage       # update
uv tool uninstall sql-sage     # remove
```

`pipx install sql-sage` and `pip install sql-sage` work too.

The PyPI package ships the `sql-sage` command as a prebuilt wheel (Linux, macOS, Windows); it has no
Python API. Prebuilt binaries are also attached to each
[GitHub release](https://github.com/vivainio/sql-sage/releases), or build from source with
`cargo install --git https://github.com/vivainio/sql-sage`. As a Rust library:

```toml
sql-sage = { git = "https://github.com/vivainio/sql-sage" }
```

## CLI

```sh
sql-sage -d oracle --check "SELECT * FROM t LIMIT 5"        # syntax check, exit 1 on error
sql-sage -d oracle --to postgres -f query.sql                # translate
sql-sage -d oracle --compat postgres -f query.sql            # what needs changing?
sql-sage -d postgres --to ordered "SELECT ..."               # readable outline
sql-sage -d postgres --to tight "SELECT ..."                 # symbolic outline
sql-sage -d oracle --fingerprint -f queries.sql              # tables, operations, functions
sql-sage -d postgres --schema migrations/                    # derive the schema from migrations
sql-sage -d sqlite --ast "SELECT 1"                          # AST as YAML
sql-sage -d sqlite --ast-compact "SELECT 1"                  # shorter YAML-like text (not valid YAML)
sql-sage -d sqlite --ast-debug "SELECT 1"                    # verbose Rust Debug output
```

Dialects: `postgres`, `sqlite`, `oracle`, and `sqlserver` (parse-only: `[bracketed]` identifiers, `GO` batches
and `IF cond statement`; it is left out of `--compat` and fingerprint codes). SQL comes from `-f FILE`,
an argument, or stdin.

## Library

```rust
use sql_sage::{check_syntax, check_compatibility, transpile, outline_sql};
use sql_sage::dialect::DialectKind::*;
use sql_sage::outline::Syntax;

check_syntax(Postgres, "SELECT a::int FROM t")?;                          // syntax check
let sql = transpile(Oracle, Postgres, "SELECT 1 FROM dual MINUS SELECT 2 FROM dual")?;
let report = check_compatibility(Oracle, Postgres, "SELECT NVL(a, 0) FROM t")?;
assert!(!report.is_compatible());
let text = outline_sql(Postgres, Syntax::Ordered, "SELECT a FROM t WHERE b = 1")?;
```

`parse_sql(&dyn Dialect, sql)` returns the AST (`sql_sage::ast`). Dialects implement the
`Dialect` trait, which only describes what differs between engines.

## Translation

`transpile` rewrites what can be rewritten safely and returns an error for what cannot:
pagination (`LIMIT`/`OFFSET` vs `FETCH FIRST`), `MINUS`/`EXCEPT`, `FROM dual`, bind parameters
(`$1`, `?`, `:name`), data types (`VARCHAR2`/`VARCHAR`, `NUMBER`/`NUMERIC`, ...), `ILIKE`,
`%` vs `MOD`, booleans, Oracle/PostgreSQL functions SQLite lacks, rewritten to SQLite constructs when they have an
exact equivalent (`NVL`, `NVL2`, `DECODE`, `GREATEST`/`LEAST`, `LPAD`/`RPAD`, `LISTAGG`/`STRING_AGG`,
`SYSDATE`/`NOW`/`CURRENT_DATE`, `ADD_MONTHS`, `LAST_DAY`, `TRUNC` on dates, `CHR`, `LNNVL`, `NANVL`, `INSTR` with
a start, `TO_NUMBER(x)`, `MONTHS_BETWEEN`, `INITCAP` (ASCII letters), `STDDEV`/`VARIANCE` and their `_POP`/`_SAMP` forms, and `TO_CHAR`/`TO_DATE` with
masks `strftime` can reproduce; other masks stay an error), `INSERT OR IGNORE`, and more. `RETURNING`, `IF EXISTS`, `GLOB`, array
types and similar constructs error on engines that lack them.

Oracle scripts to SQLite also get: `TO_DATE`/`TO_TIMESTAMP` with literal arguments evaluated at
translation time (`TO_DATE('01/02/2020', 'MM/DD/YYYY')` becomes `'2020-01-02'`; many masks, incl.
`RR`, `MON`, `AM`, `FF`), `SYS_GUID()`, `SYS_EXTRACT_UTC`, `HEXTORAW('..')`, `NLSSORT(x, 'NLS_SORT=BINARY_CI')`,
`CREATE OR REPLACE [FORCE] VIEW` (as `DROP VIEW IF EXISTS` + `CREATE VIEW`), `COMMIT`/`ROLLBACK`/`SAVEPOINT`,
`INSERT ALL ... FROM dual` (one `INSERT` per target), `ALTER INDEX ... REBUILD` (`REINDEX`), and
`ALTER TABLE` storage clauses (`MOVE`, `LOGGING`, `PARALLEL`, `COMPRESS`, ... are dropped, being no-ops there).
Not translatable: PL/SQL, sequences, `ALTER TABLE` that changes a column or adds/drops a constraint (SQLite
cannot), `MERGE`, `CONNECT BY`, and calls to the script's own functions and procedures. For a script that
`ALTER`s its tables, derive the final tables instead: `sql-sage -d oracle --to sqlite --schema scripts/`
(objects SQLite cannot hold, such as sequences, are left out with a warning).

## Compatibility report

```
oracle -> postgres: 1 statement
  2 incompatible, 1 warning, 1 rewritten automatically

statement 1 (line 1)
  [incompatible] function NVL does not exist in postgres: use COALESCE
      at: NVL(e.bonus, 0)
  [warning] '' is NULL in oracle but an empty string in postgres
  [rewritten] set difference spelled EXCEPT on the target
```

* **incompatible**: needs a manual change
* **warning**: translates, but behaves differently (`''` is NULL, `||` with NULL, integer
  division, identifier case folding)
* **rewritten**: handled automatically by `transpile`

The report and `transpile` share one set of rules.

## Readable outlines

Neither outline is SQL and neither can be parsed back. Both read in execution order.

`ordered` (words):

```
from orders as o
left join customers as c on o.customer_id = c.id
filter o.status = 'open'
   and not exists (
     from blocked as b
     filter b.customer_id = c.id
     select 1
   )
group c.name
select c.name, sum(o.total) as revenue
sort revenue desc
take 10
```

`tight` (symbols: `@` from, `+ <+ +> <+> x` joins, `?` filter, `&` and, `by` group,
`??` having, `>` select, `~` sort, `[offset:limit]`, `{ }` subqueries):

```
@ orders o
<+ customers c on o.customer_id = c.id
? o.status = 'open'
& !exists {
  @ blocked b
  ? b.customer_id = c.id
  > 1
}
by c.name
> c.name, sum(o.total) as revenue
~ revenue desc
[:10]
```

## Fingerprint

A fingerprint says which **tables** a statement touches (and how), which **operations** it
applies and which **functions** it calls. Literals, columns, aliases, bind parameters, CTE
names and dialect spelling are ignored, so the same query shape gets the same fingerprint
on every dialect and with any parameter values.

```
$ sql-sage -d oracle --fingerprint "SELECT NVL(a,0), count(*) FROM emp GROUP BY a"
b0f00150db4166d4 [ora] select | emp:read | group aggregate | count() nvl()
```

Sections are `kind | tables | operations | functions` (`-` when empty). The leading id is a
64-bit hash of that text, handy for grouping queries in logs or finding every query that
writes to a table.

The bracketed code lists the dialects the statement runs on **without manual changes**:
`ora`, `pg`, `lite` (for example `[ora,pg,lite]` for portable SQL, `[pg,lite]` for
`RETURNING`, `[ora]` for `RATIO_TO_REPORT`, `-` when none fit). A `?` means nothing is known to be wrong
but the statement calls functions that cannot be verified there (`[ora?,pg,lite?]` for a
user-defined function written for Postgres). It uses the same analysis as `--compat` against
every dialect, the source one included. Warnings and automatic rewrites do not reduce
compatibility. The code is not part of the id, so the id depends only on query shape.

From Rust: `fingerprint_sql(dialect, sql)` or `stmt.fingerprint_for(dialect)` (use
`stmt.fingerprint()` for shape only), with `reads()`, `writes()`, `is_read_only()`, `id()`
and `compat_code()` on the result.

## Schema from migrations

`--schema` simulates DDL: it applies `CREATE TABLE`, `ALTER TABLE`, `CREATE INDEX`, `CREATE SEQUENCE`,
`CREATE VIEW` and `DROP`
from the given files in order (directories contribute their `*.sql` files, sorted) and prints
the resulting schema as `CREATE` statements. `--to` picks the output dialect.

```
$ sql-sage -d postgres --schema 001_init.sql 002_change.sql
002_change.sql: table nope does not exist at line 5, column 1
CREATE TABLE accounts (id SERIAL PRIMARY KEY, mail TEXT NOT NULL, name VARCHAR(100) DEFAULT 'x' NOT NULL);
CREATE TABLE orders (id INT PRIMARY KEY, user_id INT REFERENCES accounts (id));
CREATE INDEX ix_o ON orders (user_id);
```

Statements that do not parse are skipped instead of stopping the run: a summary goes to stderr,
`-v` lists each one, and they do not affect the exit code. In Oracle scripts, SQL*Plus commands
(`SET`, `WHENEVER`, `DEFINE`, a lone `/`), a leading BOM, UTF-16 and Latin-1 files, `N'...'`
strings and physical clauses (`TABLESPACE`, `PCTFREE`, `COMPRESS ...`, `ROW STORE ...`,
`USING INDEX ...`, `ENABLE`/`NOVALIDATE`, `LOB (...) STORE AS ...`, `PARTITION BY ...`) are
understood; the physical clauses are accepted but dropped from the dump. `ALTER TABLE` clauses
that are not modeled (`MOVE`, `ENABLE CONSTRAINT`, `LOGGING`, partition maintenance, ...) and
statements such as `COMMIT`, `GRANT`, `COMMENT ON`, `CALL`, `ALTER VIEW ... COMPILE` or
`DROP FUNCTION` are accepted and have no effect on the schema.

PL/SQL units (anonymous blocks, stored functions and procedures, triggers including compound
triggers) are parsed completely, but only for their syntax: a schema is not derived from their
logic, and the summary counts them. The exception is the SQL in `EXECUTE IMMEDIATE '...'` strings
of anonymous `BEGIN ... END` / `DECLARE ... END` blocks (also parenthesised, or literals joined with
`||`): it is extracted and applied in place, as if the block ran it unconditionally. Strings built
from variables, and anything inside functions, procedures and triggers, are not seen. Extracted
statements that cannot apply (the table already exists, the column is missing) are usually
guarded by an `IF`, so they are not errors; `-v` lists them as "not applied".

Migrations that cannot apply (dropping a missing column, creating a table twice, ...) are
reported on stderr and skipped, and the exit code is 1. Dropping a column also drops the
constraints and indexes that use it; renaming a table or column updates references to it.
`DROP CONSTRAINT` also understands PostgreSQL's generated names (`t_pkey`, `t_col_key`,
`t_col_fkey`). Names match on their last part (`public.t` is `t`). Queries and DML are ignored;
materialized views, partitioning and procedural code are not modeled. Dropping a view that is
not tracked is not an error. From Rust: `sql_sage::schema::Schema`.

`CREATE TABLE ... AS SELECT` becomes an ordinary table. Its columns are the select list (names
from aliases, the column list, or the columns themselves; `*` and `t.*` are expanded from known
tables). Types are copied from source columns and `CAST`s; for anything else (functions,
arithmetic, unknown tables) the type is printed as `UNKNOWN`, since there is no type inference.
`NOT NULL` and other constraints are not carried over.

## Function catalogs

Function availability is judged against catalogs of the built-in functions of the latest
release of each engine, generated from the engines' own sources by
`tools/gen_function_catalogs.py` into `data/functions/`:

| engine | source | names |
|---|---|---|
| PostgreSQL 18 | `pg_proc.dat` + `system_functions.sql` (REL_18_STABLE) | 2793 |
| SQLite 3.53.4 | `PRAGMA function_list` of the official amalgamation, built with the CLI options | 137 |
| Oracle AI Database 26ai | function pages of the SQL Language Reference (plus conditions and byte/char variants) | 342 |

For each call and target dialect: in the target's catalog means fine; missing from the target
but present in another dialect is *incompatible* (with a replacement hint such as `NVL` ->
`COALESCE` where known); in no catalog is *unverified* (probably user-defined or from an
extension) and is trusted for the dialect the SQL was written in. A few names exist
everywhere but mean different things (`DECODE`, `CONCAT` with 3+ arguments, `MAX(a, b)`);
those are flagged as collisions. Schema- or package-qualified calls are ignored.

Re-run the script to move to newer releases:
`tools/gen_function_catalogs.py --pg 19 --sqlite <path> --oracle 26` (needs network, `gcc`, `unzip`).

## AST dump

`--ast` prints the shared AST as **YAML** that any YAML parser loads. The node type is a key;
empty fields (`None`, `[]`, `false`, zero counts) are left out, wrapper nodes collapsed, and
small nodes inlined. Several statements become a multi-document stream (`---`).

```
$ sql-sage -d sqlite --ast "SELECT a FROM t WHERE b > 1 LIMIT 5, 10"
Query:
  body:
    Select:
      projection: [{Identifier: a}]
      from: [{TableWithJoins: {relation: {Table: {name: t}}}}]
      selection: {BinaryOp: {left: {Identifier: b}, op: Gt, right: {Number: 1}}}
  limit: {Number: 10}
  offset: {Number: 5}
```

Strings stay strings (`'true'`, `'1'`, `'2020-01-01'` are quoted), numbers and booleans are typed,
quoted identifiers appear as `{Quoted: name}`, and keys YAML 1.1 would read as booleans (`On`)
are quoted. `--ast-compact` is a shorter form for reading (`body: Select` with fields nested
under it) and is **not** valid YAML; `--ast-debug` is the verbose Rust `Debug` output.

From Rust: `sql_sage::ast_dump::{yaml, compact}`. The shape follows the AST and is not a stable
interchange format.

## Scope and limits

* Syntax only: no check that tables, columns or types exist.
* Covered: `SELECT` (CTEs, set operations, joins, subqueries, window functions), `INSERT`
  (incl. `ON CONFLICT`, Oracle `INSERT ALL`), `UPDATE`, `DELETE`, `MERGE`, `TRUNCATE`,
  `CREATE TABLE` (incl. `AS SELECT`), `ALTER TABLE`, `CREATE INDEX`, `CREATE SEQUENCE`,
  `CREATE VIEW`, `DROP`, and Oracle's `START WITH ... CONNECT BY`.
* PL/SQL: anonymous blocks, functions, procedures and triggers (declarations, `IF`/`CASE`/loops,
  cursors, `EXECUTE IMMEDIATE`, exception handlers, `SELECT ... INTO`, `RETURNING ... INTO`,
  `:NEW`/`:OLD`, named arguments) are parsed into `Statement::Plsql`. Types are kept as text.
  Packages and object types are not parsed.
* Accepted but not modeled: `COMMIT`, `GRANT`, `COMMENT ON`, `CALL`, `ALTER` of anything but
  tables, and `CREATE` of anything but the above (synonyms, materialized views, ...).
* Not covered: Oracle `(+)` joins, window frames, `IS DISTINCT FROM`, Oracle packages and object
  types, and most of T-SQL (variables, `BEGIN ... END`, `TOP`, `OUTPUT`, ...).
* The parser is lenient in places (for example `AUTOINCREMENT` is accepted in every dialect).
* Function catalogs contain names only (no argument types or per-version availability) and
  cover core built-ins; extension functions such as `pg_trgm`'s are *unverified*. The Postgres
  catalog is every callable `pg_proc` function, including internal ones. The Oracle catalog is
  scraped from the docs, so a function missing from those pages would be reported as missing.
  Replacement hints (`Dialect::function_hint`) and collisions are short curated lists.
  `transpile` does not translate functions.
* The AST has no source spans, so findings point at a statement and the expression text.

## Development

```sh
cargo test
cargo clippy
```

The library has no dependencies; `yaml-rust2` is a dev-dependency used to check that `--ast`
output is valid YAML.

## Releases

The version in `Cargo.toml` is a placeholder (`0.0.0-dev`); the release workflow takes the real one
from the git tag. To cut a release, let `gh` create the tag:

```sh
gh release create vX.Y.Z --title "vX.Y.Z" --notes "..."
```

That triggers `.github/workflows/release.yml`, which builds binaries for Linux, macOS (Intel and
Apple Silicon) and Windows with checksums, builds wheels and an sdist with maturin, attaches the
binaries to the release and publishes the wheels to PyPI via
[Trusted Publishing](https://docs.pypi.org/trusted-publishers/) (no token stored). One-time setup on
PyPI: add a (pending) publisher for project `sql-sage` with owner `vivainio`, repository `sql-sage`,
workflow `release.yml` and environment `pypi`.

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `clippy -D warnings` and the tests on Linux,
macOS and Windows, and smoke-tests a built wheel.

## License

MIT
