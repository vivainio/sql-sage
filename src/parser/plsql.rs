//! PL/SQL: anonymous blocks, stored functions and procedures, and triggers.
//!
//! The grammar covers what migration and installation scripts contain: declarations,
//! control flow, cursors, `EXECUTE IMMEDIATE`, exception handlers and embedded SQL. Types are
//! kept as text; packages, object types and compound triggers are not parsed.

use super::*;

impl<'a> Parser<'a> {
    /// Does a PL/SQL unit start here?
    pub(super) fn is_plsql_start(&self) -> bool {
        if self.peek_keyword("DECLARE") || self.peek_keyword("BEGIN") {
            return true;
        }
        if *self.peek_token() == Token::Lt && *self.peek_nth(1) == Token::Lt {
            return true;
        }
        if !self.peek_keyword("CREATE") {
            return false;
        }
        let mut i = 1;
        while ["OR", "REPLACE", "EDITIONABLE", "NONEDITIONABLE"].iter().any(|k| self.peek_nth_keyword(i, k)) {
            i += 1;
        }
        ["FUNCTION", "PROCEDURE", "TRIGGER"].iter().any(|k| self.peek_nth_keyword(i, k))
    }

    pub(super) fn parse_plsql_unit(&mut self) -> Result<Statement> {
        let start = self.index;
        let outer = std::mem::replace(&mut self.in_plsql, true);
        let unit = self.parse_plsql_unit_inner();
        self.in_plsql = outer;
        let mut unit = unit?;
        unit.text = self.tokens_text(start, self.index);
        Ok(Statement::Plsql(Box::new(unit)))
    }

    fn parse_plsql_unit_inner(&mut self) -> Result<PlsqlUnit> {
        if self.parse_keyword("CREATE") {
            self.parse_keywords(&["OR", "REPLACE"]);
            let _ = self.parse_one_of_keywords(&["EDITIONABLE", "NONEDITIONABLE"]);
            if self.parse_keyword("FUNCTION") {
                return self.parse_stored_subprogram(PlsqlKind::Function);
            }
            if self.parse_keyword("PROCEDURE") {
                return self.parse_stored_subprogram(PlsqlKind::Procedure);
            }
            if self.parse_keyword("TRIGGER") {
                return self.parse_trigger();
            }
            return self.expected("FUNCTION, PROCEDURE or TRIGGER");
        }
        let label = self.parse_pl_label()?;
        self.parse_keyword("DECLARE");
        let mut block = self.parse_plsql_body()?;
        block.label = label;
        Ok(PlsqlUnit {
            kind: PlsqlKind::Block,
            name: None,
            params: vec![],
            returns: None,
            trigger: None,
            block,
            text: String::new(),
        })
    }

    fn parse_stored_subprogram(&mut self, kind: PlsqlKind) -> Result<PlsqlUnit> {
        let name = self.parse_object_name()?;
        let params = if *self.peek_token() == Token::LParen { self.parse_pl_params()? } else { vec![] };
        let returns = if kind == PlsqlKind::Function {
            self.expect_keyword("RETURN")?;
            Some(self.parse_pl_type()?)
        } else {
            None
        };
        self.skip_pl_characteristics()?;
        if self.parse_one_of_keywords(&["IS", "AS"]).is_none() {
            return self.expected("IS or AS");
        }
        let block = self.parse_plsql_body()?;
        Ok(PlsqlUnit { kind, name: Some(name), params, returns, trigger: None, block, text: String::new() })
    }

    fn skip_pl_characteristics(&mut self) -> Result<()> {
        loop {
            if self.parse_keyword("AUTHID") {
                self.parse_ident()?;
            } else if self
                .parse_one_of_keywords(&["DETERMINISTIC", "PIPELINED", "PARALLEL_ENABLE", "RESULT_CACHE"])
                .is_some()
            {
                // Accepted and ignored.
            } else {
                return Ok(());
            }
        }
    }

