use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::schema::Schema;
use sql_sage::{check_syntax, transpile};

/// Applies `sql`, asserting there are no errors, and returns the schema as DDL for the same dialect.
fn derive(kind: DialectKind, sql: &str) -> String {
    let mut s = Schema::new(kind);
    let applied = s.apply_sql(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let errors = [applied.errors, applied.skipped].concat();
    assert!(errors.is_empty(), "{sql}: {errors:?}");
    s.to_sql(kind).unwrap()
}

/// Applies `sql` and returns the messages of statements that could not apply.
fn errors(kind: DialectKind, sql: &str) -> Vec<String> {
    let mut s = Schema::new(kind);
    let applied = s.apply_sql(sql).unwrap();
    assert!(applied.skipped.is_empty(), "{sql}: {:?}", applied.skipped);
    applied.errors.into_iter().map(|e| e.message).collect()
}

#[test]
fn alter_table_round_trips() {
    for sql in [
        "ALTER TABLE t ADD COLUMN a INT DEFAULT 1 NOT NULL",
        "ALTER TABLE IF EXISTS t ADD COLUMN IF NOT EXISTS a INT, DROP COLUMN IF EXISTS b CASCADE",
        "ALTER TABLE t RENAME COLUMN a TO b",
        "ALTER TABLE t RENAME TO u",
        "ALTER TABLE t ALTER COLUMN a TYPE TEXT, ALTER COLUMN b SET NOT NULL, ALTER COLUMN c DROP NOT NULL",
        "ALTER TABLE t ALTER COLUMN a SET DEFAULT 5, ALTER COLUMN b DROP DEFAULT",
        "ALTER TABLE t ADD CONSTRAINT fk FOREIGN KEY (a) REFERENCES u (id) ON DELETE CASCADE",
        "ALTER TABLE t ADD PRIMARY KEY (a, b), DROP CONSTRAINT IF EXISTS c",
    ] {
        assert_eq!(transpile(Postgres, Postgres, sql).unwrap(), format!("{sql};"));
    }
}

#[test]
fn alter_table_dialect_forms() {
    assert!(check_syntax(Postgres, "ALTER TABLE t ALTER COLUMN a SET DATA TYPE bigint USING a::bigint").is_ok());
    assert!(check_syntax(Oracle, "ALTER TABLE t ADD (a NUMBER, b VARCHAR2(5))").is_ok());
    assert!(check_syntax(Oracle, "ALTER TABLE t MODIFY (a NUMBER(5) NOT NULL, b VARCHAR2(9))").is_ok());
    assert!(check_syntax(Oracle, "ALTER TABLE t DROP (a, b)").is_ok());
    assert!(check_syntax(Sqlite, "ALTER TABLE t ADD COLUMN a").is_ok());
    assert!(check_syntax(Postgres, "ALTER TABLE t FROB").is_err());
    assert_eq!(
        transpile(Oracle, Postgres, "ALTER TABLE t ADD (a NUMBER, b NUMBER)").unwrap(),
        "ALTER TABLE t ADD COLUMN a NUMERIC, ADD COLUMN b NUMERIC;"
    );
    assert_eq!(
        transpile(Postgres, Oracle, "ALTER TABLE t ADD COLUMN a INT, ALTER COLUMN b SET NOT NULL").unwrap(),
        "ALTER TABLE t ADD a INT, MODIFY b NOT NULL;"
    );
    assert!(transpile(Postgres, Sqlite, "ALTER TABLE t ALTER COLUMN a TYPE TEXT").is_err());
}

#[test]
fn create_then_alter() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE users (id serial PRIMARY KEY, email text NOT NULL, legacy int);
             ALTER TABLE users DROP COLUMN legacy;
             ALTER TABLE users ADD COLUMN name varchar(100) DEFAULT 'x' NOT NULL;
             ALTER TABLE users RENAME COLUMN email TO mail;
             ALTER TABLE users ALTER COLUMN name TYPE text, ALTER COLUMN mail DROP NOT NULL;"
        ),
        "CREATE TABLE users (id SERIAL PRIMARY KEY, mail TEXT, name TEXT DEFAULT 'x' NOT NULL);"
    );
}

