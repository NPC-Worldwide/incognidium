use murkiu_lexer::{Lexer, SpannedToken, Token};

/// AST node types for JavaScript.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Number literal
    Number(f64),
    /// String literal
    Str(String),
    /// Boolean literal
    Bool(bool),
    /// null
    Null,
    /// undefined
    Undefined,
    /// Identifier reference
    Ident(String),
    /// this
    This,
    /// Binary operation: left op right
    Binary(Box<Expr>, BinOp, Box<Expr>),
    /// Unary operation: op expr
    Unary(UnaryOp, Box<Expr>),
    /// Postfix: expr op
    Postfix(Box<Expr>, UnaryOp),
    /// Assignment: target = value
    Assign(Box<Expr>, AssignOp, Box<Expr>),
    /// Member access: obj.prop
    Member(Box<Expr>, String),
    /// Computed member: obj[expr]
    Index(Box<Expr>, Box<Expr>),
    /// Function call: callee(args)
    Call(Box<Expr>, Vec<Expr>),
    /// new Constructor(args)
    New(Box<Expr>, Vec<Expr>),
    /// Array literal: [a, b, c]
    Array(Vec<Expr>),
    /// Object literal: { key: value, ... }
    Object(Vec<(PropKey, Expr)>),
    /// Function expression: function(params) { body }
    FunctionExpr {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    /// Arrow function: (params) => expr or { body }
    Arrow {
        params: Vec<String>,
        body: ArrowBody,
    },
    /// Conditional: cond ? then : else
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// typeof expr
    Typeof(Box<Expr>),
    /// void expr
    VoidExpr(Box<Expr>),
    /// delete expr
    DeleteExpr(Box<Expr>),
    /// Template literal
    Template(String),
    /// Logical: left && right, left || right
    Logical(Box<Expr>, LogicalOp, Box<Expr>),
    /// Comma expression: a, b
    Sequence(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropKey {
    Ident(String),
    Str(String),
    Number(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod, Pow,
    Eq, Ne, StrictEq, StrictNe,
    Lt, Gt, Le, Ge,
    BitAnd, BitOr, BitXor,
    Shl, Shr, UShr,
    Instanceof, In,
    NullishCoalesce,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg, Pos, Not, BitNot,
    Inc, Dec,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogicalOp {
    And, Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Assign, AddAssign, SubAssign, MulAssign, DivAssign, ModAssign,
}

/// Statement AST nodes.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Variable declaration: var/let/const name = expr
    VarDecl {
        kind: VarKind,
        declarations: Vec<(String, Option<Expr>)>,
    },
    /// Expression statement
    Expr(Expr),
    /// Block: { stmts }
    Block(Vec<Stmt>),
    /// Return statement
    Return(Option<Expr>),
    /// If statement
    If {
        condition: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
    },
    /// While loop
    While {
        condition: Expr,
        body: Box<Stmt>,
    },
    /// For loop
    For {
        init: Option<Box<Stmt>>,
        condition: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    /// Do-while loop
    DoWhile {
        body: Box<Stmt>,
        condition: Expr,
    },
    /// Function declaration
    FunctionDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    /// Break
    Break,
    /// Continue
    Continue,
    /// Throw
    Throw(Expr),
    /// Try-catch-finally
    TryCatch {
        try_block: Vec<Stmt>,
        catch_param: Option<String>,
        catch_block: Option<Vec<Stmt>>,
        finally_block: Option<Vec<Stmt>>,
    },
    /// Switch
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
    },
    /// Empty statement (;)
    Empty,
    /// Debugger
    Debugger,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub test: Option<Expr>, // None = default
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

/// A parsed JavaScript program.
#[derive(Debug, Clone)]
pub struct Program {
    pub body: Vec<Stmt>,
}

/// Parse a JavaScript source string into an AST.
pub fn parse(source: &str) -> Result<Program, String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<SpannedToken>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].token
        } else {
            &Token::Eof
        }
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos].token;
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("Expected {:?}, got {:?}", expected, self.peek()))
        }
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn parse_program(&mut self) -> Result<Program, String> {
        let mut body = Vec::new();
        while *self.peek() != Token::Eof {
            body.push(self.parse_statement()?);
        }
        Ok(Program { body })
    }

    fn parse_statement(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Token::Var => self.parse_var_decl(VarKind::Var),
            Token::Let => self.parse_var_decl(VarKind::Let),
            Token::Const => self.parse_var_decl(VarKind::Const),
            Token::Function => self.parse_function_decl(),
            Token::Return => self.parse_return(),
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::For => self.parse_for(),
            Token::Do => self.parse_do_while(),
            Token::LeftBrace => self.parse_block_stmt(),
            Token::Break => { self.advance(); self.eat(&Token::Semicolon); Ok(Stmt::Break) }
            Token::Continue => { self.advance(); self.eat(&Token::Semicolon); Ok(Stmt::Continue) }
            Token::Throw => self.parse_throw(),
            Token::Try => self.parse_try(),
            Token::Switch => self.parse_switch(),
            Token::Semicolon => { self.advance(); Ok(Stmt::Empty) }
            Token::Debugger => { self.advance(); self.eat(&Token::Semicolon); Ok(Stmt::Debugger) }
            _ => self.parse_expr_statement(),
        }
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> Result<Stmt, String> {
        self.advance(); // skip var/let/const
        let mut declarations = Vec::new();
        loop {
            let name = match self.peek().clone() {
                Token::Identifier(n) => { self.advance(); n }
                _ => return Err(format!("Expected identifier, got {:?}", self.peek())),
            };
            let init = if self.eat(&Token::Assign) {
                Some(self.parse_assignment_expr()?)
            } else {
                None
            };
            declarations.push((name, init));
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.eat(&Token::Semicolon);
        Ok(Stmt::VarDecl { kind, declarations })
    }

    fn parse_function_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'function'
        let name = match self.peek().clone() {
            Token::Identifier(n) => { self.advance(); n }
            _ => return Err("Expected function name".into()),
        };
        let params = self.parse_params()?;
        let body = self.parse_block()?;
        Ok(Stmt::FunctionDecl { name, params, body })
    }

    fn parse_params(&mut self) -> Result<Vec<String>, String> {
        self.expect(&Token::LeftParen)?;
        let mut params = Vec::new();
        while *self.peek() != Token::RightParen {
            match self.peek().clone() {
                Token::Identifier(n) => { self.advance(); params.push(n); }
                _ => return Err(format!("Expected parameter name, got {:?}", self.peek())),
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RightParen)?;
        Ok(params)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(&Token::LeftBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != Token::RightBrace && *self.peek() != Token::Eof {
            stmts.push(self.parse_statement()?);
        }
        self.expect(&Token::RightBrace)?;
        Ok(stmts)
    }

    fn parse_block_stmt(&mut self) -> Result<Stmt, String> {
        Ok(Stmt::Block(self.parse_block()?))
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'return'
        if self.eat(&Token::Semicolon) || *self.peek() == Token::RightBrace {
            return Ok(Stmt::Return(None));
        }
        let expr = self.parse_expression()?;
        self.eat(&Token::Semicolon);
        Ok(Stmt::Return(Some(expr)))
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'if'
        self.expect(&Token::LeftParen)?;
        let condition = self.parse_expression()?;
        self.expect(&Token::RightParen)?;
        let then_branch = Box::new(self.parse_statement()?);
        let else_branch = if self.eat(&Token::Else) {
            Some(Box::new(self.parse_statement()?))
        } else {
            None
        };
        Ok(Stmt::If { condition, then_branch, else_branch })
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'while'
        self.expect(&Token::LeftParen)?;
        let condition = self.parse_expression()?;
        self.expect(&Token::RightParen)?;
        let body = Box::new(self.parse_statement()?);
        Ok(Stmt::While { condition, body })
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'for'
        self.expect(&Token::LeftParen)?;

        let init = if self.eat(&Token::Semicolon) {
            None
        } else {
            let s = self.parse_statement()?;
            Some(Box::new(s))
        };

        let condition = if *self.peek() == Token::Semicolon {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.eat(&Token::Semicolon);

        let update = if *self.peek() == Token::RightParen {
            None
        } else {
            Some(self.parse_expression()?)
        };
        self.expect(&Token::RightParen)?;
        let body = Box::new(self.parse_statement()?);
        Ok(Stmt::For { init, condition, update, body })
    }

    fn parse_do_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'do'
        let body = Box::new(self.parse_statement()?);
        self.expect(&Token::While)?;
        self.expect(&Token::LeftParen)?;
        let condition = self.parse_expression()?;
        self.expect(&Token::RightParen)?;
        self.eat(&Token::Semicolon);
        Ok(Stmt::DoWhile { body, condition })
    }

    fn parse_throw(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'throw'
        let expr = self.parse_expression()?;
        self.eat(&Token::Semicolon);
        Ok(Stmt::Throw(expr))
    }

    fn parse_try(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'try'
        let try_block = self.parse_block()?;
        let (catch_param, catch_block) = if self.eat(&Token::Catch) {
            let param = if self.eat(&Token::LeftParen) {
                let p = match self.peek().clone() {
                    Token::Identifier(n) => { self.advance(); Some(n) }
                    _ => None,
                };
                self.expect(&Token::RightParen)?;
                p
            } else {
                None
            };
            let block = self.parse_block()?;
            (param, Some(block))
        } else {
            (None, None)
        };
        let finally_block = if self.eat(&Token::Finally) {
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(Stmt::TryCatch { try_block, catch_param, catch_block, finally_block })
    }

    fn parse_switch(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'switch'
        self.expect(&Token::LeftParen)?;
        let discriminant = self.parse_expression()?;
        self.expect(&Token::RightParen)?;
        self.expect(&Token::LeftBrace)?;
        let mut cases = Vec::new();
        while *self.peek() != Token::RightBrace && *self.peek() != Token::Eof {
            let test = if self.eat(&Token::Case) {
                Some(self.parse_expression()?)
            } else if self.eat(&Token::Default) {
                None
            } else {
                return Err(format!("Expected case or default, got {:?}", self.peek()));
            };
            self.expect(&Token::Colon)?;
            let mut body = Vec::new();
            while !matches!(self.peek(), Token::Case | Token::Default | Token::RightBrace) {
                body.push(self.parse_statement()?);
            }
            cases.push(SwitchCase { test, body });
        }
        self.expect(&Token::RightBrace)?;
        Ok(Stmt::Switch { discriminant, cases })
    }

    fn parse_expr_statement(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expression()?;
        self.eat(&Token::Semicolon);
        Ok(Stmt::Expr(expr))
    }

    // Expression parsing with precedence climbing

    fn parse_expression(&mut self) -> Result<Expr, String> {
        self.parse_assignment_expr()
    }

    fn parse_assignment_expr(&mut self) -> Result<Expr, String> {
        let left = self.parse_ternary()?;

        let op = match self.peek() {
            Token::Assign => AssignOp::Assign,
            Token::PlusAssign => AssignOp::AddAssign,
            Token::MinusAssign => AssignOp::SubAssign,
            Token::StarAssign => AssignOp::MulAssign,
            Token::SlashAssign => AssignOp::DivAssign,
            Token::PercentAssign => AssignOp::ModAssign,
            _ => return Ok(left),
        };
        self.advance();
        let right = self.parse_assignment_expr()?;
        Ok(Expr::Assign(Box::new(left), op, Box::new(right)))
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let cond = self.parse_nullish_coalesce()?;
        if self.eat(&Token::QuestionMark) {
            let then_expr = self.parse_assignment_expr()?;
            self.expect(&Token::Colon)?;
            let else_expr = self.parse_assignment_expr()?;
            Ok(Expr::Ternary(Box::new(cond), Box::new(then_expr), Box::new(else_expr)))
        } else {
            Ok(cond)
        }
    }

    fn parse_nullish_coalesce(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_logical_or()?;
        while self.eat(&Token::NullishCoalesce) {
            let right = self.parse_logical_or()?;
            left = Expr::Binary(Box::new(left), BinOp::NullishCoalesce, Box::new(right));
        }
        Ok(left)
    }

    fn parse_logical_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_logical_and()?;
        while self.eat(&Token::Or) {
            let right = self.parse_logical_and()?;
            left = Expr::Logical(Box::new(left), LogicalOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_or()?;
        while self.eat(&Token::And) {
            let right = self.parse_bitwise_or()?;
            left = Expr::Logical(Box::new(left), LogicalOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_xor()?;
        while self.eat(&Token::BitOr) {
            let right = self.parse_bitwise_xor()?;
            left = Expr::Binary(Box::new(left), BinOp::BitOr, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_and()?;
        while self.eat(&Token::BitXor) {
            let right = self.parse_bitwise_and()?;
            left = Expr::Binary(Box::new(left), BinOp::BitXor, Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while self.eat(&Token::BitAnd) {
            let right = self.parse_equality()?;
            left = Expr::Binary(Box::new(left), BinOp::BitAnd, Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Token::Equal => BinOp::Eq,
                Token::NotEqual => BinOp::Ne,
                Token::StrictEqual => BinOp::StrictEq,
                Token::StrictNotEqual => BinOp::StrictNe,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match self.peek() {
                Token::Less => BinOp::Lt,
                Token::Greater => BinOp::Gt,
                Token::LessEqual => BinOp::Le,
                Token::GreaterEqual => BinOp::Ge,
                Token::Instanceof => BinOp::Instanceof,
                Token::In => BinOp::In,
                _ => break,
            };
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::ShiftLeft => BinOp::Shl,
                Token::ShiftRight => BinOp::Shr,
                Token::UShiftRight => BinOp::UShr,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_exponentiation()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_exponentiation()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn parse_exponentiation(&mut self) -> Result<Expr, String> {
        let base = self.parse_unary()?;
        if self.eat(&Token::StarStar) {
            let exp = self.parse_exponentiation()?; // right-associative
            Ok(Expr::Binary(Box::new(base), BinOp::Pow, Box::new(exp)))
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Neg, Box::new(expr)))
            }
            Token::Plus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Pos, Box::new(expr)))
            }
            Token::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Not, Box::new(expr)))
            }
            Token::BitNot => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::BitNot, Box::new(expr)))
            }
            Token::PlusPlus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Inc, Box::new(expr)))
            }
            Token::MinusMinus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Dec, Box::new(expr)))
            }
            Token::Typeof => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Typeof(Box::new(expr)))
            }
            Token::Void => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::VoidExpr(Box::new(expr)))
            }
            Token::Delete => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::DeleteExpr(Box::new(expr)))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_call_expr()?;
        match self.peek() {
            Token::PlusPlus => {
                self.advance();
                expr = Expr::Postfix(Box::new(expr), UnaryOp::Inc);
            }
            Token::MinusMinus => {
                self.advance();
                expr = Expr::Postfix(Box::new(expr), UnaryOp::Dec);
            }
            _ => {}
        }
        Ok(expr)
    }

    fn parse_call_expr(&mut self) -> Result<Expr, String> {
        let mut expr = if self.eat(&Token::New) {
            let callee = self.parse_primary()?;
            let args = if *self.peek() == Token::LeftParen {
                self.parse_arguments()?
            } else {
                Vec::new()
            };
            Expr::New(Box::new(callee), args)
        } else {
            self.parse_primary()?
        };

        loop {
            match self.peek() {
                Token::LeftParen => {
                    let args = self.parse_arguments()?;
                    expr = Expr::Call(Box::new(expr), args);
                }
                Token::Dot => {
                    self.advance();
                    let prop = match self.peek().clone() {
                        Token::Identifier(n) => { self.advance(); n }
                        _ => return Err(format!("Expected property name, got {:?}", self.peek())),
                    };
                    expr = Expr::Member(Box::new(expr), prop);
                }
                Token::LeftBracket => {
                    self.advance();
                    let index = self.parse_expression()?;
                    self.expect(&Token::RightBracket)?;
                    expr = Expr::Index(Box::new(expr), Box::new(index));
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_arguments(&mut self) -> Result<Vec<Expr>, String> {
        self.expect(&Token::LeftParen)?;
        let mut args = Vec::new();
        while *self.peek() != Token::RightParen && *self.peek() != Token::Eof {
            args.push(self.parse_assignment_expr()?);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RightParen)?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Number(n) => { self.advance(); Ok(Expr::Number(n)) }
            Token::String(s) => { self.advance(); Ok(Expr::Str(s)) }
            Token::TemplateLiteral(s) => { self.advance(); Ok(Expr::Template(s)) }
            Token::Bool(b) => { self.advance(); Ok(Expr::Bool(b)) }
            Token::Null => { self.advance(); Ok(Expr::Null) }
            Token::Undefined => { self.advance(); Ok(Expr::Undefined) }
            Token::This => { self.advance(); Ok(Expr::This) }
            Token::Identifier(_) => {
                // Could be an arrow function: (ident) => ...
                // or just an identifier
                let Token::Identifier(name) = self.advance().clone() else { unreachable!() };
                // Check for arrow: ident => ...
                if *self.peek() == Token::Arrow {
                    self.advance();
                    let body = if *self.peek() == Token::LeftBrace {
                        ArrowBody::Block(self.parse_block()?)
                    } else {
                        ArrowBody::Expr(Box::new(self.parse_assignment_expr()?))
                    };
                    return Ok(Expr::Arrow { params: vec![name], body });
                }
                Ok(Expr::Ident(name))
            }
            Token::LeftParen => {
                self.advance();
                // Could be arrow function params or grouping
                // Try to detect: () => or (a, b) =>
                let save = self.pos;
                if let Ok(params) = self.try_parse_arrow_params() {
                    if *self.peek() == Token::Arrow {
                        self.advance();
                        let body = if *self.peek() == Token::LeftBrace {
                            ArrowBody::Block(self.parse_block()?)
                        } else {
                            ArrowBody::Expr(Box::new(self.parse_assignment_expr()?))
                        };
                        return Ok(Expr::Arrow { params, body });
                    }
                }
                // Reset and parse as grouping
                self.pos = save;
                let expr = self.parse_expression()?;
                self.expect(&Token::RightParen)?;
                Ok(expr)
            }
            Token::LeftBracket => self.parse_array_literal(),
            Token::LeftBrace => self.parse_object_literal(),
            Token::Function => {
                self.advance();
                let name = if let Token::Identifier(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let params = self.parse_params()?;
                let body = self.parse_block()?;
                Ok(Expr::FunctionExpr { name, params, body })
            }
            _ => Err(format!("Unexpected token: {:?}", self.peek())),
        }
    }

    fn try_parse_arrow_params(&mut self) -> Result<Vec<String>, String> {
        // We're already past the '('
        let mut params = Vec::new();
        if *self.peek() == Token::RightParen {
            self.advance();
            return Ok(params);
        }
        loop {
            match self.peek().clone() {
                Token::Identifier(n) => {
                    self.advance();
                    params.push(n);
                }
                _ => return Err("Not arrow params".into()),
            }
            if self.eat(&Token::Comma) {
                continue;
            }
            if *self.peek() == Token::RightParen {
                self.advance();
                return Ok(params);
            }
            return Err("Not arrow params".into());
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expr, String> {
        self.advance(); // skip [
        let mut elements = Vec::new();
        while *self.peek() != Token::RightBracket && *self.peek() != Token::Eof {
            elements.push(self.parse_assignment_expr()?);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RightBracket)?;
        Ok(Expr::Array(elements))
    }

    fn parse_object_literal(&mut self) -> Result<Expr, String> {
        self.advance(); // skip {
        let mut props = Vec::new();
        while *self.peek() != Token::RightBrace && *self.peek() != Token::Eof {
            let key = match self.peek().clone() {
                Token::Identifier(n) => { self.advance(); PropKey::Ident(n) }
                Token::String(s) => { self.advance(); PropKey::Str(s) }
                Token::Number(n) => { self.advance(); PropKey::Number(n) }
                _ => return Err(format!("Expected property key, got {:?}", self.peek())),
            };
            // Shorthand: { x } is same as { x: x }
            if !self.eat(&Token::Colon) {
                if let PropKey::Ident(ref name) = key {
                    props.push((key.clone(), Expr::Ident(name.clone())));
                    if !self.eat(&Token::Comma) { break; }
                    continue;
                }
            }
            let value = self.parse_assignment_expr()?;
            props.push((key, value));
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RightBrace)?;
        Ok(Expr::Object(props))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_var_decl() {
        let prog = parse("var x = 42;").unwrap();
        assert_eq!(prog.body.len(), 1);
        if let Stmt::VarDecl { kind, declarations } = &prog.body[0] {
            assert_eq!(*kind, VarKind::Var);
            assert_eq!(declarations[0].0, "x");
        } else {
            panic!("Expected VarDecl");
        }
    }

    #[test]
    fn test_parse_function() {
        let prog = parse("function add(a, b) { return a + b; }").unwrap();
        if let Stmt::FunctionDecl { name, params, body } = &prog.body[0] {
            assert_eq!(name, "add");
            assert_eq!(params, &["a", "b"]);
            assert_eq!(body.len(), 1);
        } else {
            panic!("Expected FunctionDecl");
        }
    }

    #[test]
    fn test_parse_if_else() {
        let prog = parse("if (x > 0) { y = 1; } else { y = 2; }").unwrap();
        if let Stmt::If { condition, then_branch, else_branch } = &prog.body[0] {
            assert!(else_branch.is_some());
        } else {
            panic!("Expected If");
        }
    }

    #[test]
    fn test_parse_while() {
        let prog = parse("while (i < 10) { i++; }").unwrap();
        assert!(matches!(&prog.body[0], Stmt::While { .. }));
    }

    #[test]
    fn test_parse_for() {
        let prog = parse("for (var i = 0; i < 10; i++) { x += i; }").unwrap();
        assert!(matches!(&prog.body[0], Stmt::For { .. }));
    }

    #[test]
    fn test_parse_binary_expr() {
        let prog = parse("var x = 2 + 3 * 4;").unwrap();
        if let Stmt::VarDecl { declarations, .. } = &prog.body[0] {
            // Should be Add(2, Mul(3, 4)) due to precedence
            if let Some(Expr::Binary(left, BinOp::Add, right)) = &declarations[0].1 {
                assert_eq!(**left, Expr::Number(2.0));
                assert!(matches!(**right, Expr::Binary(_, BinOp::Mul, _)));
            } else {
                panic!("Expected binary expr");
            }
        }
    }

    #[test]
    fn test_parse_call_expr() {
        let prog = parse("console.log('hello');").unwrap();
        if let Stmt::Expr(Expr::Call(callee, args)) = &prog.body[0] {
            assert!(matches!(**callee, Expr::Member(_, _)));
            assert_eq!(args.len(), 1);
        } else {
            panic!("Expected call expr");
        }
    }

    #[test]
    fn test_parse_arrow_function() {
        let prog = parse("var f = (x) => x * 2;").unwrap();
        if let Stmt::VarDecl { declarations, .. } = &prog.body[0] {
            assert!(matches!(declarations[0].1, Some(Expr::Arrow { .. })));
        } else {
            panic!("Expected arrow");
        }
    }

    #[test]
    fn test_parse_object_literal() {
        let prog = parse("var o = { a: 1, b: 2 };").unwrap();
        if let Stmt::VarDecl { declarations, .. } = &prog.body[0] {
            if let Some(Expr::Object(props)) = &declarations[0].1 {
                assert_eq!(props.len(), 2);
            } else {
                panic!("Expected object");
            }
        }
    }

    #[test]
    fn test_parse_array_literal() {
        let prog = parse("var a = [1, 2, 3];").unwrap();
        if let Stmt::VarDecl { declarations, .. } = &prog.body[0] {
            if let Some(Expr::Array(elems)) = &declarations[0].1 {
                assert_eq!(elems.len(), 3);
            } else {
                panic!("Expected array");
            }
        }
    }

    #[test]
    fn test_parse_try_catch() {
        let prog = parse("try { x(); } catch (e) { log(e); } finally { done(); }").unwrap();
        assert!(matches!(&prog.body[0], Stmt::TryCatch { .. }));
    }
}
