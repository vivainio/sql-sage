use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::outline::Syntax::{self, *};
use sql_sage::outline_sql;

fn o(kind: DialectKind, syntax: Syntax, sql: &str) -> String {
    outline_sql(kind, syntax, sql).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[test]
fn ordered_reads_in_execution_order() {
    let out = o(
        Postgres,
        Ordered,
        "SELECT c.name, sum(o.total) AS revenue FROM orders o LEFT JOIN customers c ON o.customer_id = c.id \
         WHERE o.status = 'open' AND o.total <> 0 GROUP BY c.name HAVING sum(o.total) > 10 \
         ORDER BY revenue DESC LIMIT 10 OFFSET 5",
    );
    assert_eq!(
        out,
        "from orders as o\n\
         left join customers as c on o.customer_id = c.id\n\
         filter o.status = 'open' and o.total != 0\n\
         group c.name\n\
         having sum(o.total) > 10\n\
         select c.name, sum(o.total) as revenue\n\
         sort revenue desc\n\
         skip 5\n\
         take 10"
    );
}

#[test]
fn ordered_subqueries_indent_two_spaces() {
    assert_eq!(
        o(Postgres, Ordered, "DELETE FROM t WHERE id IN (SELECT id FROM u WHERE x = 1) RETURNING id"),
        "delete from t\nfilter id in (\n  from u\n  filter x = 1\n  select id\n)\nreturning id"
    );
    // inside a multi-condition filter the block shifts with its `and` line
    let out = o(
        Postgres,
        Ordered,
        "SELECT 1 FROM t WHERE a = 1 AND b = 'a long string to force wrapping of the filter' AND NOT EXISTS (SELECT 1 FROM u WHERE u.id = t.id)",
    );
    assert!(out.contains("   and not exists (\n     from u\n     filter u.id = t.id\n     select 1\n   )"), "{out}");
    assert_eq!(
        o(Postgres, Ordered, "SELECT * FROM (SELECT a FROM t) s"),
        "from (\n  from t\n  select a\n) as s\nselect *"
    );
}

#[test]
fn dialects_converge_on_one_outline() {
    for syntax in [Ordered, Tight] {
        let pg = o(Postgres, syntax, "SELECT a FROM t EXCEPT SELECT a FROM u LIMIT 3");
        let ora = o(Oracle, syntax, "SELECT a FROM t MINUS SELECT a FROM u FETCH FIRST 3 ROWS ONLY");
        assert_eq!(pg, ora, "{syntax:?}");
    }
}

#[test]
fn tight_query() {
    assert_eq!(
        o(
            Postgres,
            Tight,
            "SELECT c.name, sum(o.total) AS revenue FROM orders o LEFT JOIN customers c ON o.customer_id = c.id \
             WHERE o.status = 'open' AND o.total BETWEEN 1 AND 9 AND o.k NOT IN (1, 2) GROUP BY c.name \
             HAVING sum(o.total) > 10 ORDER BY revenue DESC LIMIT 10 OFFSET 5"
        ),
        "@ orders o\n\
         <+ customers c on o.customer_id = c.id\n\
         ? o.status = 'open'\n\
         & o.total in 1..9\n\
         & o.k !in [1, 2]\n\
         by c.name\n\
         ?? sum(o.total) > 10\n\
         > c.name, sum(o.total) as revenue\n\
         ~ revenue desc\n\
         [5:10]"
    );
}

#[test]
fn tight_subqueries_ctes_and_set_ops() {
    assert_eq!(
        o(Postgres, Tight, "WITH r AS (SELECT id FROM t) SELECT * FROM r WHERE id IN (SELECT id FROM u) AND NOT EXISTS (SELECT 1 FROM z)"),
        "let r = {\n  @ t\n  > id\n}\n@ r\n? id in {\n  @ u\n  > id\n}\n& !exists {\n  @ z\n  > 1\n}\n> *"
    );
    assert_eq!(o(Postgres, Tight, "SELECT 1 UNION ALL SELECT 2"), "{\n  > 1\n}\nunion all\n{\n  > 2\n}");
    assert_eq!(o(Postgres, Tight, "SELECT * FROM (SELECT a FROM t) s"), "@ {\n  @ t\n  > a\n} s\n> *");
}

#[test]
fn tight_statements() {
    assert_eq!(
        o(Sqlite, Tight, "INSERT INTO t (a) VALUES (1), (2) ON CONFLICT (a) DO NOTHING RETURNING a"),
        "insert t(a)\nvalues\n  (1)\n  (2)\non conflict (a) nothing\nreturning a"
    );
    assert_eq!(
        o(Sqlite, Tight, "INSERT INTO t (a, b) VALUES (1, 2) ON CONFLICT (a) DO UPDATE SET b = 1, c = 2"),
        "insert t(a, b)\nvalues\n  (1, 2)\non conflict (a) update\n  b = 1\n  c = 2"
    );
    assert_eq!(
        o(Sqlite, Tight, "UPDATE t SET a = 1, b = 2 WHERE id = ?1"),
        "update t\nset\n  a = 1\n  b = 2\n? id = ?1"
    );
    assert_eq!(o(Sqlite, Tight, "DELETE FROM t WHERE id = 1"), "delete t\n? id = 1");
    assert_eq!(
        o(Postgres, Tight, "CREATE TABLE t (id int PRIMARY KEY, n text NOT NULL DEFAULT 'x', PRIMARY KEY (id), FOREIGN KEY (n) REFERENCES u(k) ON DELETE CASCADE)"),
        "table t {\n  id  int   pk\n  n   text  !null = 'x'\n  pk(id)\n  fk(n) -> u(k) on delete cascade\n}"
    );
}

#[test]
fn ordered_statements() {
    assert_eq!(
        o(Sqlite, Ordered, "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL); DROP TABLE IF EXISTS t"),
        "create table t\n  id    integer  primary key\n  name  text     not null\n\ndrop table t if exists"
    );
}
