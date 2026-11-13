//! PL/SQL blocks, functions, procedures and triggers. All names are made up.

use sql_sage::ast::*;
use sql_sage::dialect::DialectKind::{self, *};
use sql_sage::dialect::OracleDialect;
use sql_sage::schema::Schema;
use sql_sage::{check_syntax, parse_sql, transpile};

fn unit(sql: &str) -> PlsqlUnit {
    let mut stmts = parse_sql(&OracleDialect, sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    assert_eq!(stmts.len(), 1, "{sql}");
    match stmts.pop().unwrap() {
        Statement::Plsql(u) => *u,
        other => panic!("not a PL/SQL unit: {other:?}"),
    }
}

fn ok(kind: DialectKind, sql: &str) {
    if let Err(e) = check_syntax(kind, sql) {
        panic!("{sql}\n{e}");
    }
}

#[test]
fn anonymous_block_with_declarations_control_flow_and_handlers() {
    let u = unit(
        "DECLARE
           v_count NUMBER := 0;
           v_name  CONSTANT VARCHAR2(30 CHAR) NOT NULL := 'x';
           v_row   orders%ROWTYPE;
           v_col   orders.id%TYPE;
           e_bad   EXCEPTION;
           PRAGMA EXCEPTION_INIT(e_bad, -20001);
           TYPE t_ids IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
           CURSOR c_o (p_min NUMBER) IS SELECT id FROM orders WHERE total > p_min;
           PROCEDURE log_it(p_msg IN VARCHAR2 DEFAULT 'none') IS
           BEGIN
             DBMS_OUTPUT.PUT_LINE(p_msg);
           END log_it;
         BEGIN
           <<outer>>
           FOR i IN 1..10 LOOP
             IF i = 3 THEN
               CONTINUE;
             ELSIF i > 8 THEN
               EXIT outer;
             ELSE
               v_count := v_count + i;
             END IF;
           END LOOP outer;
           FOR r IN (SELECT id FROM orders) LOOP
             log_it('row ' || r.id);
           END LOOP;
           FOR r IN c_o(5) LOOP NULL; END LOOP;
           WHILE v_count > 0 LOOP
             v_count := v_count - 1;
             EXIT WHEN v_count < 5;
           END LOOP;
           CASE v_count WHEN 1 THEN NULL; ELSE log_it('many'); END CASE;
           SELECT COUNT(*) INTO v_count FROM orders WHERE id = :new_id;
           UPDATE orders SET total = 0 WHERE id = 1 RETURNING total INTO v_count;
           IF SQL%ROWCOUNT = 0 AND NOT c_o%ISOPEN THEN
             RAISE e_bad;
           END IF;
           OPEN c_o(1);
           FETCH c_o BULK COLLECT INTO v_count LIMIT 100;
           CLOSE c_o;
           EXECUTE IMMEDIATE 'DROP TABLE tmp' || v_count;
           EXECUTE IMMEDIATE 'SELECT 1 FROM dual' INTO v_count USING IN v_name;
           BEGIN
             RAISE_APPLICATION_ERROR(-20000, 'boom');
           EXCEPTION
             WHEN NO_DATA_FOUND OR e_bad THEN NULL;
           END;
         EXCEPTION
           WHEN OTHERS THEN
             ROLLBACK;
             RAISE;
         END;",
    );
    assert_eq!(u.kind, PlsqlKind::Block);
    assert_eq!(u.block.declarations.len(), 9);
    assert_eq!(u.block.body.len(), 14);
    assert_eq!(u.block.handlers.len(), 1);
    assert!(matches!(&u.block.body[0], PlStatement::ForRange { label: Some(l), .. } if l.value == "outer"));
    assert!(matches!(&u.block.body[4], PlStatement::Case { operand: Some(_), .. }));
    assert!(matches!(&u.block.body[5], PlStatement::Sql { into, .. } if into.len() == 1));
    assert!(matches!(&u.block.body[6], PlStatement::Sql { into, .. } if into.len() == 1));
    assert!(
        matches!(&u.block.body[12], PlStatement::ExecuteImmediate { into, using, .. } if into.len() == 1 && using.len() == 1)
    );
    assert!(matches!(&u.block.declarations[8], PlDeclaration::Subprogram(s) if !s.is_function && s.params.len() == 1));
}

#[test]
fn stored_functions_and_procedures() {
    let f = unit(
        "CREATE OR REPLACE FUNCTION add_one(p_x IN NUMBER, p_y NUMBER DEFAULT 1) RETURN NUMBER DETERMINISTIC IS
           v NUMBER;
         BEGIN
           v := p_x + p_y;
           RETURN v;
         END add_one;",
    );
    assert_eq!(f.kind, PlsqlKind::Function);
    assert_eq!(f.name.as_ref().unwrap().0[0].value, "add_one");
    assert_eq!(f.params.len(), 2);
    assert_eq!(f.returns.as_deref(), Some("NUMBER"));
    let p = unit(
        "CREATE PROCEDURE reset(p_id VARCHAR2) AUTHID CURRENT_USER AS BEGIN UPDATE t SET a = 0 WHERE id = p_id; END;",
    );
    assert_eq!(p.kind, PlsqlKind::Procedure);
    assert_eq!(p.block.body.len(), 1);
    assert!(f.text.starts_with("CREATE OR REPLACE FUNCTION add_one"), "{}", f.text);
}

#[test]
fn triggers() {
    let t = unit(
        "CREATE OR REPLACE TRIGGER trg_orders
           BEFORE INSERT OR UPDATE OF total, status ON orders
           REFERENCING NEW AS n OLD AS o
           FOR EACH ROW
           WHEN (n.total > 0)
         BEGIN
           :NEW.updated := SYSDATE;
           IF :OLD.status <> :NEW.status THEN
             INSERT INTO audit_log (id) VALUES (:OLD.id);
           END IF;
         END;",
    );
    let tr = t.trigger.as_ref().unwrap();
    assert_eq!(tr.timing, "BEFORE");
    assert_eq!(tr.events, ["INSERT", "UPDATE OF total, status"]);
    assert!(tr.for_each_row && tr.when.is_some());
    let c = unit(
        "CREATE OR REPLACE TRIGGER trg_cdc
           FOR DELETE ON orders COMPOUND TRIGGER
           TYPE t_ids IS TABLE OF NUMBER;
           v_ids t_ids := t_ids();
           BEFORE STATEMENT IS BEGIN v_ids.DELETE; END BEFORE STATEMENT;
           AFTER EACH ROW IS BEGIN v_ids.EXTEND; v_ids(v_ids.LAST) := :OLD.id; END AFTER EACH ROW;
           AFTER STATEMENT IS BEGIN NULL; EXCEPTION WHEN OTHERS THEN NULL; END AFTER STATEMENT;
         END trg_cdc;",
    );
    let tr = c.trigger.as_ref().unwrap();
    assert_eq!(tr.timing, "COMPOUND");
    assert_eq!(
        tr.sections.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["BEFORE STATEMENT", "AFTER EACH ROW", "AFTER STATEMENT"]
    );
    assert_eq!(c.block.declarations.len(), 2);
}

#[test]
fn plsql_errors_are_reported_with_a_location() {
    let e = parse_sql(&OracleDialect, "BEGIN\n  IF x THEN NULL;\nEND;").unwrap_err();
    assert_eq!(e.location.unwrap().line, 3);
    assert!(check_syntax(Oracle, "BEGIN x := ; END;").is_err());
    assert!(check_syntax(Postgres, "BEGIN NULL; END;").is_err());
    assert!(check_syntax(Sqlite, "DECLARE x NUMBER; BEGIN NULL; END;").is_err());
}

#[test]
fn sql_constructs_found_in_plsql_scripts() {
    ok(Oracle, "SELECT LISTAGG(name, ', ') WITHIN GROUP (ORDER BY name) FROM t");
    ok(
        Oracle,
        "SELECT RTRIM(XMLAGG(XMLELEMENT(e, name, ',').EXTRACT('//text()') ORDER BY name).GetClobVal(), ',') FROM t",
    );
    ok(Oracle, "SELECT id FROM t WHERE x = 1 FOR UPDATE OF id NOWAIT");
    ok(Oracle, "SELECT * FROM t FOR UPDATE SKIP LOCKED");
    ok(Oracle, "UPDATE t SET d = (d + INTERVAL '1' DAY) - INTERVAL '2' YEAR TO MONTH, e = SYSDATE + INTERVAL '7' DAY");
    ok(Oracle, "INSERT INTO t ei (a, b) SELECT 1, 2 FROM dual");
    ok(Oracle, "BEGIN FOR i IN 36..40 LOOP NULL; END LOOP; FOR j IN v_low..v_high LOOP NULL; END LOOP; END;");
    ok(Oracle, "DECLARE v VARCHAR2(10) NULL; BEGIN NULL; END;");
    ok(Oracle, "BEGIN my_proc(p_a => 1, p_b => 'x'); END;");
    assert_eq!(transpile(Oracle, Oracle, "SELECT a.b(1).c FROM t").unwrap(), "SELECT a.b(1).c FROM t;");
    assert_eq!(
        transpile(Oracle, Oracle, "SELECT LISTAGG(n, ',') WITHIN GROUP (ORDER BY n DESC), XMLAGG(x ORDER BY y) FROM t")
            .unwrap(),
        "SELECT LISTAGG(n, ',') WITHIN GROUP (ORDER BY n DESC), XMLAGG(x ORDER BY y) FROM t;"
    );
}

#[test]
fn sqlplus_substitution_variables() {
    ok(Oracle, "DECLARE n INT := &1; m INT := &&2; BEGIN NULL; END;");
    ok(Oracle, "SELECT * FROM &schema..t WHERE x = '&val'");
    ok(Oracle, "UPDATE &SourceSchema.t SET a = &1");
}

#[test]
fn plsql_units_in_a_schema_are_counted_and_their_dynamic_sql_applied() {
    let mut s = Schema::new(Oracle);
    let applied = s
        .apply_sql(
            "CREATE TABLE t (a NUMBER);
             CREATE OR REPLACE FUNCTION f RETURN NUMBER IS BEGIN RETURN 1; END;
             /
             CREATE OR REPLACE TRIGGER trg BEFORE INSERT ON t FOR EACH ROW BEGIN :NEW.a := 1; END;
             /
             BEGIN
               EXECUTE IMMEDIATE 'ALTER TABLE t ADD b NUMBER';
             END;
             /",
        )
        .unwrap();
    assert_eq!(applied.skipped, vec![]);
    assert_eq!(applied.errors, vec![]);
    assert_eq!(applied.plsql.len(), 3);
    assert_eq!(s.table("t").unwrap().columns.len(), 2);
}