    fn parse_trigger(&mut self) -> Result<PlsqlUnit> {
        let name = self.parse_object_name()?;
        let compound = self.parse_keyword("FOR");
        let timing = if compound {
            "COMPOUND"
        } else if self.parse_keyword("BEFORE") {
            "BEFORE"
        } else if self.parse_keyword("AFTER") {
            "AFTER"
        } else if self.parse_keywords(&["INSTEAD", "OF"]) {
            "INSTEAD OF"
        } else {
            return self.expected("BEFORE, AFTER, INSTEAD OF or FOR");
        };
        let events = self.parse_trigger_events()?;
        self.expect_keyword("ON")?;
        let table = self.parse_object_name()?;
        if compound {
            return self.parse_compound_trigger(name, events, table);
        }
        if self.parse_keyword("REFERENCING") {
            while !["FOR", "WHEN", "DECLARE", "BEGIN", "ENABLE", "DISABLE"].iter().any(|k| self.peek_keyword(k)) {
                if *self.peek_token() == Token::Eof {
                    return self.expected("FOR EACH ROW or BEGIN");
                }
                self.index += 1;
            }
        }
        let for_each_row = self.parse_keywords(&["FOR", "EACH", "ROW"]);
        if self.parse_keyword("FOLLOWS") {
            self.parse_comma_separated(Self::parse_object_name)?;
        }
        let _ = self.parse_one_of_keywords(&["ENABLE", "DISABLE"]);
        let when = if self.parse_keyword("WHEN") {
            self.expect_token(&Token::LParen)?;
            let e = self.parse_expr()?;
            self.expect_token(&Token::RParen)?;
            Some(e)
        } else {
            None
        };
        self.parse_keyword("DECLARE");
        let block = self.parse_plsql_body()?;
        let trigger = PlTrigger { timing: timing.to_string(), events, table, for_each_row, when, sections: vec![] };
        Ok(PlsqlUnit {
            kind: PlsqlKind::Trigger,
            name: Some(name),
            params: vec![],
            returns: None,
            trigger: Some(trigger),
            block,
            text: String::new(),
        })
    }

    /// `INSERT OR UPDATE OF a, b OR DELETE`
    fn parse_trigger_events(&mut self) -> Result<Vec<String>> {
        let mut events = vec![];
        loop {
            if self.parse_keyword("INSERT") {
                events.push("INSERT".to_string());
            } else if self.parse_keyword("DELETE") {
                events.push("DELETE".to_string());
            } else if self.parse_keyword("UPDATE") {
                if self.parse_keyword("OF") {
                    let cols = self.parse_comma_separated(Self::parse_ident)?;
                    let cols: Vec<_> = cols.iter().map(|c| c.value.clone()).collect();
                    events.push(format!("UPDATE OF {}", cols.join(", ")));
                } else {
                    events.push("UPDATE".to_string());
                }
            } else {
                return self.expected("INSERT, UPDATE or DELETE");
            }
            if !self.parse_keyword("OR") {
                return Ok(events);
            }
        }
    }

    /// `BEFORE STATEMENT`, `AFTER EACH ROW`, `INSTEAD OF EACH ROW`
    fn parse_timing_point(&mut self) -> Result<String> {
        let when = if self.parse_keyword("BEFORE") {
            "BEFORE"
        } else if self.parse_keyword("AFTER") {
            "AFTER"
        } else if self.parse_keywords(&["INSTEAD", "OF"]) {
            "INSTEAD OF"
        } else {
            return self.expected("BEFORE, AFTER or INSTEAD OF");
        };
        if self.parse_keyword("STATEMENT") {
            Ok(format!("{when} STATEMENT"))
        } else if self.parse_keywords(&["EACH", "ROW"]) {
            Ok(format!("{when} EACH ROW"))
        } else {
            self.expected("STATEMENT or EACH ROW")
        }
    }

