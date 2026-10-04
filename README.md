# sql-sage

Parse, check, translate and review SQL for **PostgreSQL**, **SQLite** and **Oracle**.
No dependencies. Every dialect parses into one shared AST, and everything else works on that AST.

```
SQL (any dialect) --parse--> shared AST --+--> SQL for another dialect   (transpile)
                                          +--> compatibility report      (--compat)
                                          +--> readable outline          (ordered / tight)
                                          +--> query fingerprint         (--fingerprint)
```

## CLI

```sh
cargo install --path .

sql-sage -d oracle --check "SELECT * FROM t LIMIT 5"        # syntax check, exit 1 on error
sql-sage -d oracle --to postgres -f query.sql                # translate
sql-sage -d oracle --compat postgres -f query.sql            # what needs changing?
sql-sage -d postgres --to ordered "SELECT ..."               # readable outline
sql-sage -d postgres --to tight "SELECT ..."                 # symbolic outline
sql-sage -d oracle --fingerprint -f queries.sql              # tables, operations, functions
sql-sage -d sqlite --ast "SELECT 1"                          # debug AST
```

Dialects: `postgres`, `sqlite`, `oracle`. SQL comes from `-f FILE`, an argument, or stdin.

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
`%` vs `MOD`, booleans, `INSERT OR IGNORE`, and more. `RETURNING`, `IF EXISTS`, `GLOB`, array
types and similar constructs error on engines that lack them.

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
b0f00150db4166d4 select | emp:read | group aggregate | count() nvl()
```

Sections are `kind | tables | operations | functions` (`-` when empty). The leading id is a
64-bit hash of that text, handy for grouping queries in logs or finding every query that
writes to a table. From Rust: `fingerprint_sql(dialect, sql)` or `stmt.fingerprint()`, with
`reads()`, `writes()`, `is_read_only()` and `id()` on the result.

## Scope and limits

* Syntax only: no check that tables, columns or types exist.
* Covered: `SELECT` (CTEs, set operations, joins, subqueries, window functions), `INSERT`
  (incl. `ON CONFLICT`), `UPDATE`, `DELETE`, `CREATE TABLE`, `CREATE INDEX`, `DROP`.
* Not covered: PL/SQL and procedural code, `CONNECT BY`, Oracle `(+)` joins, window frames,
  `IS DISTINCT FROM`, `CREATE VIEW`, `ALTER`.
* The parser is lenient in places (for example `AUTOINCREMENT` is accepted in every dialect).
* Function and pseudo-column hints in the compatibility report are short curated lists
  (`Dialect::function_hint`); extend them as needed. `transpile` does not translate functions.
* The AST has no source spans, so findings point at a statement and the expression text.

## Development

```sh
cargo test
cargo clippy
```

## License

MIT