#[test]
fn defaults_and_not_null_replace_each_other() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE t (a int DEFAULT 1, b int NULL);
             ALTER TABLE t ALTER COLUMN a SET DEFAULT 2;
             ALTER TABLE t ALTER COLUMN b SET NOT NULL;
             ALTER TABLE t ALTER COLUMN b SET NOT NULL;"
        ),
        "CREATE TABLE t (a INT DEFAULT 2, b INT NOT NULL);"
    );
    assert_eq!(
        derive(Postgres, "CREATE TABLE t (a int DEFAULT 1); ALTER TABLE t ALTER COLUMN a DROP DEFAULT;"),
        "CREATE TABLE t (a INT);"
    );
}

#[test]
fn rename_table_follows_references() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE a (id int PRIMARY KEY);
             CREATE TABLE b (a_id int REFERENCES a(id), FOREIGN KEY (a_id) REFERENCES a (id));
             CREATE INDEX ix ON a (id);
             ALTER TABLE a RENAME TO c;"
        ),
        "CREATE TABLE c (id INT PRIMARY KEY);\n\
         CREATE TABLE b (a_id INT REFERENCES c (id), FOREIGN KEY (a_id) REFERENCES c (id));\n\
         CREATE INDEX ix ON c (id);"
    );
}

#[test]
fn drop_column_takes_constraints_and_indexes_with_it() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE t (a int, b int, c int, UNIQUE (a, b), UNIQUE (c));
             CREATE INDEX ia ON t (a);
             CREATE INDEX ic ON t (c);
             ALTER TABLE t DROP COLUMN a;"
        ),
        "CREATE TABLE t (b INT, c INT, UNIQUE (c));\nCREATE INDEX ic ON t (c);"
    );
}

#[test]
fn rename_column_updates_constraints_and_indexes() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE t (a int, b int, PRIMARY KEY (a, b));
             CREATE INDEX ia ON t (a);
             ALTER TABLE t RENAME COLUMN a TO z;"
        ),
        "CREATE TABLE t (z INT, b INT, PRIMARY KEY (z, b));\nCREATE INDEX ia ON t (z);"
    );
}

#[test]
fn drop_constraint_by_name_and_by_generated_name() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE t (id int PRIMARY KEY, a int UNIQUE, b int REFERENCES u(id), c int, CONSTRAINT ck CHECK (c > 0));
             ALTER TABLE t DROP CONSTRAINT t_pkey;
             ALTER TABLE t DROP CONSTRAINT t_a_key;
             ALTER TABLE t DROP CONSTRAINT t_b_fkey;
             ALTER TABLE t DROP CONSTRAINT ck;"
        ),
        "CREATE TABLE t (id INT, a INT, b INT, c INT);"
    );
    assert_eq!(errors(Postgres, "CREATE TABLE t (a int); ALTER TABLE t DROP CONSTRAINT nope;").len(), 1);
    assert!(errors(Postgres, "CREATE TABLE t (a int); ALTER TABLE t DROP CONSTRAINT IF EXISTS nope;").is_empty());
}

#[test]
fn drop_table_removes_indexes_and_cascade_removes_foreign_keys() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TABLE a (id int PRIMARY KEY);
             CREATE TABLE b (x int, a_id int REFERENCES a(id), FOREIGN KEY (a_id) REFERENCES a (id));
             CREATE INDEX ia ON a (id);
             DROP TABLE a CASCADE;"
        ),
        "CREATE TABLE b (x INT, a_id INT);"
    );
    assert_eq!(
        derive(Postgres, "CREATE TABLE a (id int); CREATE INDEX i ON a (id); DROP INDEX i;"),
        "CREATE TABLE a (id INT);"
    );
}