    /// After `FOR events ON table`: `COMPOUND TRIGGER declarations sections END [name]`.
    fn parse_compound_trigger(
        &mut self,
        name: ObjectName,
        events: Vec<String>,
        table: ObjectName,
    ) -> Result<PlsqlUnit> {
        self.expect_keyword("COMPOUND")?;
        self.expect_keyword("TRIGGER")?;
        let mut block = PlsqlBlock::default();
        let at_timing_point =
            |p: &Self| p.peek_keyword("BEFORE") || p.peek_keyword("AFTER") || p.peek_keyword("INSTEAD");
        while !at_timing_point(self) {
            if *self.peek_token() == Token::Eof {
                return self.expected("a timing point");
            }
            block.declarations.push(self.parse_pl_declaration()?);
        }
        let mut sections = vec![];
        while at_timing_point(self) {
            let point = self.parse_timing_point()?;
            self.expect_keyword("IS")?;
            self.expect_keyword("BEGIN")?;
            let mut section = PlsqlBlock::default();
            self.parse_pl_body_tail(&mut section)?;
            self.expect_keyword("END")?;
            self.parse_timing_point()?;
            self.expect_token(&Token::Semicolon)?;
            sections.push((point, section));
        }
        self.expect_keyword("END")?;
        self.skip_pl_label_name();
        let trigger = PlTrigger { timing: "COMPOUND".into(), events, table, for_each_row: false, when: None, sections };
        Ok(PlsqlUnit {
            kind: PlsqlKind::Trigger,
            name: Some(name),
            params: vec![],
            returns: None,
            trigger: Some(trigger),
            block,
            text: String::new(),
        })
    }

    /// Statements and exception handlers, up to `END`.
    fn parse_pl_body_tail(&mut self, block: &mut PlsqlBlock) -> Result<()> {
        block.body = self.parse_pl_statements()?;
        if self.parse_keyword("EXCEPTION") {
            while self.parse_keyword("WHEN") {
                let mut exceptions = vec![self.parse_object_name()?];
                while self.parse_keyword("OR") {
                    exceptions.push(self.parse_object_name()?);
                }
                self.expect_keyword("THEN")?;
                let body = self.parse_pl_statements()?;
                block.handlers.push(PlHandler { exceptions, body });
            }
        }
        Ok(())
    }

    /// `[declarations] BEGIN statements [EXCEPTION handlers] END [name]`
    fn parse_plsql_body(&mut self) -> Result<PlsqlBlock> {
        let mut block = PlsqlBlock::default();
        while !self.peek_keyword("BEGIN") {
            if *self.peek_token() == Token::Eof {
                return self.expected("BEGIN");
            }
            block.declarations.push(self.parse_pl_declaration()?);
        }
        self.expect_keyword("BEGIN")?;
        self.parse_pl_body_tail(&mut block)?;
        self.expect_keyword("END")?;
        self.skip_pl_label_name();
        Ok(block)
    }

    /// The optional name after `END`, `END LOOP`, ...
    fn skip_pl_label_name(&mut self) {
        if matches!(self.peek_token(), Token::Word(_)) {
            self.index += 1;
        }
    }

    fn parse_pl_label(&mut self) -> Result<Option<Ident>> {
        if *self.peek_token() == Token::Lt && *self.peek_nth(1) == Token::Lt {
            self.index += 2;
            let name = self.parse_ident()?;
            self.expect_token(&Token::Gt)?;
            self.expect_token(&Token::Gt)?;
            return Ok(Some(name));
        }
        Ok(None)
    }

    fn parse_pl_params(&mut self) -> Result<Vec<PlParam>> {
        self.expect_token(&Token::LParen)?;
        let params = self.parse_comma_separated(Self::parse_pl_param)?;
        self.expect_token(&Token::RParen)?;
        Ok(params)
    }

