use crate::ast::*;
use crate::dialect::Dialect;
use crate::error::{Location, ParseError, Result};
use crate::lexer::{Lexer, Token, TokenWithLocation, Word};

const RESERVED: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "HAVING",
    "ORDER",
    "BY",
    "LIMIT",
    "OFFSET",
    "FETCH",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "SET",
    "RETURNING",
    "VALUES",
    "WINDOW",
    "FOR",
    "INTO",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IS",
    "IN",
    "LIKE",
    "BETWEEN",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "CASE",
    "NULL",
    "WITH",
    "DO",
];

/// Result of [`Parser::parse_statements_lenient`].
#[derive(Debug)]
pub struct Lenient {
    pub statements: Vec<(Statement, Location)>,
    /// Statements that did not parse: where each starts, and why.
    pub skipped: Vec<(Location, ParseError)>,
}

pub struct Parser<'a> {
    dialect: &'a dyn Dialect,
    tokens: Vec<TokenWithLocation>,
    index: usize,
}

impl<'a> Parser<'a> {
    pub fn new(dialect: &'a dyn Dialect, sql: &str) -> Result<Self> {
        let tokens = Lexer::new(dialect, sql).tokenize()?;
        Ok(Self { dialect, tokens, index: 0 })
    }

    // ------------------------------------------------------------ plumbing

    fn peek_nth(&self, n: usize) -> &Token {
        &self.tokens[(self.index + n).min(self.tokens.len() - 1)].token
    }

    fn peek_token(&self) -> &Token {
        self.peek_nth(0)
    }

    fn next_token(&mut self) -> Token {
        let t = self.peek_token().clone();
        if t != Token::Eof {
            self.index += 1;
        }
        t
    }

    fn location(&self) -> Location {
        self.tokens[self.index.min(self.tokens.len() - 1)].location.clone()
    }

