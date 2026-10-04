use crate::dialect::Dialect;
use crate::error::{Location, ParseError, Result};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub value: String,
    pub quote: Option<char>,
}

impl Word {
    pub fn is_keyword(&self, kw: &str) -> bool {
        self.quote.is_none() && self.value.eq_ignore_ascii_case(kw)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(Word),
    Number(String),
    SingleQuotedString(String),
    Placeholder(String),
    Comma,
    Period,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Semicolon,
    Colon,
    DoubleColon,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Neq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Concat,
    Arrow,
    LongArrow,
    Other(char),
    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Word(w) => match w.quote {
                Some('[') => write!(f, "[{}]", w.value),
                Some(q) => write!(f, "{q}{}{q}", w.value),
                None => f.write_str(&w.value),
            },
            Token::Number(n) => f.write_str(n),
            Token::SingleQuotedString(s) => write!(f, "'{s}'"),
            Token::Placeholder(p) => f.write_str(p),
            Token::Comma => f.write_str(","),
            Token::Period => f.write_str("."),
            Token::LParen => f.write_str("("),
            Token::RParen => f.write_str(")"),
            Token::LBracket => f.write_str("["),
            Token::RBracket => f.write_str("]"),
            Token::Semicolon => f.write_str(";"),
            Token::Colon => f.write_str(":"),
            Token::DoubleColon => f.write_str("::"),
            Token::Plus => f.write_str("+"),
            Token::Minus => f.write_str("-"),
            Token::Star => f.write_str("*"),
            Token::Slash => f.write_str("/"),
            Token::Percent => f.write_str("%"),
            Token::Eq => f.write_str("="),
            Token::Neq => f.write_str("<>"),
            Token::Lt => f.write_str("<"),
            Token::LtEq => f.write_str("<="),
            Token::Gt => f.write_str(">"),
            Token::GtEq => f.write_str(">="),
            Token::Concat => f.write_str("||"),
            Token::Arrow => f.write_str("->"),
            Token::LongArrow => f.write_str("->>"),
            Token::Other(c) => write!(f, "{c}"),
            Token::Eof => f.write_str("end of input"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TokenWithLocation {
    pub token: Token,
    pub location: Location,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub struct Lexer<'a> {
    dialect: &'a dyn Dialect,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(dialect: &'a dyn Dialect, sql: &str) -> Self {
        Self { dialect, chars: sql.chars().collect(), pos: 0, line: 1, col: 1 }
    }

    pub fn tokenize(&mut self) -> Result<Vec<TokenWithLocation>> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia()?;
            let location = self.location();
            match self.next_token()? {
                Some(token) => out.push(TokenWithLocation { token, location }),
                None => {
                    out.push(TokenWithLocation { token: Token::Eof, location });
                    return Ok(out);
                }
            }
        }
    }