    fn parse_pl_param(&mut self) -> Result<PlParam> {
        let name = self.parse_ident()?;
        let mode = if self.parse_keywords(&["IN", "OUT"]) {
            Some("IN OUT".to_string())
        } else if self.parse_keyword("IN") {
            Some("IN".to_string())
        } else if self.parse_keyword("OUT") {
            Some("OUT".to_string())
        } else {
            None
        };
        self.parse_keyword("NOCOPY");
        let data_type = self.parse_pl_type()?;
        let default = self.parse_pl_default()?;
        Ok(PlParam { name, mode, data_type, default })
    }

    fn parse_pl_default(&mut self) -> Result<Option<Expr>> {
        if self.consume_token(&Token::Colon) {
            self.expect_token(&Token::Eq)?;
            Ok(Some(self.parse_expr()?))
        } else if self.parse_keyword("DEFAULT") {
            Ok(Some(self.parse_expr()?))
        } else {
            Ok(None)
        }
    }

    /// A type as written: `NUMBER`, `VARCHAR2(10 CHAR)`, `t.c%TYPE`, `TIMESTAMP WITH TIME ZONE`.
    fn parse_pl_type(&mut self) -> Result<String> {
        const WORDS: [&str; 17] = [
            "WITH",
            "WITHOUT",
            "LOCAL",
            "TIME",
            "ZONE",
            "PRECISION",
            "VARYING",
            "RAW",
            "CHAR",
            "BYTE",
            "YEAR",
            "MONTH",
            "DAY",
            "HOUR",
            "MINUTE",
            "SECOND",
            "TO",
        ];
        let start = self.index;
        if !matches!(self.peek_token(), Token::Word(_)) {
            return self.expected("a data type");
        }
        self.index += 1;
        while self.consume_token(&Token::Period) {
            self.parse_ident()?;
        }
        if self.consume_token(&Token::Percent) && self.parse_one_of_keywords(&["TYPE", "ROWTYPE"]).is_none() {
            return self.expected("TYPE or ROWTYPE");
        }
        loop {
            match self.peek_token() {
                Token::LParen => self.skip_balanced_parens()?,
                Token::Word(w) if w.quote.is_none() && WORDS.iter().any(|k| w.is_keyword(k)) => self.index += 1,
                _ => break,
            }
        }
        Ok(self.tokens_text(start, self.index))
    }

    fn parse_pl_declaration(&mut self) -> Result<PlDeclaration> {
        if self.parse_keyword("PRAGMA") {
            let start = self.index;
            self.skip_to_semicolon()?;
            let text = self.tokens_text(start, self.index);
            self.expect_token(&Token::Semicolon)?;
            return Ok(PlDeclaration::Pragma(text));
        }
        if self.parse_keyword("CURSOR") {
            let name = self.parse_ident()?;
            let params = if *self.peek_token() == Token::LParen { self.parse_pl_params()? } else { vec![] };
            self.expect_keyword("IS")?;
            let query = Box::new(self.parse_query()?);
            self.expect_token(&Token::Semicolon)?;
            return Ok(PlDeclaration::Cursor { name, params, query });
        }
        if self.parse_keyword("TYPE") || self.parse_keyword("SUBTYPE") {
            let name = self.parse_ident()?;
            let start = self.index;
            self.skip_to_semicolon()?;
            let definition = self.tokens_text(start, self.index);
            self.expect_token(&Token::Semicolon)?;
            return Ok(PlDeclaration::Type { name, definition });
        }
        if self.peek_keyword("FUNCTION") || self.peek_keyword("PROCEDURE") {
            return self.parse_pl_subprogram();
        }
        let name = self.parse_ident()?;
        if self.parse_keyword("EXCEPTION") {
            self.expect_token(&Token::Semicolon)?;
            return Ok(PlDeclaration::Exception(name));
        }
        let constant = self.parse_keyword("CONSTANT");
        let data_type = self.parse_pl_type()?;
        let not_null = self.parse_keywords(&["NOT", "NULL"]);
        if !not_null {
            self.parse_keyword("NULL");
        }
        let default = self.parse_pl_default()?;
        self.expect_token(&Token::Semicolon)?;
        Ok(PlDeclaration::Variable { name, constant, data_type, not_null, default })
    }