#[test]
fn impossible_migrations_are_reported_and_skipped() {
    assert_eq!(errors(Postgres, "CREATE TABLE t (a int); CREATE TABLE t (a int);"), ["table t already exists"]);
    assert_eq!(errors(Postgres, "ALTER TABLE nope ADD COLUMN a int;"), ["table nope does not exist"]);
    assert_eq!(
        errors(Postgres, "CREATE TABLE t (a int); ALTER TABLE t DROP COLUMN b; ALTER TABLE t ADD COLUMN a int;"),
        ["column b of table t does not exist", "column a already exists"]
    );
    assert_eq!(errors(Postgres, "DROP TABLE nope;"), ["table nope does not exist"]);
    assert_eq!(errors(Postgres, "CREATE INDEX i ON nope (a);"), ["table nope does not exist"]);
    assert_eq!(errors(Postgres, "CREATE TABLE t (a int, A int);"), ["column A specified more than once"]);
    // IF [NOT] EXISTS makes them fine.
    assert!(errors(
        Postgres,
        "CREATE TABLE t (a int); CREATE TABLE IF NOT EXISTS t (a int); DROP TABLE IF EXISTS nope; \
         ALTER TABLE IF EXISTS nope ADD COLUMN a int; ALTER TABLE t ADD COLUMN IF NOT EXISTS a int; \
         ALTER TABLE t DROP COLUMN IF EXISTS zz;"
    )
    .is_empty());
}

#[test]
fn a_failing_action_leaves_the_table_untouched() {
    let mut s = Schema::new(Postgres);
    let errs = s.apply_sql("CREATE TABLE t (a int); ALTER TABLE t ADD COLUMN b int, DROP COLUMN nope;").unwrap().errors;
    assert_eq!(errs.len(), 1);
    assert_eq!(s.to_sql(Postgres).unwrap(), "CREATE TABLE t (a INT);");
    assert_eq!(errs[0].location.as_ref().map(|l| l.line), Some(1));
}

#[test]
fn names_are_folded_per_dialect_and_schema_prefix_is_ignored() {
    assert_eq!(
        derive(Postgres, "CREATE TABLE public.Users (Id int); ALTER TABLE USERS ADD COLUMN x int;"),
        "CREATE TABLE public.Users (Id INT, x INT);"
    );
    // Quoted identifiers keep their case in Postgres, so these are different tables.
    assert_eq!(derive(Postgres, "CREATE TABLE \"T\" (a int); CREATE TABLE t (a int);").lines().count(), 2);
    assert_eq!(derive(Oracle, "CREATE TABLE t (a int); ALTER TABLE T ADD (b int);").lines().count(), 1);
    assert_eq!(derive(Sqlite, "CREATE TABLE t (a); ALTER TABLE T ADD COLUMN b;").lines().count(), 1);
}

#[test]
fn temp_tables_dml_and_views_are_ignored() {
    assert_eq!(
        derive(
            Postgres,
            "CREATE TEMP TABLE scratch (a int); CREATE TABLE t (a int); INSERT INTO t VALUES (1);
             UPDATE t SET a = 2; DELETE FROM t; SELECT * FROM t; DROP VIEW IF EXISTS v;"
        ),
        "CREATE TABLE t (a INT);"
    );
}

#[test]
fn output_can_target_another_dialect() {
    let mut s = Schema::new(Postgres);
    s.apply_sql("CREATE TABLE t (id bigint PRIMARY KEY, s text); CREATE INDEX i ON t (s);").unwrap();
    assert_eq!(
        s.to_sql(Oracle).unwrap(),
        "CREATE TABLE t (id NUMBER(19) PRIMARY KEY, s CLOB);\nCREATE INDEX i ON t (s);"
    );
}

#[test]
fn sqlite_rebuild_pattern() {
    assert_eq!(
        derive(
            Sqlite,
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT);
             CREATE TABLE t_new (a INTEGER PRIMARY KEY, b TEXT, c INTEGER NOT NULL DEFAULT 0);
             INSERT INTO t_new (a, b) SELECT a, b FROM t;
             DROP TABLE t;
             ALTER TABLE t_new RENAME TO t;"
        ),
        "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT, c INTEGER DEFAULT 0 NOT NULL);"
    );
}

#[test]
fn alter_table_fingerprint() {
    let fp = sql_sage::fingerprint_sql(Postgres, "ALTER TABLE t ADD COLUMN a int").unwrap();
    assert_eq!(fp[0].to_string(), "alter_table | t:alter | - | -");
}
