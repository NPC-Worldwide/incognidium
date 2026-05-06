/// JavaScript token types.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    Identifier(String),

    // Keywords
    Var,
    Let,
    Const,
    Function,
    Return,
    If,
    Else,
    While,
    For,
    Do,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    New,
    This,
    Typeof,
    Instanceof,
    In,
    Of,
    Throw,
    Try,
    Catch,
    Finally,
    Class,
    Extends,
    Super,
    Import,
    Export,
    Void,
    Delete,
    Yield,
    Async,
    Await,
    Debugger,

    // Operators
    Plus,            // +
    Minus,           // -
    Star,            // *
    Slash,           // /
    Percent,         // %
    StarStar,        // **
    Assign,          // =
    PlusAssign,      // +=
    MinusAssign,     // -=
    StarAssign,      // *=
    SlashAssign,     // /=
    PercentAssign,   // %=
    Equal,           // ==
    NotEqual,        // !=
    StrictEqual,     // ===
    StrictNotEqual,  // !==
    Less,            // <
    Greater,         // >
    LessEqual,       // <=
    GreaterEqual,    // >=
    And,             // &&
    Or,              // ||
    Not,             // !
    BitAnd,          // &
    BitOr,           // |
    BitXor,          // ^
    BitNot,          // ~
    ShiftLeft,       // <<
    ShiftRight,      // >>
    UShiftRight,     // >>>
    QuestionMark,    // ?
    NullishCoalesce, // ??
    OptionalChain,   // ?.
    Arrow,           // =>
    Spread,          // ...
    PlusPlus,        // ++
    MinusMinus,      // --

    // Delimiters
    LeftParen,    // (
    RightParen,   // )
    LeftBrace,    // {
    RightBrace,   // }
    LeftBracket,  // [
    RightBracket, // ]
    Semicolon,    // ;
    Comma,        // ,
    Dot,          // .
    Colon,        // :

    // Template literals
    TemplateLiteral(String),

    // Special
    Eof,
}

/// Position in source code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub col: u32,
}

/// A token with its position.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: Span,
}