    /// Consumes tokens up to (not including) the next `;` outside parentheses.
    fn skip_to_semicolon(&mut self) -> Result<()> {
        let mut depth = 0usize;
        loop {
            match self.peek_token() {
                Token::Eof => return self.expected("';'"),
                Token::Semicolon if depth == 0 => return Ok(()),
                Token::LParen => depth += 1,
                Token::RParen => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.index += 1;
        }
    }

    fn parse_pl_subprogram(&mut self) -> Result<PlDeclaration> {
        let is_function = self.peek_keyword("FUNCTION");
        self.index += 1;
        let name = self.parse_ident()?;
        let params = if *self.peek_token() == Token::LParen { self.parse_pl_params()? } else { vec![] };
        let returns = if is_function {
            self.expect_keyword("RETURN")?;
            Some(self.parse_pl_type()?)
        } else {
            None
        };
        self.skip_pl_characteristics()?;
        let block = if self.peek_token() == &Token::Semicolon {
            None
        } else {
            if self.parse_one_of_keywords(&["IS", "AS"]).is_none() {
                return self.expected("IS or AS");
            }
            Some(self.parse_plsql_body()?)
        };
        self.expect_token(&Token::Semicolon)?;
        Ok(PlDeclaration::Subprogram(Box::new(PlSubprogram { is_function, name, params, returns, block })))
    }

    /// Statements up to `END`, `EXCEPTION`, `ELSE`, `ELSIF` or `WHEN`, each ended by `;`.
    fn parse_pl_statements(&mut self) -> Result<Vec<PlStatement>> {
        let mut out = vec![];
        loop {
            while self.consume_token(&Token::Semicolon) {}
            let end = ["END", "EXCEPTION", "ELSE", "ELSIF", "WHEN"].iter().any(|k| self.peek_keyword(k))
                || *self.peek_token() == Token::Eof;
            if end {
                return Ok(out);
            }
            out.push(self.parse_pl_statement()?);
            self.expect_token(&Token::Semicolon)?;
        }
    }

    fn parse_pl_statement(&mut self) -> Result<PlStatement> {
        let label = self.parse_pl_label()?;
        if self.parse_keyword("NULL") {
            return Ok(PlStatement::Null);
        }
        if self.parse_keyword("IF") {
            return self.parse_pl_if();
        }
        if self.parse_keyword("CASE") {
            return self.parse_pl_case();
        }
        if self.parse_keyword("LOOP") {
            let body = self.parse_pl_statements()?;
            self.expect_keyword("END")?;
            self.expect_keyword("LOOP")?;
            self.skip_pl_label_name();
            return Ok(PlStatement::Loop { label, body });
        }
        if self.parse_keyword("WHILE") {
            let condition = self.parse_expr()?;
            self.expect_keyword("LOOP")?;
            let body = self.parse_pl_statements()?;
            self.expect_keyword("END")?;
            self.expect_keyword("LOOP")?;
            self.skip_pl_label_name();
            return Ok(PlStatement::While { label, condition, body });
        }
        if self.parse_keyword("FOR") {
            return self.parse_pl_for(label);
        }
        if self.parse_keyword("FORALL") {
            let var = self.parse_ident()?;
            self.expect_keyword("IN")?;
            let low = self.parse_expr()?;
            self.expect_token(&Token::Period)?;
            self.expect_token(&Token::Period)?;
            let high = self.parse_expr()?;
            self.parse_keywords(&["SAVE", "EXCEPTIONS"]);
            let statement = Box::new(self.parse_pl_statement()?);
            return Ok(PlStatement::Forall { var, low, high, statement });
        }
        if self.parse_keyword("RETURN") {
            let value = if *self.peek_token() == Token::Semicolon { None } else { Some(self.parse_expr()?) };
            return Ok(PlStatement::Return(value));
        }
        if self.peek_keyword("EXIT") || self.peek_keyword("CONTINUE") {
            let is_exit = self.peek_keyword("EXIT");
            self.index += 1;
            let label = match self.peek_token() {
                Token::Word(w) if !w.is_keyword("WHEN") => Some(self.parse_ident()?),
                _ => None,
            };
            let when = if self.parse_keyword("WHEN") { Some(self.parse_expr()?) } else { None };
            return Ok(if is_exit { PlStatement::Exit { label, when } } else { PlStatement::Continue { label, when } });
        }
        if self.parse_keyword("GOTO") {
            return Ok(PlStatement::Goto(self.parse_ident()?));
        }
        if self.parse_keyword("RAISE") {
            let name = if *self.peek_token() == Token::Semicolon { None } else { Some(self.parse_object_name()?) };
            return Ok(PlStatement::Raise(name));
        }
        if self.peek_keyword("EXECUTE") && self.peek_nth_keyword(1, "IMMEDIATE") {
            self.index += 2;
            return self.parse_pl_execute_immediate();
        }
        if self.parse_keyword("OPEN") {
            let cursor = self.parse_expr()?;
            let mut query = None;
            if self.parse_keyword("FOR") {
                if self.peek_query_start() || *self.peek_token() == Token::LParen {
                    query = Some(Box::new(self.parse_query()?));
                } else {
                    self.parse_expr()?;
                    if self.parse_keyword("USING") {
                        self.parse_comma_separated(Self::parse_expr)?;
                    }
                }
            }
            return Ok(PlStatement::Open { cursor, query });
        }
        if self.parse_keyword("FETCH") {
            let cursor = self.parse_expr()?;
            self.parse_keywords(&["BULK", "COLLECT"]);
            self.expect_keyword("INTO")?;
            let into = self.parse_comma_separated(Self::parse_expr)?;
            if self.parse_keyword("LIMIT") {
                self.parse_expr()?;
            }
            return Ok(PlStatement::Fetch { cursor, into });
        }
        if self.parse_keyword("CLOSE") {
            return Ok(PlStatement::Close(self.parse_expr()?));
        }
        if self.peek_keyword("BEGIN") || self.peek_keyword("DECLARE") {
            self.parse_keyword("DECLARE");
            let mut block = self.parse_plsql_body()?;
            block.label = label;
            return Ok(PlStatement::Block(Box::new(block)));
        }
        let sql_start = [
            "SELECT",
            "WITH",
            "INSERT",
            "UPDATE",
            "DELETE",
            "MERGE",
            "COMMIT",
            "ROLLBACK",
            "SAVEPOINT",
            "LOCK",
            "TRUNCATE",
        ]
        .iter()
        .any(|k| self.peek_keyword(k));
        if sql_start {
            self.pending_into.clear();
            let statement = Box::new(self.parse_statement()?);
            let mut into = std::mem::take(&mut self.pending_into);
            // RETURNING ... INTO
            self.parse_keywords(&["BULK", "COLLECT"]);
            if self.parse_keyword("INTO") {
                into.extend(self.parse_comma_separated(Self::parse_expr)?);
            }
            return Ok(PlStatement::Sql { statement, into });
        }
        let target = self.parse_expr()?;
        if self.consume_token(&Token::Colon) {
            self.expect_token(&Token::Eq)?;
            let value = self.parse_expr()?;
            return Ok(PlStatement::Assign { target, value });
        }
        Ok(PlStatement::Call(target))
    }

    fn parse_pl_execute_immediate(&mut self) -> Result<PlStatement> {
        let sql = self.parse_expr()?;
        let (mut into, mut using) = (vec![], vec![]);
        loop {
            if self.parse_keywords(&["BULK", "COLLECT", "INTO"]) || self.parse_keyword("INTO") {
                into.extend(self.parse_comma_separated(Self::parse_expr)?);
            } else if self.parse_keyword("USING") {
                using.extend(self.parse_comma_separated(|p| {
                    let _ = p.parse_keywords(&["IN", "OUT"]) || p.parse_keyword("IN") || p.parse_keyword("OUT");
                    p.parse_expr()
                })?);
            } else if self.parse_keyword("RETURNING") || self.parse_keyword("RETURN") {
                self.expect_keyword("INTO")?;
                into.extend(self.parse_comma_separated(Self::parse_expr)?);
            } else {
                return Ok(PlStatement::ExecuteImmediate { sql, into, using });
            }
        }
    }

    fn parse_pl_if(&mut self) -> Result<PlStatement> {
        let mut branches = vec![];
        let condition = self.parse_expr()?;
        self.expect_keyword("THEN")?;
        branches.push((condition, self.parse_pl_statements()?));
        let mut else_body = vec![];
        loop {
            if self.parse_keyword("ELSIF") {
                let condition = self.parse_expr()?;
                self.expect_keyword("THEN")?;
                branches.push((condition, self.parse_pl_statements()?));
            } else if self.parse_keyword("ELSE") {
                else_body = self.parse_pl_statements()?;
            } else {
                break;
            }
        }
        self.expect_keyword("END")?;
        self.expect_keyword("IF")?;
        Ok(PlStatement::If { branches, else_body })
    }

    fn parse_pl_case(&mut self) -> Result<PlStatement> {
        let operand = if self.peek_keyword("WHEN") { None } else { Some(self.parse_expr()?) };
        let mut whens = vec![];
        while self.parse_keyword("WHEN") {
            let condition = self.parse_expr()?;
            self.expect_keyword("THEN")?;
            whens.push((condition, self.parse_pl_statements()?));
        }
        let else_body = if self.parse_keyword("ELSE") { Some(self.parse_pl_statements()?) } else { None };
        self.expect_keyword("END")?;
        self.expect_keyword("CASE")?;
        self.skip_pl_label_name();
        Ok(PlStatement::Case { operand, whens, else_body })
    }

    /// After `FOR`: `i IN [REVERSE] a..b LOOP`, `r IN (SELECT ...) LOOP` or `r IN cursor LOOP`.
    fn parse_pl_for(&mut self, label: Option<Ident>) -> Result<PlStatement> {
        let var = self.parse_ident()?;
        self.expect_keyword("IN")?;
        let reverse = self.parse_keyword("REVERSE");
        let is_query = *self.peek_token() == Token::LParen && {
            let n = self.peek_nth(1);
            matches!(n, Token::Word(w) if w.is_keyword("SELECT") || w.is_keyword("WITH"))
        };
        if is_query {
            self.expect_token(&Token::LParen)?;
            let query = Box::new(self.parse_query()?);
            self.expect_token(&Token::RParen)?;
            self.expect_keyword("LOOP")?;
            let body = self.parse_pl_statements()?;
            self.expect_keyword("END")?;
            self.expect_keyword("LOOP")?;
            self.skip_pl_label_name();
            return Ok(PlStatement::ForIn { label, var, source: PlCursorSource::Query(query), body });
        }
        let first = self.parse_expr()?;
        let (source_or_range, body);
        if self.consume_token(&Token::Period) {
            self.expect_token(&Token::Period)?;
            let high = self.parse_expr()?;
            self.expect_keyword("LOOP")?;
            body = self.parse_pl_statements()?;
            source_or_range = Ok((first, high));
        } else {
            self.expect_keyword("LOOP")?;
            body = self.parse_pl_statements()?;
            source_or_range = Err(first);
        }
        self.expect_keyword("END")?;
        self.expect_keyword("LOOP")?;
        self.skip_pl_label_name();
        Ok(match source_or_range {
            Ok((low, high)) => PlStatement::ForRange { label, var, reverse, low, high, body },
            Err(cursor) => PlStatement::ForIn { label, var, source: PlCursorSource::Cursor(cursor), body },
        })
    }
}