    fn error<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(ParseError::new(msg, Some(self.location())))
    }

    fn expected<T>(&self, what: &str) -> Result<T> {
        self.error(format!("expected {what}, found {}", self.peek_token()))
    }

    fn peek_nth_keyword(&self, n: usize, kw: &str) -> bool {
        matches!(self.peek_nth(n), Token::Word(w) if w.is_keyword(kw))
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        self.peek_nth_keyword(0, kw)
    }

    fn parse_keyword(&mut self, kw: &str) -> bool {
        if self.peek_keyword(kw) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn parse_keywords(&mut self, kws: &[&str]) -> bool {
        let start = self.index;
        for kw in kws {
            if !self.parse_keyword(kw) {
                self.index = start;
                return false;
            }
        }
        true
    }

    fn parse_one_of_keywords(&mut self, kws: &[&'static str]) -> Option<&'static str> {
        kws.iter().copied().find(|kw| self.parse_keyword(kw))
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<()> {
        if self.parse_keyword(kw) {
            Ok(())
        } else {
            self.expected(kw)
        }
    }

    fn consume_token(&mut self, t: &Token) -> bool {
        if self.peek_token() == t {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect_token(&mut self, t: &Token) -> Result<()> {
        if self.consume_token(t) {
            Ok(())
        } else {
            self.expected(&format!("'{t}'"))
        }
    }

    fn parse_comma_separated<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let mut items = vec![f(self)?];
        while self.consume_token(&Token::Comma) {
            items.push(f(self)?);
        }
        Ok(items)
    }

    fn is_reserved(&self, word: &str) -> bool {
        let u = word.to_ascii_uppercase();
        RESERVED.contains(&u.as_str()) || (u == "MINUS" && self.dialect.supports_minus_operator())
    }

    fn peek_query_start(&self) -> bool {
        self.peek_keyword("SELECT") || self.peek_keyword("WITH") || self.peek_keyword("VALUES")
    }

    // ---------------------------------------------------------- statements

    pub fn parse_statements(&mut self) -> Result<Vec<Statement>> {
        Ok(self.parse_statements_located()?.into_iter().map(|(s, _)| s).collect())
    }

    /// Like [`Self::parse_statements`], with the location where each statement starts.
    pub fn parse_statements_located(&mut self) -> Result<Vec<(Statement, Location)>> {
        let mut stmts = vec![];
        loop {
            while self.consume_token(&Token::Semicolon) {}
            if *self.peek_token() == Token::Eof {
                return Ok(stmts);
            }
            let at = self.location();
            stmts.push((self.parse_statement()?, at));
            if !matches!(self.peek_token(), Token::Semicolon | Token::Eof) {
                return self.expected("end of statement");
            }
        }
    }

    /// Parses as many statements as possible. A statement that does not parse is skipped (up to
    /// the next `;`, or for PL/SQL units up to the next `/` line) and reported in
    /// [`Lenient::skipped`] instead of stopping the parse. For dialects with SQL*Plus commands,
    /// those (`SET ...`, `WHENEVER ...`, a lone `/`) are skipped silently.
    pub fn parse_statements_lenient(&mut self) -> Lenient {
        let mut out = Lenient { statements: vec![], skipped: vec![] };
        loop {
            while self.consume_token(&Token::Semicolon) {}
            if *self.peek_token() == Token::Eof {
                return out;
            }
            if self.dialect.supports_sqlplus_commands() && self.skip_sqlplus_command() {
                continue;
            }
            let start = self.index;
            let at = self.location();
            let result = self.parse_statement().and_then(|s| {
                if matches!(self.peek_token(), Token::Semicolon | Token::Eof) {
                    Ok(s)
                } else {
                    self.expected("end of statement")
                }
            });
            match result {
                Ok(s) => out.statements.push((s, at)),
                Err(e) => {
                    out.skipped.push((at, e));
                    self.index = start;
                    self.recover();
                }
            }
        }
    }

    fn line_of(&self, index: usize) -> usize {
        self.tokens[index.min(self.tokens.len() - 1)].location.line
    }

    /// Is the token at `index` the first one on its line?
    fn starts_line(&self, index: usize) -> bool {
        index == 0 || self.line_of(index - 1) < self.line_of(index)
    }

    fn skip_sqlplus_command(&mut self) -> bool {
        let lone_slash = *self.peek_token() == Token::Slash && self.starts_line(self.index);
        let command = ["SET", "WHENEVER", "SPOOL", "PROMPT", "EXIT", "DEFINE", "UNDEFINE", "REM"]
            .iter()
            .any(|k| self.peek_keyword(k));
        if !(lone_slash || command) {
            return false;
        }
        let line = self.line_of(self.index);
        while *self.peek_token() != Token::Eof && self.line_of(self.index) == line {
            self.index += 1;
        }
        true
    }

    /// Moves past the statement that starts at the current token and failed to parse.
    fn recover(&mut self) {
        let plsql = self.peek_keyword("DECLARE")
            || self.peek_keyword("BEGIN")
            || (self.peek_keyword("CREATE") && {
                let mut i = 1;
                while ["OR", "REPLACE", "EDITIONABLE", "NONEDITIONABLE"].iter().any(|k| self.peek_nth_keyword(i, k)) {
                    i += 1;
                }
                ["FUNCTION", "PROCEDURE", "TRIGGER", "PACKAGE", "TYPE"].iter().any(|k| self.peek_nth_keyword(i, k))
            });
        self.index += 1;
        loop {
            match self.peek_token() {
                Token::Eof => return,
                Token::Slash if plsql && self.starts_line(self.index) => {
                    self.index += 1;
                    return;
                }
                Token::Semicolon if !plsql => {
                    self.index += 1;
                    return;
                }
                _ => self.index += 1,
            }
        }
    }

    pub fn parse_statement(&mut self) -> Result<Statement> {
        if self.peek_query_start() || *self.peek_token() == Token::LParen {
            return Ok(Statement::Query(Box::new(self.parse_query()?)));
        }
        if self.peek_keyword("INSERT") {
            self.parse_insert()
        } else if self.peek_keyword("UPDATE") {
            self.parse_update()
        } else if self.peek_keyword("DELETE") {
            self.parse_delete()
        } else if self.peek_keyword("CREATE") {
            self.parse_create()
        } else if self.peek_keyword("ALTER") {
            self.parse_alter_table()
        } else if self.peek_keyword("DROP") {
            self.parse_drop()
        } else {
            self.expected("a statement (SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER or DROP)")
        }
    }

    fn parse_returning(&mut self) -> Result<Vec<SelectItem>> {
        if self.dialect.supports_returning() && self.parse_keyword("RETURNING") {
            self.parse_comma_separated(Self::parse_select_item)
        } else {
            Ok(vec![])
        }
    }

    fn parse_insert(&mut self) -> Result<Statement> {
        self.expect_keyword("INSERT")?;
        let or = if self.dialect.supports_insert_or() && self.parse_keyword("OR") {
            match self.parse_one_of_keywords(&["ROLLBACK", "ABORT", "REPLACE", "FAIL", "IGNORE"]) {
                Some(a) => Some(a.to_string()),
                None => return self.expected("ROLLBACK, ABORT, REPLACE, FAIL or IGNORE"),
            }
        } else {
            None
        };
        self.expect_keyword("INTO")?;
        let table = self.parse_object_name()?;
        let mut columns = vec![];
        if *self.peek_token() == Token::LParen
            && !(self.peek_nth_keyword(1, "SELECT") || self.peek_nth_keyword(1, "WITH"))
        {
            self.next_token();
            columns = self.parse_comma_separated(Self::parse_ident)?;
            self.expect_token(&Token::RParen)?;
        }
        let source = if self.parse_keywords(&["DEFAULT", "VALUES"]) {
            InsertSource::DefaultValues
        } else {
            InsertSource::Query(Box::new(self.parse_query()?))
        };
        let on_conflict = if self.dialect.supports_on_conflict() && self.parse_keywords(&["ON", "CONFLICT"]) {
            let mut target = vec![];
            if self.consume_token(&Token::LParen) {
                target = self.parse_comma_separated(Self::parse_ident)?;
                self.expect_token(&Token::RParen)?;
            }
            self.expect_keyword("DO")?;
            let action = if self.parse_keyword("NOTHING") {
                OnConflictAction::DoNothing
            } else {
                self.expect_keyword("UPDATE")?;
                self.expect_keyword("SET")?;
                let assignments = self.parse_comma_separated(Self::parse_assignment)?;
                let selection = if self.parse_keyword("WHERE") { Some(self.parse_expr()?) } else { None };
                OnConflictAction::DoUpdate { assignments, selection }
            };
            Some(OnConflict { target, action })
        } else {
            None
        };
        let returning = self.parse_returning()?;
        Ok(Statement::Insert(Insert { or, table, columns, source, on_conflict, returning }))
    }

    fn parse_assignment(&mut self) -> Result<Assignment> {
        let column = self.parse_object_name()?;
        self.expect_token(&Token::Eq)?;
        Ok(Assignment { column, value: self.parse_expr()? })
    }

    fn parse_update(&mut self) -> Result<Statement> {
        self.expect_keyword("UPDATE")?;
        let table = self.parse_table_factor()?;
        self.expect_keyword("SET")?;
        let assignments = self.parse_comma_separated(Self::parse_assignment)?;
        let from = if self.dialect.supports_update_from() && self.parse_keyword("FROM") {
            self.parse_comma_separated(Self::parse_table_and_joins)?
        } else {
            vec![]
        };
        let selection = if self.parse_keyword("WHERE") { Some(self.parse_expr()?) } else { None };
        let returning = self.parse_returning()?;
        Ok(Statement::Update(Update { table, assignments, from, selection, returning }))
    }

    fn parse_delete(&mut self) -> Result<Statement> {
        self.expect_keyword("DELETE")?;
        self.parse_keyword("FROM"); // optional in Oracle; accepted everywhere
        let table = self.parse_table_factor()?;
        let selection = if self.parse_keyword("WHERE") { Some(self.parse_expr()?) } else { None };
        let returning = self.parse_returning()?;
        Ok(Statement::Delete(Delete { table, selection, returning }))
    }

    fn parse_drop_behavior(&mut self) -> Option<DropBehavior> {
        match self.parse_one_of_keywords(&["CASCADE", "RESTRICT"]) {
            Some("CASCADE") => Some(DropBehavior::Cascade),
            Some(_) => Some(DropBehavior::Restrict),
            None => None,
        }
    }

    fn peek_table_constraint(&self) -> bool {
        ["CONSTRAINT", "PRIMARY", "UNIQUE", "FOREIGN", "CHECK"].iter().any(|k| self.peek_keyword(k))
    }

    fn parse_alter_table(&mut self) -> Result<Statement> {
        self.expect_keyword("ALTER")?;
        self.expect_keyword("TABLE")?;
        let if_exists = self.parse_keywords(&["IF", "EXISTS"]);
        let name = self.parse_object_name()?;
        let mut actions = vec![];
        loop {
            self.parse_alter_action(&mut actions)?;
            if !self.consume_token(&Token::Comma) {
                break;
            }
        }
        Ok(Statement::AlterTable(AlterTable { if_exists, name, actions }))
    }

    fn parse_alter_action(&mut self, out: &mut Vec<AlterAction>) -> Result<()> {
        if self.parse_keyword("ADD") {
            if self.peek_table_constraint() {
                out.push(AlterAction::AddConstraint(self.parse_table_constraint()?));
                return Ok(());
            }
            let _ = self.parse_keyword("COLUMN");
            let if_not_exists = self.parse_keywords(&["IF", "NOT", "EXISTS"]);
            // Oracle: ADD (a int, b int)
            if self.consume_token(&Token::LParen) {
                loop {
                    if self.peek_table_constraint() {
                        out.push(AlterAction::AddConstraint(self.parse_table_constraint()?));
                    } else {
                        out.push(AlterAction::AddColumn { if_not_exists, column: self.parse_column_def()? });
                    }
                    if !self.consume_token(&Token::Comma) {
                        break;
                    }
                }
                return self.expect_token(&Token::RParen);
            }
            out.push(AlterAction::AddColumn { if_not_exists, column: self.parse_column_def()? });
        } else if self.parse_keyword("DROP") {
            if self.parse_keyword("CONSTRAINT") {
                let if_exists = self.parse_keywords(&["IF", "EXISTS"]);
                let name = self.parse_ident()?;
                let behavior = self.parse_drop_behavior();
                out.push(AlterAction::DropConstraint { if_exists, name, behavior });
                return Ok(());
            }
            let _ = self.parse_keyword("COLUMN");
            let if_exists = self.parse_keywords(&["IF", "EXISTS"]);
            // Oracle: DROP (a, b)
            let names = if *self.peek_token() == Token::LParen {
                self.parse_paren_idents()?
            } else {
                vec![self.parse_ident()?]
            };
            let behavior = self.parse_drop_behavior();
            for name in names {
                out.push(AlterAction::DropColumn { if_exists, name, behavior });
            }
        } else if self.parse_keyword("RENAME") {
            if self.parse_keyword("TO") {
                out.push(AlterAction::RenameTable(self.parse_ident()?));
            } else {
                let _ = self.parse_keyword("COLUMN");
                let old = self.parse_ident()?;
                self.expect_keyword("TO")?;
                out.push(AlterAction::RenameColumn { old, new: self.parse_ident()? });
            }
        } else if self.parse_keyword("ALTER") {
            let _ = self.parse_keyword("COLUMN");
            let name = self.parse_ident()?;
            if self.parse_keywords(&["SET", "NOT", "NULL"]) {
                out.push(AlterAction::SetNotNull(name));
            } else if self.parse_keywords(&["DROP", "NOT", "NULL"]) {
                out.push(AlterAction::DropNotNull(name));
            } else if self.parse_keywords(&["SET", "DEFAULT"]) {
                out.push(AlterAction::SetDefault { name, default: self.parse_expr()? });
            } else if self.parse_keywords(&["DROP", "DEFAULT"]) {
                out.push(AlterAction::DropDefault(name));
            } else {
                let _ = self.parse_keywords(&["SET", "DATA"]);
                self.expect_keyword("TYPE")?;
                let data_type = self.parse_data_type()?;
                if self.parse_keyword("USING") {
                    self.parse_expr()?;
                }
                out.push(AlterAction::AlterColumnType { name, data_type });
            }
        } else if self.parse_keyword("MODIFY") {
            // Oracle: MODIFY col [type] [DEFAULT x] [NOT NULL | NULL], or MODIFY (c1 ..., c2 ...)
            let paren = self.consume_token(&Token::LParen);
            loop {
                let name = self.parse_ident()?;
                if !(self.peek_keyword("DEFAULT") || self.peek_keyword("NOT") || self.peek_keyword("NULL")) {
                    out.push(AlterAction::AlterColumnType { name: name.clone(), data_type: self.parse_data_type()? });
                }
                if self.parse_keyword("DEFAULT") {
                    out.push(AlterAction::SetDefault { name: name.clone(), default: self.parse_expr()? });
                }
                if self.parse_keywords(&["NOT", "NULL"]) {
                    out.push(AlterAction::SetNotNull(name));
                } else if self.parse_keyword("NULL") {
                    out.push(AlterAction::DropNotNull(name));
                }
                if !(paren && self.consume_token(&Token::Comma)) {
                    break;
                }
            }
            if paren {
                self.expect_token(&Token::RParen)?;
            }
        } else {
            return self.expected("ADD, DROP, RENAME, ALTER or MODIFY");
        }
        Ok(())
    }

    fn parse_drop(&mut self) -> Result<Statement> {
        self.expect_keyword("DROP")?;
        let object_type = match self.parse_one_of_keywords(&["TABLE", "VIEW", "INDEX", "SEQUENCE"]) {
            Some("TABLE") => ObjectType::Table,
            Some("VIEW") => ObjectType::View,
            Some("INDEX") => ObjectType::Index,
            Some(_) => ObjectType::Sequence,
            None => return self.expected("TABLE, VIEW, INDEX or SEQUENCE"),
        };
        let if_exists = self.parse_keywords(&["IF", "EXISTS"]);
        let names = self.parse_comma_separated(Self::parse_object_name)?;
        let behavior = self.parse_drop_behavior();
        if self.dialect.supports_physical_clauses() {
            // Oracle: CASCADE CONSTRAINTS, PURGE
            self.parse_keyword("CONSTRAINTS");
            self.parse_keyword("PURGE");
        }
        Ok(Statement::Drop(Drop { object_type, if_exists, names, behavior }))
    }

    fn parse_create(&mut self) -> Result<Statement> {
        self.expect_keyword("CREATE")?;
        let or_replace = self.parse_keywords(&["OR", "REPLACE"]);
        let force = self.parse_keyword("FORCE");
        let _ = self.parse_one_of_keywords(&["EDITIONABLE", "NONEDITIONABLE"]);
        let temporary = self.parse_keywords(&["GLOBAL", "TEMPORARY"])
            || self.parse_keyword("TEMPORARY")
            || self.parse_keyword("TEMP");
        let unique = self.parse_keyword("UNIQUE");
        if self.parse_keyword("VIEW") {
            if temporary || unique {
                return self.error("TEMPORARY and UNIQUE are not valid before VIEW");
            }
            return self.parse_create_view(or_replace, force);
        }
        if or_replace || force {
            return self.expected("VIEW");
        }
        if self.parse_keyword("TABLE") {
            if unique {
                return self.error("UNIQUE is only valid before INDEX");
            }
            self.parse_create_table(temporary)
        } else if self.parse_keyword("INDEX") {
            if temporary {
                return self.error("TEMPORARY is not valid before INDEX");
            }
            self.parse_create_index(unique)
        } else if !temporary && !unique && self.parse_keyword("SEQUENCE") {
            self.parse_create_sequence()
        } else {
            self.expected("TABLE, INDEX, SEQUENCE or VIEW")
        }
    }

    fn parse_create_view(&mut self, or_replace: bool, force: bool) -> Result<Statement> {
        let name = self.parse_object_name()?;
        let columns = if *self.peek_token() == Token::LParen { self.parse_paren_idents()? } else { vec![] };
        self.expect_keyword("AS")?;
        let query = Box::new(self.parse_query()?);
        if self.dialect.supports_physical_clauses() && self.parse_keyword("WITH") {
            // WITH READ ONLY | WITH CHECK OPTION [CONSTRAINT name]
            if self.parse_keywords(&["READ", "ONLY"]) {
            } else if self.parse_keywords(&["CHECK", "OPTION"]) {
                if self.parse_keyword("CONSTRAINT") {
                    self.parse_ident()?;
                }
            } else {
                return self.expected("READ ONLY or CHECK OPTION");
            }
        }
        Ok(Statement::CreateView(CreateView { or_replace, force, name, columns, query }))
    }

    fn parse_signed_number(&mut self) -> Result<String> {
        let neg = self.consume_token(&Token::Minus);
        if !neg {
            self.consume_token(&Token::Plus);
        }
        match self.peek_token().clone() {
            Token::Number(n) => {
                self.index += 1;
                Ok(if neg { format!("-{n}") } else { n })
            }
            _ => self.expected("a number"),
        }
    }

    fn parse_create_sequence(&mut self) -> Result<Statement> {
        let if_not_exists = self.parse_keywords(&["IF", "NOT", "EXISTS"]);
        let name = self.parse_object_name()?;
        let mut options = vec![];
        loop {
            let opt = if self.parse_keywords(&["START", "WITH"]) || self.parse_keyword("START") {
                SequenceOption::StartWith(self.parse_signed_number()?)
            } else if self.parse_keywords(&["INCREMENT", "BY"]) || self.parse_keyword("INCREMENT") {
                SequenceOption::IncrementBy(self.parse_signed_number()?)
            } else if self.parse_keyword("MINVALUE") {
                SequenceOption::MinValue(Some(self.parse_signed_number()?))
            } else if self.parse_keyword("NOMINVALUE") || self.parse_keywords(&["NO", "MINVALUE"]) {
                SequenceOption::MinValue(None)
            } else if self.parse_keyword("MAXVALUE") {
                SequenceOption::MaxValue(Some(self.parse_signed_number()?))
            } else if self.parse_keyword("NOMAXVALUE") || self.parse_keywords(&["NO", "MAXVALUE"]) {
                SequenceOption::MaxValue(None)
            } else if self.parse_keyword("CYCLE") {
                SequenceOption::Cycle(true)
            } else if self.parse_keyword("NOCYCLE") || self.parse_keywords(&["NO", "CYCLE"]) {
                SequenceOption::Cycle(false)
            } else if self.parse_keyword("CACHE") {
                SequenceOption::Cache(Some(self.parse_signed_number()?))
            } else if self.parse_keyword("NOCACHE") {
                SequenceOption::Cache(None)
            } else if self.parse_keyword("ORDER") {
                SequenceOption::Order(true)
            } else if self.parse_keyword("NOORDER") {
                SequenceOption::Order(false)
            } else {
                break;
            };
            options.push(opt);
        }
        Ok(Statement::CreateSequence(CreateSequence { if_not_exists, name, options }))
    }

    fn parse_create_index(&mut self, unique: bool) -> Result<Statement> {
        let if_not_exists = self.parse_keywords(&["IF", "NOT", "EXISTS"]);
        let name = self.parse_object_name()?;
        self.expect_keyword("ON")?;
        let table = self.parse_object_name()?;
        self.expect_token(&Token::LParen)?;
        let columns = self.parse_comma_separated(Self::parse_order_by_expr)?;
        self.expect_token(&Token::RParen)?;
        self.skip_physical_clauses()?;
        let selection = if self.parse_keyword("WHERE") { Some(self.parse_expr()?) } else { None };
        Ok(Statement::CreateIndex(CreateIndex { unique, if_not_exists, name, table, columns, selection }))
    }

    fn parse_create_table(&mut self, temporary: bool) -> Result<Statement> {
        let if_not_exists = self.parse_keywords(&["IF", "NOT", "EXISTS"]);
        let name = self.parse_object_name()?;
        self.expect_token(&Token::LParen)?;
        let mut columns = vec![];
        let mut constraints = vec![];
        loop {
            let is_constraint =
                ["CONSTRAINT", "PRIMARY", "UNIQUE", "FOREIGN", "CHECK"].iter().any(|k| self.peek_keyword(k));
            if is_constraint {
                constraints.push(self.parse_table_constraint()?);
            } else {
                columns.push(self.parse_column_def()?);
            }
            if !self.consume_token(&Token::Comma) {
                break;
            }
        }
        self.expect_token(&Token::RParen)?;
        self.skip_physical_clauses()?;
        Ok(Statement::CreateTable(CreateTable { temporary, if_not_exists, name, columns, constraints }))
    }

    fn parse_generated_expr(&mut self) -> Result<ColumnOption> {
        self.expect_token(&Token::LParen)?;
        let expr = self.parse_expr()?;
        self.expect_token(&Token::RParen)?;
        let stored = match self.parse_one_of_keywords(&["VIRTUAL", "STORED"]) {
            Some(k) => k == "STORED",
            None => false,
        };
        Ok(ColumnOption::Generated { expr, stored })
    }

    /// Skips a balanced `( ... )` group.
    fn skip_balanced_parens(&mut self) -> Result<()> {
        self.expect_token(&Token::LParen)?;
        let mut depth = 1;
        while depth > 0 {
            match self.next_token() {
                Token::LParen => depth += 1,
                Token::RParen => depth -= 1,
                Token::Eof => return self.expected("')'"),
                _ => {}
            }
        }
        Ok(())
    }

    /// Accepts and ignores storage and other physical clauses (`TABLESPACE x`, `PCTFREE 10`,
    /// `COMPRESS ADVANCED`, `ROW STORE COMPRESS`, `LOGGING`, `LOB (c) STORE AS ...`, ...) after
    /// a table body or an index column list. They do not change the logical schema.
    fn skip_physical_clauses(&mut self) -> Result<()> {
        if !self.dialect.supports_physical_clauses() {
            return Ok(());
        }
        loop {
            let Token::Word(w) = self.peek_token() else { return Ok(()) };
            if w.quote.is_some() {
                return Ok(());
            }
            let kw = w.value.to_ascii_uppercase();
            let next_is = |p: &Self, k: &str| p.peek_nth_keyword(1, k);
            match kw.as_str() {
                "TABLESPACE" => {
                    self.index += 1;
                    self.parse_ident()?;
                }
                "PCTFREE" | "PCTUSED" | "INITRANS" | "MAXTRANS" => {
                    self.index += 1;
                    self.parse_signed_number()?;
                }
                "STORAGE" => {
                    self.index += 1;
                    self.skip_balanced_parens()?;
                }
                "LOGGING" | "NOLOGGING" | "NOCOMPRESS" | "NOPARALLEL" | "CACHE" | "NOCACHE" | "MONITORING"
                | "NOMONITORING" | "NOSORT" | "REVERSE" | "VISIBLE" | "INVISIBLE" | "NOROWDEPENDENCIES"
                | "ROWDEPENDENCIES" => self.index += 1,
                "COMPRESS" => {
                    self.index += 1;
                    self.skip_compress_options();
                }
                "PARALLEL" => {
                    self.index += 1;
                    if matches!(self.peek_token(), Token::Number(_)) {
                        self.index += 1;
                    }
                }
                "ROW" | "COLUMN" if next_is(self, "STORE") => {
                    self.index += 2;
                    if self.parse_keyword("COMPRESS") {
                        self.skip_compress_options();
                    }
                }
                "SEGMENT" => {
                    self.index += 1;
                    self.expect_keyword("CREATION")?;
                    if self.parse_one_of_keywords(&["IMMEDIATE", "DEFERRED"]).is_none() {
                        return self.expected("IMMEDIATE or DEFERRED");
                    }
                }
                "ENABLE" | "DISABLE" if next_is(self, "ROW") => {
                    self.index += 1;
                    self.expect_keyword("ROW")?;
                    self.expect_keyword("MOVEMENT")?;
                }
                "ON" if next_is(self, "COMMIT") => {
                    self.index += 2;
                    if self.parse_one_of_keywords(&["PRESERVE", "DELETE"]).is_none() {
                        return self.expected("PRESERVE or DELETE");
                    }
                    self.expect_keyword("ROWS")?;
                }
                "ORGANIZATION" => {
                    self.index += 1;
                    if self.parse_one_of_keywords(&["HEAP", "INDEX"]).is_none() {
                        return self.expected("HEAP or INDEX");
                    }
                }
                "LOB" => {
                    self.index += 1;
                    self.skip_balanced_parens()?;
                    self.expect_keyword("STORE")?;
                    self.expect_keyword("AS")?;
                    let _ = self.parse_one_of_keywords(&["SECUREFILE", "BASICFILE"]);
                    if matches!(self.peek_token(), Token::Word(_)) && *self.peek_nth(1) == Token::LParen {
                        self.index += 1;
                    }
                    if *self.peek_token() == Token::LParen {
                        self.skip_balanced_parens()?;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_compress_options(&mut self) {
        const WORDS: [&str; 11] =
            ["BASIC", "ADVANCED", "LOW", "HIGH", "FOR", "OLTP", "QUERY", "ARCHIVE", "DIRECT_LOAD", "OPERATIONS", "ALL"];
        loop {
            match self.peek_token() {
                Token::Number(_) => self.index += 1,
                Token::Word(w) if WORDS.iter().any(|k| w.is_keyword(k)) => self.index += 1,
                _ => return,
            }
        }
    }

    /// Accepts and ignores constraint attributes: `USING INDEX ...`, `ENABLE`, `DISABLE`,
    /// `VALIDATE`, `NOVALIDATE`, `RELY`, `DEFERRABLE`, `INITIALLY ...`.
    fn skip_constraint_attributes(&mut self) -> Result<()> {
        if !self.dialect.supports_physical_clauses() {
            return Ok(());
        }
        loop {
            if self.peek_keyword("USING") && self.peek_nth_keyword(1, "INDEX") {
                self.index += 2;
                if *self.peek_token() == Token::LParen {
                    self.skip_balanced_parens()?;
                } else {
                    self.skip_physical_clauses()?;
                }
            } else if (self.peek_keyword("ENABLE") || self.peek_keyword("DISABLE")) && !self.peek_nth_keyword(1, "ROW")
            {
                self.index += 1;
            } else if self.parse_one_of_keywords(&["VALIDATE", "NOVALIDATE", "RELY", "NORELY", "DEFERRABLE"]).is_some()
            {
            } else if self.peek_keyword("NOT") && self.peek_nth_keyword(1, "DEFERRABLE") {
                self.index += 2;
            } else if self.parse_keyword("INITIALLY") {
                if self.parse_one_of_keywords(&["DEFERRED", "IMMEDIATE"]).is_none() {
                    return self.expected("DEFERRED or IMMEDIATE");
                }
            } else {
                return Ok(());
            }
        }
    }

    fn parse_paren_idents(&mut self) -> Result<Vec<Ident>> {
        self.expect_token(&Token::LParen)?;
        let v = self.parse_comma_separated(Self::parse_ident)?;
        self.expect_token(&Token::RParen)?;
        Ok(v)
    }

    fn parse_table_constraint(&mut self) -> Result<TableConstraint> {
        let name = if self.parse_keyword("CONSTRAINT") { Some(self.parse_ident()?) } else { None };
        let kind = if self.parse_keywords(&["PRIMARY", "KEY"]) {
            TableConstraintKind::PrimaryKey(self.parse_paren_idents()?)
        } else if self.parse_keyword("UNIQUE") {
            TableConstraintKind::Unique(self.parse_paren_idents()?)
        } else if self.parse_keywords(&["FOREIGN", "KEY"]) {
            let columns = self.parse_paren_idents()?;
            self.expect_keyword("REFERENCES")?;
            TableConstraintKind::ForeignKey { columns, reference: self.parse_foreign_key_ref()? }
        } else if self.parse_keyword("CHECK") {
            self.expect_token(&Token::LParen)?;
            let e = self.parse_expr()?;
            self.expect_token(&Token::RParen)?;
            TableConstraintKind::Check(e)
        } else {
            return self.expected("PRIMARY KEY, UNIQUE, FOREIGN KEY or CHECK");
        };
        self.skip_constraint_attributes()?;
        Ok(TableConstraint { name, kind })
    }

    fn parse_foreign_key_ref(&mut self) -> Result<ForeignKeyRef> {
        let table = self.parse_object_name()?;
        let columns = if *self.peek_token() == Token::LParen { self.parse_paren_idents()? } else { vec![] };
        let (mut on_delete, mut on_update) = (None, None);
        while self.parse_keyword("ON") {
            let is_delete = match self.parse_one_of_keywords(&["DELETE", "UPDATE"]) {
                Some(k) => k == "DELETE",
                None => return self.expected("DELETE or UPDATE"),
            };
            let action = if self.parse_keyword("CASCADE") {
                "CASCADE"
            } else if self.parse_keyword("RESTRICT") {
                "RESTRICT"
            } else if self.parse_keywords(&["SET", "NULL"]) {
                "SET NULL"
            } else if self.parse_keywords(&["SET", "DEFAULT"]) {
                "SET DEFAULT"
            } else if self.parse_keywords(&["NO", "ACTION"]) {
                "NO ACTION"
            } else {
                return self.expected("referential action");
            };
            if is_delete {
                on_delete = Some(action.to_string())
            } else {
                on_update = Some(action.to_string())
            }
        }
        Ok(ForeignKeyRef { table, columns, on_delete, on_update })
    }

    fn parse_column_def(&mut self) -> Result<ColumnDef> {
        let name = self.parse_ident()?;
        let untyped = self.dialect.supports_untyped_columns()
            && match self.peek_token() {
                Token::Comma | Token::RParen | Token::Semicolon | Token::Eof => true,
                Token::Word(w) => [
                    "CONSTRAINT",
                    "NOT",
                    "NULL",
                    "DEFAULT",
                    "PRIMARY",
                    "UNIQUE",
                    "REFERENCES",
                    "CHECK",
                    "COLLATE",
                    "GENERATED",
                    "AUTOINCREMENT",
                ]
                .iter()
                .any(|k| w.is_keyword(k)),
                _ => false,
            };
        let data_type = if untyped { None } else { Some(self.parse_data_type()?) };
        let mut options = vec![];
        loop {
            let cname = if self.parse_keyword("CONSTRAINT") { Some(self.parse_ident()?) } else { None };
            let option = if self.parse_keywords(&["NOT", "NULL"]) {
                ColumnOption::NotNull
            } else if self.parse_keyword("NULL") {
                ColumnOption::Null
            } else if self.parse_keyword("DEFAULT") {
                if self.dialect.supports_physical_clauses() {
                    self.parse_keywords(&["ON", "NULL"]);
                }
                ColumnOption::Default(self.parse_expr()?)
            } else if self.parse_keywords(&["PRIMARY", "KEY"]) {
                ColumnOption::PrimaryKey
            } else if self.parse_keyword("UNIQUE") {
                ColumnOption::Unique
            } else if self.parse_keyword("AUTOINCREMENT") {
                ColumnOption::Autoincrement
            } else if self.parse_keyword("REFERENCES") {
                ColumnOption::References(self.parse_foreign_key_ref()?)
            } else if self.parse_keyword("CHECK") {
                self.expect_token(&Token::LParen)?;
                let e = self.parse_expr()?;
                self.expect_token(&Token::RParen)?;
                ColumnOption::Check(e)
            } else if self.parse_keyword("COLLATE") {
                ColumnOption::Collate(self.parse_ident()?)
            } else if self.parse_keyword("GENERATED") {
                let always = if self.parse_keyword("ALWAYS") {
                    true
                } else if self.parse_keywords(&["BY", "DEFAULT"]) {
                    self.parse_keywords(&["ON", "NULL"]);
                    false
                } else {
                    return self.expected("ALWAYS or BY DEFAULT");
                };
                self.expect_keyword("AS")?;
                if self.parse_keyword("IDENTITY") {
                    if *self.peek_token() == Token::LParen {
                        self.skip_balanced_parens()?;
                    }
                    ColumnOption::Identity { always }
                } else {
                    self.parse_generated_expr()?
                }
            } else if self.dialect.supports_physical_clauses() && self.parse_keyword("AS") {
                self.parse_generated_expr()?
            } else if cname.is_some() {
                return self.expected("a column constraint");
            } else {
                break;
            };
            options.push(ColumnOptionDef { name: cname, option });
            self.skip_constraint_attributes()?;
        }
        Ok(ColumnDef { name, data_type, options })
    }

    // ------------------------------------------------------------- queries

    pub fn parse_query(&mut self) -> Result<Query> {
        let with = if self.parse_keyword("WITH") {
            let recursive = self.parse_keyword("RECURSIVE");
            Some(With { recursive, ctes: self.parse_comma_separated(Self::parse_cte)? })
        } else {
            None
        };
        let body = self.parse_set_expr(0)?;
        let order_by = if self.parse_keywords(&["ORDER", "BY"]) {
            self.parse_comma_separated(Self::parse_order_by_expr)?
        } else {
            vec![]
        };
        let (mut limit, mut offset, mut fetch) = (None, None, None);
        loop {
            if self.dialect.supports_limit() && self.parse_keyword("LIMIT") {
                if self.parse_keyword("ALL") {
                    continue;
                }
                let first = self.parse_expr()?;
                if self.dialect.supports_limit_comma() && self.consume_token(&Token::Comma) {
                    offset = Some(first);
                    limit = Some(self.parse_expr()?);
                } else {
                    limit = Some(first);
                }
            } else if self.parse_keyword("OFFSET") {
                offset = Some(self.parse_expr()?);
                self.parse_one_of_keywords(&["ROW", "ROWS"]);
            } else if self.peek_keyword("FETCH")
                && (self.peek_nth_keyword(1, "FIRST") || self.peek_nth_keyword(1, "NEXT"))
            {
                self.next_token();
                self.next_token();
                let quantity =
                    if self.peek_keyword("ROW") || self.peek_keyword("ROWS") { None } else { Some(self.parse_expr()?) };
                let percent = self.parse_keyword("PERCENT");
                if self.parse_one_of_keywords(&["ROW", "ROWS"]).is_none() {
                    return self.expected("ROW or ROWS");
                }
                let with_ties = if self.parse_keyword("ONLY") {
                    false
                } else if self.parse_keywords(&["WITH", "TIES"]) {
                    true
                } else {
                    return self.expected("ONLY or WITH TIES");
                };
                fetch = Some(Fetch { quantity, percent, with_ties });
            } else {
                break;
            }
        }
        Ok(Query { with, body, order_by, limit, offset, fetch })
    }

    fn parse_cte(&mut self) -> Result<Cte> {
        let name = self.parse_ident()?;
        let columns = if *self.peek_token() == Token::LParen { self.parse_paren_idents()? } else { vec![] };
        self.expect_keyword("AS")?;
        self.expect_token(&Token::LParen)?;
        let query = Box::new(self.parse_query()?);
        self.expect_token(&Token::RParen)?;
        Ok(Cte { name, columns, query })
    }

    fn parse_set_expr(&mut self, min_prec: u8) -> Result<SetExpr> {
        let mut left = self.parse_set_primary()?;
        loop {
            let (op, prec) = match self.peek_token() {
                Token::Word(w) if w.quote.is_none() => match w.value.to_ascii_uppercase().as_str() {
                    "UNION" => (SetOperator::Union, 10),
                    "EXCEPT" => (SetOperator::Except, 10),
                    "MINUS" if self.dialect.supports_minus_operator() => (SetOperator::Except, 10),
                    "INTERSECT" => (SetOperator::Intersect, 20),
                    _ => break,
                },
                _ => break,
            };
            if min_prec >= prec {
                break;
            }
            self.next_token();
            let quantifier = if self.parse_keyword("ALL") {
                SetQuantifier::All
            } else if self.parse_keyword("DISTINCT") {
                SetQuantifier::Distinct
            } else {
                SetQuantifier::None
            };
            let right = self.parse_set_expr(prec)?;
            left = SetExpr::SetOperation { op, quantifier, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_set_primary(&mut self) -> Result<SetExpr> {
        if self.peek_keyword("SELECT") {
            Ok(SetExpr::Select(Box::new(self.parse_select()?)))
        } else if self.consume_token(&Token::LParen) {
            let q = self.parse_query()?;
            self.expect_token(&Token::RParen)?;
            Ok(SetExpr::Query(Box::new(q)))
        } else if self.parse_keyword("VALUES") {
            let rows = self.parse_comma_separated(|p| {
                p.expect_token(&Token::LParen)?;
                let row = p.parse_comma_separated(Self::parse_expr)?;
                p.expect_token(&Token::RParen)?;
                Ok(row)
            })?;
            Ok(SetExpr::Values(Values { rows }))
        } else {
            self.expected("SELECT, VALUES or '('")
        }
    }

    fn parse_select(&mut self) -> Result<Select> {
        self.expect_keyword("SELECT")?;
        let distinct = if self.parse_keyword("DISTINCT") {
            if self.dialect.supports_distinct_on() && self.parse_keyword("ON") {
                self.expect_token(&Token::LParen)?;
                let e = self.parse_comma_separated(Self::parse_expr)?;
                self.expect_token(&Token::RParen)?;
                Some(Distinct::On(e))
            } else {
                Some(Distinct::Distinct)
            }
        } else {
            self.parse_keyword("ALL");
            None
        };
        let projection = self.parse_comma_separated(Self::parse_select_item)?;
        let from =
            if self.parse_keyword("FROM") { self.parse_comma_separated(Self::parse_table_and_joins)? } else { vec![] };
        let selection = if self.parse_keyword("WHERE") { Some(self.parse_expr()?) } else { None };
        let group_by =
            if self.parse_keywords(&["GROUP", "BY"]) { self.parse_comma_separated(Self::parse_expr)? } else { vec![] };
        let having = if self.parse_keyword("HAVING") { Some(self.parse_expr()?) } else { None };
        Ok(Select { distinct, projection, from, selection, group_by, having })
    }

    fn parse_select_item(&mut self) -> Result<SelectItem> {
        if self.consume_token(&Token::Star) {
            return Ok(SelectItem::Wildcard);
        }
        let mut i = 0;
        while matches!(self.peek_nth(i), Token::Word(_)) && *self.peek_nth(i + 1) == Token::Period {
            if *self.peek_nth(i + 2) == Token::Star {
                let mut parts = vec![];
                for _ in 0..=(i / 2) {
                    parts.push(self.parse_ident()?);
                    self.expect_token(&Token::Period)?;
                }
                self.expect_token(&Token::Star)?;
                return Ok(SelectItem::QualifiedWildcard(ObjectName(parts)));
            }
            i += 2;
        }
        let expr = self.parse_expr()?;
        Ok(match self.parse_optional_alias()? {
            Some(alias) => SelectItem::ExprWithAlias { expr, alias },
            None => SelectItem::UnnamedExpr(expr),
        })
    }

    fn parse_optional_alias(&mut self) -> Result<Option<Ident>> {
        if self.parse_keyword("AS") {
            return Ok(Some(self.parse_ident()?));
        }
        if let Token::Word(w) = self.peek_token() {
            if w.quote.is_some() || !self.is_reserved(&w.value) {
                return Ok(Some(self.parse_ident()?));
            }
        }
        Ok(None)
    }

    fn parse_table_alias(&mut self) -> Result<Option<TableAlias>> {
        let Some(name) = self.parse_optional_alias()? else { return Ok(None) };
        let columns = if *self.peek_token() == Token::LParen { self.parse_paren_idents()? } else { vec![] };
        Ok(Some(TableAlias { name, columns }))
    }

    fn parse_table_and_joins(&mut self) -> Result<TableWithJoins> {
        let relation = self.parse_table_factor()?;
        let mut joins = vec![];
        loop {
            let natural = self.parse_keyword("NATURAL");
            let kind = if self.parse_keyword("JOIN") {
                JoinKind::Inner
            } else if self.parse_keyword("INNER") {
                self.expect_keyword("JOIN")?;
                JoinKind::Inner
            } else if self.parse_keyword("CROSS") {
                self.expect_keyword("JOIN")?;
                JoinKind::Cross
            } else if let Some(k) = self.parse_one_of_keywords(&["LEFT", "RIGHT", "FULL"]) {
                self.parse_keyword("OUTER");
                self.expect_keyword("JOIN")?;
                match k {
                    "LEFT" => JoinKind::Left,
                    "RIGHT" => JoinKind::Right,
                    _ => JoinKind::Full,
                }
            } else if natural {
                return self.expected("JOIN");
            } else {
                break;
            };
            let relation = self.parse_table_factor()?;
            let constraint = if natural {
                JoinConstraint::Natural
            } else if kind == JoinKind::Cross {
                JoinConstraint::None
            } else if self.parse_keyword("ON") {
                JoinConstraint::On(self.parse_expr()?)
            } else if self.parse_keyword("USING") {
                JoinConstraint::Using(self.parse_paren_idents()?)
            } else {
                JoinConstraint::None
            };
            joins.push(Join { kind, relation, constraint });
        }
        Ok(TableWithJoins { relation, joins })
    }

    fn parse_table_factor(&mut self) -> Result<TableFactor> {
        if self.consume_token(&Token::LParen) {
            if self.peek_query_start() {
                let subquery = Box::new(self.parse_query()?);
                self.expect_token(&Token::RParen)?;
                let alias = self.parse_table_alias()?;
                return Ok(TableFactor::Derived { subquery, alias });
            }
            let inner = self.parse_table_and_joins()?;
            self.expect_token(&Token::RParen)?;
            return Ok(TableFactor::Nested(Box::new(inner)));
        }
        let name = self.parse_object_name()?;
        let alias = self.parse_table_alias()?;
        Ok(TableFactor::Table { name, alias })
    }

    fn parse_order_by_expr(&mut self) -> Result<OrderByExpr> {
        let expr = self.parse_expr()?;
        let asc = if self.parse_keyword("ASC") {
            Some(true)
        } else if self.parse_keyword("DESC") {
            Some(false)
        } else {
            None
        };
        let nulls_first = if self.parse_keywords(&["NULLS", "FIRST"]) {
            Some(true)
        } else if self.parse_keywords(&["NULLS", "LAST"]) {
            Some(false)
        } else {
            None
        };
        Ok(OrderByExpr { expr, asc, nulls_first })
    }

    // --------------------------------------------------------- identifiers

    fn parse_ident(&mut self) -> Result<Ident> {
        match self.peek_token() {
            Token::Word(w) => {
                let id = Ident { value: w.value.clone(), quote_style: w.quote };
                self.index += 1;
                Ok(id)
            }
            _ => self.expected("identifier"),
        }
    }

    fn parse_object_name(&mut self) -> Result<ObjectName> {
        let mut parts = vec![self.parse_ident()?];
        while self.consume_token(&Token::Period) {
            parts.push(self.parse_ident()?);
        }
        Ok(ObjectName(parts))
    }

    // --------------------------------------------------------- data types

    pub fn parse_data_type(&mut self) -> Result<DataType> {
        let mut name = match self.peek_token() {
            Token::Word(w) => w.value.to_ascii_uppercase(),
            _ => return self.expected("data type"),
        };
        self.index += 1;
        let (mut args, mut suffix) = (vec![], None);
        loop {
            if *self.peek_token() == Token::LParen && args.is_empty() {
                self.next_token();
                loop {
                    args.push(self.parse_type_arg()?);
                    if !self.consume_token(&Token::Comma) {
                        break;
                    }
                }
                self.expect_token(&Token::RParen)?;
                continue;
            }
            if suffix.is_none() && args.is_empty() {
                if let Some(k) = self.parse_one_of_keywords(&["PRECISION", "VARYING"]) {
                    name.push(' ');
                    name.push_str(k);
                    continue;
                }
            }
            if suffix.is_none() && self.peek_nth_keyword(1, "TIME") {
                if let Some(k) = self.parse_one_of_keywords(&["WITH", "WITHOUT"]) {
                    self.expect_keyword("TIME")?;
                    self.expect_keyword("ZONE")?;
                    suffix = Some(format!("{k} TIME ZONE"));
                    continue;
                }
            }
            break;
        }
        let mut array_dims = 0;
        while self.consume_token(&Token::LBracket) {
            self.expect_token(&Token::RBracket)?;
            array_dims += 1;
        }
        Ok(DataType { name, args, suffix, array_dims })
    }

    fn parse_type_arg(&mut self) -> Result<String> {
        let mut parts = vec![];
        while !matches!(self.peek_token(), Token::Comma | Token::RParen | Token::Eof) {
            parts.push(self.next_token().to_string());
        }
        if parts.is_empty() {
            return self.expected("type argument");
        }
        Ok(parts.join(" "))
    }

    // --------------------------------------------------------- expressions

    pub fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_subexpr(0)
    }

    fn parse_subexpr(&mut self, prec: u8) -> Result<Expr> {
        let mut expr = self.parse_prefix()?;
        loop {
            let next = self.next_precedence();
            if prec >= next {
                return Ok(expr);
            }
            expr = self.parse_infix(expr, next)?;
        }
    }

    fn is_predicate_keyword(&self, n: usize) -> bool {
        ["IN", "BETWEEN", "LIKE"].iter().any(|k| self.peek_nth_keyword(n, k))
            || (self.dialect.supports_ilike() && self.peek_nth_keyword(n, "ILIKE"))
            || (self.dialect.supports_glob() && self.peek_nth_keyword(n, "GLOB"))
    }

    fn next_precedence(&self) -> u8 {
        match self.peek_token() {
            Token::Word(w) if w.quote.is_none() => match w.value.to_ascii_uppercase().as_str() {
                "OR" => 5,
                "AND" => 10,
                "IS" => 20,
                "IN" | "BETWEEN" | "LIKE" => 20,
                "ILIKE" | "GLOB" if self.is_predicate_keyword(0) => 20,
                "NOT" if self.is_predicate_keyword(1) => 20,
                _ => 0,
            },
            Token::Eq | Token::Neq | Token::Lt | Token::LtEq | Token::Gt | Token::GtEq => 20,
            Token::Concat => 30,
            Token::Arrow | Token::LongArrow if self.dialect.supports_json_arrows() => 30,
            Token::Plus | Token::Minus => 40,
            Token::Star | Token::Slash | Token::Percent => 50,
            Token::DoubleColon if self.dialect.supports_double_colon_cast() => 70,
            _ => 0,
        }
    }

    fn parse_infix(&mut self, left: Expr, prec: u8) -> Result<Expr> {
        let tok = self.next_token();
        let op = match &tok {
            Token::Plus => Some(BinaryOperator::Plus),
            Token::Minus => Some(BinaryOperator::Minus),
            Token::Star => Some(BinaryOperator::Multiply),
            Token::Slash => Some(BinaryOperator::Divide),
            Token::Percent => Some(BinaryOperator::Modulo),
            Token::Concat => Some(BinaryOperator::Concat),
            Token::Eq => Some(BinaryOperator::Eq),
            Token::Neq => Some(BinaryOperator::NotEq),
            Token::Lt => Some(BinaryOperator::Lt),
            Token::LtEq => Some(BinaryOperator::LtEq),
            Token::Gt => Some(BinaryOperator::Gt),
            Token::GtEq => Some(BinaryOperator::GtEq),
            Token::Arrow => Some(BinaryOperator::JsonGet),
            Token::LongArrow => Some(BinaryOperator::JsonGetText),
            Token::Word(w) if w.is_keyword("AND") => Some(BinaryOperator::And),
            Token::Word(w) if w.is_keyword("OR") => Some(BinaryOperator::Or),
            _ => None,
        };
        if let Some(op) = op {
            let right = self.parse_subexpr(prec)?;
            return Ok(Expr::BinaryOp { left: Box::new(left), op, right: Box::new(right) });
        }
        let left = Box::new(left);
        match tok {
            Token::DoubleColon => Ok(Expr::Cast { expr: left, data_type: self.parse_data_type()? }),
            Token::Word(w) => {
                let upper = w.value.to_ascii_uppercase();
                if upper == "IS" {
                    let negated = self.parse_keyword("NOT");
                    self.expect_keyword("NULL")?;
                    return Ok(if negated { Expr::IsNotNull(left) } else { Expr::IsNull(left) });
                }
                let (negated, kw) = if upper == "NOT" {
                    match self.next_token() {
                        Token::Word(w2) => (true, w2.value.to_ascii_uppercase()),
                        _ => return self.expected("IN, BETWEEN or LIKE"),
                    }
                } else {
                    (false, upper)
                };
                match kw.as_str() {
                    "IN" => {
                        self.expect_token(&Token::LParen)?;
                        if self.peek_query_start() {
                            let subquery = Box::new(self.parse_query()?);
                            self.expect_token(&Token::RParen)?;
                            Ok(Expr::InSubquery { expr: left, subquery, negated })
                        } else {
                            let list = self.parse_comma_separated(Self::parse_expr)?;
                            self.expect_token(&Token::RParen)?;
                            Ok(Expr::InList { expr: left, list, negated })
                        }
                    }
                    "BETWEEN" => {
                        let low = Box::new(self.parse_subexpr(20)?);
                        self.expect_keyword("AND")?;
                        let high = Box::new(self.parse_subexpr(20)?);
                        Ok(Expr::Between { expr: left, negated, low, high })
                    }
                    "LIKE" | "ILIKE" | "GLOB" => {
                        let kind = match kw.as_str() {
                            "LIKE" => LikeKind::Like,
                            "ILIKE" => LikeKind::ILike,
                            _ => LikeKind::Glob,
                        };
                        let pattern = Box::new(self.parse_subexpr(20)?);
                        let escape =
                            if self.parse_keyword("ESCAPE") { Some(Box::new(self.parse_subexpr(20)?)) } else { None };
                        Ok(Expr::Like { expr: left, negated, kind, pattern, escape })
                    }
                    _ => self.expected("IN, BETWEEN or LIKE"),
                }
            }
            _ => self.expected("operator"),
        }
    }

    fn parse_prefix(&mut self) -> Result<Expr> {
        match self.next_token() {
            Token::Number(n) => Ok(Expr::Value(Value::Number(n))),
            Token::SingleQuotedString(s) => Ok(Expr::Value(Value::String(s))),
            Token::Placeholder(p) => Ok(Expr::Placeholder(p)),
            Token::Plus => Ok(Expr::UnaryOp { op: UnaryOperator::Plus, expr: Box::new(self.parse_subexpr(60)?) }),
            Token::Minus => Ok(Expr::UnaryOp { op: UnaryOperator::Minus, expr: Box::new(self.parse_subexpr(60)?) }),
            Token::LParen => {
                if self.peek_query_start() {
                    let q = self.parse_query()?;
                    self.expect_token(&Token::RParen)?;
                    return Ok(Expr::Subquery(Box::new(q)));
                }
                let first = self.parse_expr()?;
                if self.consume_token(&Token::Comma) {
                    let mut items = vec![first];
                    items.extend(self.parse_comma_separated(Self::parse_expr)?);
                    self.expect_token(&Token::RParen)?;
                    Ok(Expr::Tuple(items))
                } else {
                    self.expect_token(&Token::RParen)?;
                    Ok(Expr::Nested(Box::new(first)))
                }
            }
            Token::Word(w) => self.parse_word_prefix(w),
            t => {
                if t != Token::Eof {
                    self.index -= 1;
                }
                self.expected("expression")
            }
        }
    }

    fn parse_word_prefix(&mut self, w: Word) -> Result<Expr> {
        if w.quote.is_none() {
            let upper = w.value.to_ascii_uppercase();
            let paren = *self.peek_token() == Token::LParen;
            match upper.as_str() {
                "NULL" => return Ok(Expr::Value(Value::Null)),
                "TRUE" => return Ok(Expr::Value(Value::Boolean(true))),
                "FALSE" => return Ok(Expr::Value(Value::Boolean(false))),
                "CASE" => return self.parse_case(),
                "CAST" if paren => {
                    self.next_token();
                    let expr = Box::new(self.parse_expr()?);
                    self.expect_keyword("AS")?;
                    let data_type = self.parse_data_type()?;
                    self.expect_token(&Token::RParen)?;
                    return Ok(Expr::Cast { expr, data_type });
                }
                "EXISTS" if paren => {
                    self.next_token();
                    let subquery = Box::new(self.parse_query()?);
                    self.expect_token(&Token::RParen)?;
                    return Ok(Expr::Exists { subquery, negated: false });
                }
                "NOT" => {
                    return Ok(match self.parse_subexpr(15)? {
                        Expr::Exists { subquery, negated } => Expr::Exists { subquery, negated: !negated },
                        other => Expr::UnaryOp { op: UnaryOperator::Not, expr: Box::new(other) },
                    });
                }
                "DATE" | "TIMESTAMP" | "TIME" | "INTERVAL"
                    if matches!(self.peek_token(), Token::SingleQuotedString(_)) =>
                {
                    let Token::SingleQuotedString(value) = self.next_token() else { unreachable!() };
                    let data_type = DataType { name: upper, args: vec![], suffix: None, array_dims: 0 };
                    return Ok(Expr::TypedString { data_type, value });
                }
                _ => {}
            }
            if self.is_reserved(&w.value) && !paren {
                self.index -= 1;
                return self.error(format!("unexpected keyword {}", w.value.to_ascii_uppercase()));
            }
        }
        let mut parts = vec![Ident { value: w.value, quote_style: w.quote }];
        while self.consume_token(&Token::Period) {
            parts.push(self.parse_ident()?);
        }
        if *self.peek_token() == Token::LParen {
            return self.parse_function(ObjectName(parts));
        }
        Ok(if parts.len() == 1 { Expr::Identifier(parts.pop().unwrap()) } else { Expr::CompoundIdentifier(parts) })
    }

    fn parse_case(&mut self) -> Result<Expr> {
        let operand = if self.peek_keyword("WHEN") { None } else { Some(Box::new(self.parse_expr()?)) };
        let mut branches = vec![];
        while self.parse_keyword("WHEN") {
            let condition = self.parse_expr()?;
            self.expect_keyword("THEN")?;
            branches.push(CaseBranch { condition, result: self.parse_expr()? });
        }
        if branches.is_empty() {
            return self.expected("WHEN");
        }
        let else_result = if self.parse_keyword("ELSE") { Some(Box::new(self.parse_expr()?)) } else { None };
        self.expect_keyword("END")?;
        Ok(Expr::Case { operand, branches, else_result })
    }

    /// After `TRIM (`: the `[side] [chars] FROM expr` form, or `None` (nothing consumed) for an ordinary call.
    fn parse_trim_from(&mut self) -> Result<Option<Expr>> {
        let start = self.index;
        let side = match self.parse_one_of_keywords(&["LEADING", "TRAILING", "BOTH"]) {
            Some("LEADING") => Some(TrimSide::Leading),
            Some("TRAILING") => Some(TrimSide::Trailing),
            Some(_) => Some(TrimSide::Both),
            None => None,
        };
        let chars = if self.peek_keyword("FROM") { None } else { Some(Box::new(self.parse_expr()?)) };
        if !self.parse_keyword("FROM") {
            if side.is_some() {
                return self.expected("FROM");
            }
            // `TRIM(expr)` or `TRIM(a, b)`: an ordinary call
            self.index = start;
            return Ok(None);
        }
        let expr = Box::new(self.parse_expr()?);
        self.expect_token(&Token::RParen)?;
        Ok(Some(Expr::Trim { side, chars, expr }))
    }

    fn parse_function(&mut self, name: ObjectName) -> Result<Expr> {
        self.expect_token(&Token::LParen)?;
        if matches!(name.0.as_slice(), [n] if n.value.eq_ignore_ascii_case("trim")) {
            if let Some(e) = self.parse_trim_from()? {
                return Ok(e);
            }
        }
        let distinct = self.parse_keyword("DISTINCT");
        let mut args = vec![];
        if !self.consume_token(&Token::RParen) {
            loop {
                if self.consume_token(&Token::Star) {
                    args.push(FunctionArg::Wildcard);
                } else {
                    args.push(FunctionArg::Expr(self.parse_expr()?));
                }
                if self.consume_token(&Token::Comma) {
                    continue;
                }
                self.expect_token(&Token::RParen)?;
                break;
            }
        }
        let over = if self.parse_keyword("OVER") {
            self.expect_token(&Token::LParen)?;
            let partition_by = if self.parse_keywords(&["PARTITION", "BY"]) {
                self.parse_comma_separated(Self::parse_expr)?
            } else {
                vec![]
            };
            let order_by = if self.parse_keywords(&["ORDER", "BY"]) {
                self.parse_comma_separated(Self::parse_order_by_expr)?
            } else {
                vec![]
            };
            self.expect_token(&Token::RParen)?;
            Some(WindowSpec { partition_by, order_by })
        } else {
            None
        };
        Ok(Expr::Function(Function { name, distinct, args, over }))
    }
}