/// Lexer for JavaScript source code.
pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.token == Token::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn peek(&self) -> char {
        if self.pos < self.source.len() {
            self.source[self.pos]
        } else {
            '\0'
        }
    }

    fn peek_at(&self, offset: usize) -> char {
        let idx = self.pos + offset;
        if idx < self.source.len() {
            self.source[idx]
        } else {
            '\0'
        }
    }

    fn advance(&mut self) -> char {
        let ch = self.peek();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        self.pos += 1;
        ch
    }

    fn span_from(&self, start: usize, start_line: u32, start_col: u32) -> Span {
        Span {
            start,
            end: self.pos,
            line: start_line,
            col: start_col,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.source.len() {
            match self.peek() {
                ' ' | '\t' | '\r' | '\n' => {
                    self.advance();
                }
                '/' if self.peek_at(1) == '/' => {
                    // Line comment
                    while self.pos < self.source.len() && self.peek() != '\n' {
                        self.advance();
                    }
                }
                '/' if self.peek_at(1) == '*' => {
                    // Block comment
                    self.advance();
                    self.advance();
                    while self.pos < self.source.len() {
                        if self.peek() == '*' && self.peek_at(1) == '/' {
                            self.advance();
                            self.advance();
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    pub fn next_token(&mut self) -> SpannedToken {
        self.skip_whitespace();

        let start = self.pos;
        let start_line = self.line;
        let start_col = self.col;

        if self.pos >= self.source.len() {
            return SpannedToken {
                token: Token::Eof,
                span: self.span_from(start, start_line, start_col),
            };
        }

        let ch = self.peek();

        let token = match ch {
            // Numbers
            '0'..='9' => self.read_number(),

            // Strings
            '"' | '\'' => self.read_string(ch),
            '`' => self.read_template_literal(),

            // Identifiers and keywords
            'a'..='z' | 'A'..='Z' | '_' | '$' => self.read_identifier(),

            // Operators and punctuation
            '+' => {
                self.advance();
                if self.peek() == '+' {
                    self.advance();
                    Token::PlusPlus
                } else if self.peek() == '=' {
                    self.advance();
                    Token::PlusAssign
                } else {
                    Token::Plus
                }
            }
            '-' => {
                self.advance();
                if self.peek() == '-' {
                    self.advance();
                    Token::MinusMinus
                } else if self.peek() == '=' {
                    self.advance();
                    Token::MinusAssign
                } else {
                    Token::Minus
                }
            }
            '*' => {
                self.advance();
                if self.peek() == '*' {
                    self.advance();
                    Token::StarStar
                } else if self.peek() == '=' {
                    self.advance();
                    Token::StarAssign
                } else {
                    Token::Star
                }
            }
            '/' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::SlashAssign
                } else {
                    Token::Slash
                }
            }
            '%' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::PercentAssign
                } else {
                    Token::Percent
                }
            }
            '=' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Token::StrictEqual
                    } else {
                        Token::Equal
                    }
                } else if self.peek() == '>' {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Assign
                }
            }
            '!' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    if self.peek() == '=' {
                        self.advance();
                        Token::StrictNotEqual
                    } else {
                        Token::NotEqual
                    }
                } else {
                    Token::Not
                }
            }
            '<' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::LessEqual
                } else if self.peek() == '<' {
                    self.advance();
                    Token::ShiftLeft
                } else {
                    Token::Less
                }
            }
            '>' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::GreaterEqual
                } else if self.peek() == '>' {
                    self.advance();
                    if self.peek() == '>' {
                        self.advance();
                        Token::UShiftRight
                    } else {
                        Token::ShiftRight
                    }
                } else {
                    Token::Greater
                }
            }
            '&' => {
                self.advance();
                if self.peek() == '&' {
                    self.advance();
                    Token::And
                } else {
                    Token::BitAnd
                }
            }
            '|' => {
                self.advance();
                if self.peek() == '|' {
                    self.advance();
                    Token::Or
                } else {
                    Token::BitOr
                }
            }
            '^' => {
                self.advance();
                Token::BitXor
            }
            '~' => {
                self.advance();
                Token::BitNot
            }
            '?' => {
                self.advance();
                if self.peek() == '?' {
                    self.advance();
                    Token::NullishCoalesce
                } else if self.peek() == '.' {
                    self.advance();
                    Token::OptionalChain
                } else {
                    Token::QuestionMark
                }
            }
            '.' => {
                self.advance();
                if self.peek() == '.' && self.peek_at(1) == '.' {
                    self.advance();
                    self.advance();
                    Token::Spread
                } else {
                    Token::Dot
                }
            }
            '(' => {
                self.advance();
                Token::LeftParen
            }
            ')' => {
                self.advance();
                Token::RightParen
            }
            '{' => {
                self.advance();
                Token::LeftBrace
            }
            '}' => {
                self.advance();
                Token::RightBrace
            }
            '[' => {
                self.advance();
                Token::LeftBracket
            }
            ']' => {
                self.advance();
                Token::RightBracket
            }
            ';' => {
                self.advance();
                Token::Semicolon
            }
            ',' => {
                self.advance();
                Token::Comma
            }
            ':' => {
                self.advance();
                Token::Colon
            }

            _ => {
                self.advance();
                // Unknown character, skip
                Token::Eof
            }
        };

        SpannedToken {
            token,
            span: self.span_from(start, start_line, start_col),
        }
    }

    fn read_number(&mut self) -> Token {
        let mut s = String::new();
        let mut has_dot = false;

        // Handle 0x, 0o, 0b prefixes
        if self.peek() == '0' {
            s.push(self.advance());
            match self.peek() {
                'x' | 'X' => {
                    s.push(self.advance());
                    while self.peek().is_ascii_hexdigit() {
                        s.push(self.advance());
                    }
                    return Token::Number(i64::from_str_radix(&s[2..], 16).unwrap_or(0) as f64);
                }
                'o' | 'O' => {
                    s.push(self.advance());
                    while matches!(self.peek(), '0'..='7') {
                        s.push(self.advance());
                    }
                    return Token::Number(i64::from_str_radix(&s[2..], 8).unwrap_or(0) as f64);
                }
                'b' | 'B' => {
                    s.push(self.advance());
                    while matches!(self.peek(), '0' | '1') {
                        s.push(self.advance());
                    }
                    return Token::Number(i64::from_str_radix(&s[2..], 2).unwrap_or(0) as f64);
                }
                _ => {}
            }
        }

        while self.pos < self.source.len() {
            let c = self.peek();
            if c.is_ascii_digit() {
                s.push(self.advance());
            } else if c == '.' && !has_dot {
                has_dot = true;
                s.push(self.advance());
            } else if c == 'e' || c == 'E' {
                s.push(self.advance());
                if self.peek() == '+' || self.peek() == '-' {
                    s.push(self.advance());
                }
            } else {
                break;
            }
        }

        Token::Number(s.parse::<f64>().unwrap_or(0.0))
    }

    fn read_string(&mut self, quote: char) -> Token {
        self.advance(); // skip opening quote
        let mut s = String::new();
        while self.pos < self.source.len() && self.peek() != quote {
            if self.peek() == '\\' {
                self.advance();
                match self.peek() {
                    'n' => {
                        self.advance();
                        s.push('\n');
                    }
                    't' => {
                        self.advance();
                        s.push('\t');
                    }
                    'r' => {
                        self.advance();
                        s.push('\r');
                    }
                    '\\' => {
                        self.advance();
                        s.push('\\');
                    }
                    '\'' => {
                        self.advance();
                        s.push('\'');
                    }
                    '"' => {
                        self.advance();
                        s.push('"');
                    }
                    '0' => {
                        self.advance();
                        s.push('\0');
                    }
                    'u' => {
                        self.advance();
                        // Unicode escape: \uXXXX or \u{XXXX}
                        if self.peek() == '{' {
                            self.advance();
                            let mut hex = String::new();
                            while self.peek() != '}' && self.pos < self.source.len() {
                                hex.push(self.advance());
                            }
                            self.advance(); // skip }
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    s.push(c);
                                }
                            }
                        } else {
                            let mut hex = String::new();
                            for _ in 0..4 {
                                hex.push(self.advance());
                            }
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    s.push(c);
                                }
                            }
                        }
                    }
                    c => {
                        self.advance();
                        s.push(c);
                    }
                }
            } else {
                s.push(self.advance());
            }
        }
        if self.pos < self.source.len() {
            self.advance(); // skip closing quote
        }
        Token::String(s)
    }

    fn read_template_literal(&mut self) -> Token {
        self.advance(); // skip `
        let mut s = String::new();
        while self.pos < self.source.len() && self.peek() != '`' {
            if self.peek() == '\\' {
                self.advance();
                s.push(self.advance());
            } else {
                s.push(self.advance());
            }
        }
        if self.pos < self.source.len() {
            self.advance(); // skip closing `
        }
        Token::TemplateLiteral(s)
    }

    fn read_identifier(&mut self) -> Token {
        let mut s = String::new();
        while self.pos < self.source.len() {
            let c = self.peek();
            if c.is_alphanumeric() || c == '_' || c == '$' {
                s.push(self.advance());
            } else {
                break;
            }
        }

        match s.as_str() {
            "var" => Token::Var,
            "let" => Token::Let,
            "const" => Token::Const,
            "function" => Token::Function,
            "return" => Token::Return,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "do" => Token::Do,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "switch" => Token::Switch,
            "case" => Token::Case,
            "default" => Token::Default,
            "new" => Token::New,
            "this" => Token::This,
            "typeof" => Token::Typeof,
            "instanceof" => Token::Instanceof,
            "in" => Token::In,
            "of" => Token::Of,
            "throw" => Token::Throw,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "finally" => Token::Finally,
            "class" => Token::Class,
            "extends" => Token::Extends,
            "super" => Token::Super,
            "import" => Token::Import,
            "export" => Token::Export,
            "void" => Token::Void,
            "delete" => Token::Delete,
            "yield" => Token::Yield,
            "async" => Token::Async,
            "await" => Token::Await,
            "debugger" => Token::Debugger,
            "true" => Token::Bool(true),
            "false" => Token::Bool(false),
            "null" => Token::Null,
            "undefined" => Token::Undefined,
            _ => Token::Identifier(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let mut lexer = Lexer::new("var x = 42;");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::Var);
        assert_eq!(tokens[1].token, Token::Identifier("x".into()));
        assert_eq!(tokens[2].token, Token::Assign);
        assert_eq!(tokens[3].token, Token::Number(42.0));
        assert_eq!(tokens[4].token, Token::Semicolon);
        assert_eq!(tokens[5].token, Token::Eof);
    }

    #[test]
    fn test_string_literals() {
        let mut lexer = Lexer::new(r#""hello" 'world'"#);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::String("hello".into()));
        assert_eq!(tokens[1].token, Token::String("world".into()));
    }

    #[test]
    fn test_operators() {
        let mut lexer = Lexer::new("=== !== => ... ?? ?.");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::StrictEqual);
        assert_eq!(tokens[1].token, Token::StrictNotEqual);
        assert_eq!(tokens[2].token, Token::Arrow);
        assert_eq!(tokens[3].token, Token::Spread);
        assert_eq!(tokens[4].token, Token::NullishCoalesce);
        assert_eq!(tokens[5].token, Token::OptionalChain);
    }

    #[test]
    fn test_keywords() {
        let mut lexer = Lexer::new("function if else return true false null");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::Function);
        assert_eq!(tokens[1].token, Token::If);
        assert_eq!(tokens[2].token, Token::Else);
        assert_eq!(tokens[3].token, Token::Return);
        assert_eq!(tokens[4].token, Token::Bool(true));
        assert_eq!(tokens[5].token, Token::Bool(false));
        assert_eq!(tokens[6].token, Token::Null);
    }

    #[test]
    fn test_hex_number() {
        let mut lexer = Lexer::new("0xFF");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::Number(255.0));
    }

    #[test]
    fn test_comments() {
        let mut lexer = Lexer::new("a // comment\nb /* block */ c");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::Identifier("a".into()));
        assert_eq!(tokens[1].token, Token::Identifier("b".into()));
        assert_eq!(tokens[2].token, Token::Identifier("c".into()));
    }

    #[test]
    fn test_function_expression() {
        let mut lexer = Lexer::new("function add(a, b) { return a + b; }");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::Function);
        assert_eq!(tokens[1].token, Token::Identifier("add".into()));
        assert_eq!(tokens[2].token, Token::LeftParen);
        assert_eq!(tokens[3].token, Token::Identifier("a".into()));
        assert_eq!(tokens[4].token, Token::Comma);
        assert_eq!(tokens[5].token, Token::Identifier("b".into()));
        assert_eq!(tokens[6].token, Token::RightParen);
        assert_eq!(tokens[7].token, Token::LeftBrace);
        assert_eq!(tokens[8].token, Token::Return);
    }

    #[test]
    fn test_escape_sequences() {
        let mut lexer = Lexer::new(r#""hello\nworld""#);
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::String("hello\nworld".into()));
    }

    #[test]
    fn test_arrow_function() {
        let mut lexer = Lexer::new("(x) => x * 2");
        let tokens = lexer.tokenize();
        assert_eq!(tokens[0].token, Token::LeftParen);
        assert_eq!(tokens[1].token, Token::Identifier("x".into()));
        assert_eq!(tokens[2].token, Token::RightParen);
        assert_eq!(tokens[3].token, Token::Arrow);
        assert_eq!(tokens[4].token, Token::Identifier("x".into()));
        assert_eq!(tokens[5].token, Token::Star);
        assert_eq!(tokens[6].token, Token::Number(2.0));
    }
}
