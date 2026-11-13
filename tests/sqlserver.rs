//! The minimal, parse-only SQL Server dialect: bracketed identifiers, `GO` batches, `IF`.

use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::schema::Schema;
use sql_sage::{check_syntax, transpile};

#[test]
fn bracketed_identifiers_and_xml_methods() {
    let sql = "DELETE FROM [dbo].[products] WHERE [host] = 'h' AND CAST([descriptor] AS xml).exist('/d/id[text() = ''A'']') = 1";
    assert!(check_syntax(SqlServer, sql).is_ok());
    // Printed with standard double quotes, which SQL Server accepts.
    assert_eq!(
        transpile(SqlServer, SqlServer, sql).unwrap(),
        "DELETE FROM \"dbo\".\"products\" WHERE \"host\" = 'h' AND CAST(\"descriptor\" AS XML).exist('/d/id[text() = ''A'']') = 1;"
    );
    assert!(check_syntax(SqlServer, "SELECT @x, #tmp.a FROM #tmp").is_ok());
    assert!(check_syntax(Oracle, "SELECT [a] FROM t").is_err());
}

#[test]
fn go_separates_batches() {
    let mut s = Schema::new(SqlServer);
    let applied = s
        .apply_sql(
            "CREATE TABLE [t] ([a] int)
             GO
             INSERT INTO [t] ([a]) VALUES (1)
             GO 2
             delete from [t] where [a] = 1
             GO
             ALTER TABLE [t] ADD [b] int
             GO",
        )
        .unwrap();
    assert_eq!(applied.skipped, vec![]);
    assert_eq!(applied.errors, vec![]);
    assert_eq!(s.table("t").unwrap().columns.len(), 2);
}

#[test]
fn if_statements() {
    let mut s = Schema::new(SqlServer);
    let applied = s
        .apply_sql(
            "IF DB_ID('x') IS NULL
               CREATE DATABASE x
             GO
             IF OBJECT_ID('t') IS NULL DELETE FROM t ELSE TRUNCATE TABLE t
             GO",
        )
        .unwrap();
    assert_eq!(applied.skipped, vec![]);
    assert!(transpile(SqlServer, Oracle, "IF a = 1 DELETE FROM t").is_err());
}

#[test]
fn it_is_not_part_of_the_compatibility_set() {
    assert_eq!(DialectKind::ALL.len(), 3);
    assert_eq!("tsql".parse::<DialectKind>(), Ok(SqlServer));
    assert_eq!(SqlServer.name(), "sqlserver");
}