    fn location(&self) -> Location {
        Location { line: self.line, column: self.col }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn err<T>(&self, msg: &str, at: Location) -> Result<T> {
        Err(ParseError::new(msg, Some(at)))
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('-') if self.peek_at(1) == Some('-') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    let start = self.location();
                    let nested = self.dialect.supports_nested_block_comments();
                    self.bump();
                    self.bump();
                    let mut depth = 1;
                    loop {
                        match (self.peek(), self.peek_at(1)) {
                            (Some('*'), Some('/')) => {
                                self.bump();
                                self.bump();
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            (Some('/'), Some('*')) if nested => {
                                self.bump();
                                self.bump();
                                depth += 1;
                            }
                            (Some(_), _) => {
                                self.bump();
                            }
                            (None, _) => return self.err("unterminated block comment", start),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// Reads up to `end` (the opening delimiter is already consumed).
    fn read_delimited(&mut self, end: char, doubled: bool, what: &str) -> Result<String> {
        let start = self.location();
        let mut s = String::new();
        loop {
            match self.bump() {
                Some(c) if c == end => {
                    if doubled && self.peek() == Some(end) {
                        self.bump();
                        s.push(end);
                    } else {
                        return Ok(s);
                    }
                }
                Some(c) => s.push(c),
                None => return self.err(what, start),
            }
        }
    }

    fn read_escape_string(&mut self) -> Result<String> {
        let start = self.location();
        let mut s = String::new();
        loop {
            match self.bump() {
                Some('\\') => match self.bump() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some(c) => s.push(c),
                    None => return self.err("unterminated string literal", start),
                },
                Some('\'') => {
                    if self.peek() == Some('\'') {
                        self.bump();
                        s.push('\'');
                    } else {
                        return Ok(s);
                    }
                }
                Some(c) => s.push(c),
                None => return self.err("unterminated string literal", start),
            }
        }
    }

    /// Oracle `q'[...]'`; the `q'` prefix is already consumed.
    fn read_q_string(&mut self) -> Result<String> {
        let start = self.location();
        let open = match self.bump() {
            Some(c) if !c.is_whitespace() => c,
            _ => return self.err("invalid q-quote delimiter", start),
        };
        let close = match open {
            '[' => ']',
            '(' => ')',
            '{' => '}',
            '<' => '>',
            c => c,
        };
        let mut s = String::new();
        loop {
            match self.bump() {
                Some(c) if c == close && self.peek() == Some('\'') => {
                    self.bump();
                    return Ok(s);
                }
                Some(c) => s.push(c),
                None => return self.err("unterminated q-quoted string", start),
            }
        }
    }

    fn try_dollar_quoted(&mut self) -> Result<Option<String>> {
        if !self.dialect.supports_dollar_quoted_strings() {
            return Ok(None);
        }
        let mut i = 1;
        while self.peek_at(i).is_some_and(is_word_char) {
            i += 1;
        }
        if self.peek_at(i) != Some('$') {
            return Ok(None);
        }
        let tag: Vec<char> = self.chars[self.pos + 1..self.pos + i].to_vec();
        if tag.first().is_some_and(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        let mut delim = vec!['$'];
        delim.extend(&tag);
        delim.push('$');
        let start = self.location();
        for _ in 0..delim.len() {
            self.bump();
        }
        let mut s = String::new();
        loop {
            if self.chars[self.pos..].starts_with(&delim) {
                for _ in 0..delim.len() {
                    self.bump();
                }
                return Ok(Some(s));
            }
            match self.bump() {
                Some(c) => s.push(c),
                None => return self.err("unterminated dollar-quoted string", start),
            }
        }
    }

    fn read_number(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            s.push(c);
            self.bump();
        }
        if self.peek() == Some('.') && self.peek_at(1) != Some('.') {
            s.push('.');
            self.bump();
            while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                s.push(c);
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let n1 = self.peek_at(1);
            let digit_after_sign = matches!(n1, Some('+' | '-')) && self.peek_at(2).is_some_and(|c| c.is_ascii_digit());
            if n1.is_some_and(|c| c.is_ascii_digit()) || digit_after_sign {
                s.push(self.bump().unwrap());
                if matches!(self.peek(), Some('+' | '-')) {
                    s.push(self.bump().unwrap());
                }
                while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                    s.push(c);
                    self.bump();
                }
            }
        }
        s
    }

    fn read_word(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if self.dialect.is_identifier_part(c) {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        s
    }

    fn read_placeholder(&mut self, prefix: char) -> Token {
        self.bump();
        let mut s = String::from(prefix);
        while let Some(c) = self.peek().filter(|c| is_word_char(*c)) {
            s.push(c);
            self.bump();
        }
        Token::Placeholder(s)
    }

    fn next_token(&mut self) -> Result<Option<Token>> {
        let Some(c) = self.peek() else { return Ok(None) };
        let next_is_word = self.peek_at(1).is_some_and(is_word_char);
        let tok = match c {
            '\'' => {
                self.bump();
                Token::SingleQuotedString(self.read_delimited('\'', true, "unterminated string literal")?)
            }
            c if self.dialect.is_delimited_identifier_start(c) => {
                self.bump();
                let (end, doubled) = if c == '[' { (']', false) } else { (c, true) };
                let value = self.read_delimited(end, doubled, "unterminated quoted identifier")?;
                Token::Word(Word { value, quote: Some(c) })
            }
            c if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) => {
                Token::Number(self.read_number())
            }
            c if self.dialect.is_identifier_start(c) => {
                let quote_follows = self.peek_at(1) == Some('\'');
                if matches!(c, 'e' | 'E') && quote_follows && self.dialect.supports_escape_strings() {
                    self.bump();
                    self.bump();
                    Token::SingleQuotedString(self.read_escape_string()?)
                } else if matches!(c, 'q' | 'Q') && quote_follows && self.dialect.supports_q_quote_strings() {
                    self.bump();
                    self.bump();
                    Token::SingleQuotedString(self.read_q_string()?)
                } else {
                    Token::Word(Word { value: self.read_word(), quote: None })
                }
            }
            '$' => {
                if let Some(s) = self.try_dollar_quoted()? {
                    Token::SingleQuotedString(s)
                } else if self.dialect.supports_dollar_placeholders() && next_is_word {
                    self.read_placeholder('$')
                } else {
                    self.bump();
                    Token::Other('$')
                }
            }
            '?' if self.dialect.supports_question_placeholders() => self.read_placeholder('?'),
            ':' => {
                if self.peek_at(1) == Some(':') {
                    self.bump();
                    self.bump();
                    Token::DoubleColon
                } else if self.dialect.supports_colon_placeholders() && next_is_word {
                    self.read_placeholder(':')
                } else {
                    self.bump();
                    Token::Colon
                }
            }
            '@' if self.dialect.supports_at_placeholders() && next_is_word => self.read_placeholder('@'),
            '-' => {
                self.bump();
                if self.peek() == Some('>') {
                    self.bump();
                    if self.peek() == Some('>') {
                        self.bump();
                        Token::LongArrow
                    } else {
                        Token::Arrow
                    }
                } else {
                    Token::Minus
                }
            }
            '<' => {
                self.bump();
                match self.peek() {
                    Some('=') => {
                        self.bump();
                        Token::LtEq
                    }
                    Some('>') => {
                        self.bump();
                        Token::Neq
                    }
                    _ => Token::Lt,
                }
            }
            '>' => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    Token::GtEq
                } else {
                    Token::Gt
                }
            }
            '!' => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    Token::Neq
                } else {
                    Token::Other('!')
                }
            }
            '|' => {
                self.bump();
                if self.peek() == Some('|') {
                    self.bump();
                    Token::Concat
                } else {
                    Token::Other('|')
                }
            }
            '=' => {
                self.bump();
                Token::Eq
            }
            other => {
                self.bump();
                match other {
                    ',' => Token::Comma,
                    '.' => Token::Period,
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    '[' => Token::LBracket,
                    ']' => Token::RBracket,
                    ';' => Token::Semicolon,
                    '+' => Token::Plus,
                    '*' => Token::Star,
                    '/' => Token::Slash,
                    '%' => Token::Percent,
                    c => Token::Other(c),
                }
            }
        };
        Ok(Some(tok))
    }
}
