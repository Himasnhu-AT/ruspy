/// Parser module for the Ruspy language
///
/// This module is responsible for parsing tokens from the lexer into an Abstract Syntax Tree (AST).
/// It implements a recursive descent parser that handles variable declarations, assignments,
/// control flow, and expressions.
use crate::lexer::{Lexer, Token};
use crate::types::RuspyType;
use log::{debug, error};
use serde::{Deserialize, Serialize};

/// Represents nodes in the Abstract Syntax Tree (AST)
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub enum ASTNode {
    /// Represents a numeric literal value
    Number(i64),
    /// Represents a float literal value
    FloatLiteral(f64),
    /// Represents a string literal value
    StringLiteral(String),
    /// Represents a variable reference
    Identifier(String),
    /// Represents a binary operation (e.g., addition, multiplication, comparison)
    BinaryOp(Box<ASTNode>, Token, Box<ASTNode>),
    /// Represents an untyped variable assignment
    VarAssign(String, Box<ASTNode>),
    /// Represents a typed variable assignment with type annotation
    TypedVarAssign(String, RuspyType, Box<ASTNode>),
    /// Represents a print statement
    Print(Box<ASTNode>),
    /// Represents a block of statements
    Block(Vec<ASTNode>),
    /// Represents an If-Else statement
    If(Box<ASTNode>, Box<ASTNode>, Option<Box<ASTNode>>),
    /// Represents a function call
    FunctionCall(String, Vec<ASTNode>),
    /// Represents member access (e.g., object.field)
    MemberAccess(Box<ASTNode>, String),
}

/// Parser struct that maintains the state during parsing
pub struct Parser<'a> {
    /// The lexer that provides tokens
    lexer: Lexer<'a>,
    /// The current token being processed
    current_token: Token,
}

impl<'a> Parser<'a> {
    pub fn new(lexer: Lexer<'a>) -> Self {
        let mut parser = Parser {
            lexer,
            current_token: Token::EOF,
        };
        parser.current_token = parser.lexer.get_next_token();
        debug!("Initial token: {:?}", parser.current_token);
        parser
    }

    fn eat(&mut self, expected_token: Token) -> Result<(), String> {
        // We need custom equality check because Token::StringLiteral(String) and others hold data
        let matches = match (&self.current_token, &expected_token) {
            (Token::Identifier(_), Token::Identifier(_)) => true,
            (Token::Number(_), Token::Number(_)) => true,
            (Token::FloatLiteral(_), Token::FloatLiteral(_)) => true,
            (Token::StringLiteral(_), Token::StringLiteral(_)) => true,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        };

        if matches {
            self.current_token = self.lexer.get_next_token();
            Ok(())
        } else {
             let error_msg = format!(
                "Parser error: Expected token {:?}, found {:?}",
                expected_token, self.current_token
            );
            error!("{}", error_msg);
            Err(error_msg)
        }
    }

    pub fn parse(&mut self) -> Result<Vec<ASTNode>, String> {
        let mut statements = Vec::new();
        while self.current_token != Token::EOF {
            let statement = self.statement()?;
            statements.push(statement);
        }
        Ok(statements)
    }

    fn statement(&mut self) -> Result<ASTNode, String> {
        match &self.current_token {
            Token::Print => self.print_statement(),
            Token::If => self.if_statement(),
            Token::LBrace => self.block_statement(),
            Token::Identifier(_) => {
                if self.peek_next() == Some(Token::Colon) {
                    self.variable_declaration_with_type()
                } else if self.peek_next() == Some(Token::Assign) {
                    self.variable_declaration_without_type()
                } else {
                    self.expression_statement()
                }
            },
            _ => self.expression_statement(),
        }
    }

    fn block_statement(&mut self) -> Result<ASTNode, String> {
        self.eat(Token::LBrace)?;
        let mut statements = Vec::new();
        while self.current_token != Token::RBrace && self.current_token != Token::EOF {
            statements.push(self.statement()?);
        }
        self.eat(Token::RBrace)?;
        Ok(ASTNode::Block(statements))
    }

    fn if_statement(&mut self) -> Result<ASTNode, String> {
        self.eat(Token::If)?;
        let condition = self.comparison()?; // Condition
        
        // We expect a block (using braces for now as per plan)
        let then_branch = self.block_statement()?;
        
        let else_branch = if self.current_token == Token::Else {
            self.eat(Token::Else)?;
            Some(Box::new(self.block_statement()?))
        } else {
            None
        };

        Ok(ASTNode::If(Box::new(condition), Box::new(then_branch), else_branch))
    }

    fn variable_declaration_with_type(&mut self) -> Result<ASTNode, String> {
        let name = match &self.current_token {
            Token::Identifier(name) => name.clone(),
            _ => return Err("Expected identifier".to_string()),
        };
        self.eat(Token::Identifier(name.clone()))?;
        self.eat(Token::Colon)?;
        let var_type = self.parse_type()?;
        self.eat(Token::Assign)?;
        let value = self.comparison()?; // Support expressions
        self.eat(Token::Semicolon)?;
        Ok(ASTNode::TypedVarAssign(name, var_type, Box::new(value)))
    }

    fn variable_declaration_without_type(&mut self) -> Result<ASTNode, String> {
        let name = match &self.current_token {
            Token::Identifier(name) => name.clone(),
            _ => return Err("Expected identifier".to_string()),
        };
        self.eat(Token::Identifier(name.clone()))?;
        self.eat(Token::Assign)?;
        let value = self.comparison()?;
        self.eat(Token::Semicolon)?;
        Ok(ASTNode::VarAssign(name, Box::new(value)))
    }

    fn parse_type(&mut self) -> Result<RuspyType, String> {
        match &self.current_token {
            Token::TypeInt => { self.eat(Token::TypeInt)?; Ok(RuspyType::Int(0)) },
            Token::TypeInt32 => { self.eat(Token::TypeInt32)?; Ok(RuspyType::Int32(0)) },
            Token::TypeInt64 => { self.eat(Token::TypeInt64)?; Ok(RuspyType::Int64(0)) },
            Token::TypeFloat => { self.eat(Token::TypeFloat)?; Ok(RuspyType::Float(0.0)) },
            Token::TypeFloat32 => { self.eat(Token::TypeFloat32)?; Ok(RuspyType::Float32(0.0)) },
            Token::TypeFloat64 => { self.eat(Token::TypeFloat64)?; Ok(RuspyType::Float64(0.0)) },
            Token::TypeStr => { self.eat(Token::TypeStr)?; Ok(RuspyType::Str(String::new())) },
            Token::TypeChar => { self.eat(Token::TypeChar)?; Ok(RuspyType::Char('\0')) },
            _ => Err(format!("Invalid type: {:?}", self.current_token)),
        }
    }

    fn peek_next(&self) -> Option<Token> {
        let mut lexer_clone = self.lexer.clone();
        // let current_token_clone = self.current_token.clone(); // unused
        let next_token = lexer_clone.get_next_token();
        Some(next_token)
    }

    fn print_statement(&mut self) -> Result<ASTNode, String> {
        self.eat(Token::Print)?;
        let expr = self.comparison()?;
        self.eat(Token::Semicolon)?;
        Ok(ASTNode::Print(Box::new(expr)))
    }

    fn expression_statement(&mut self) -> Result<ASTNode, String> {
        let expr = self.comparison()?;
        self.eat(Token::Semicolon)?;
        Ok(expr)
    }

    // New wrapper for comparisons
    fn comparison(&mut self) -> Result<ASTNode, String> {
        let mut node = self.expr()?;

        loop {
            match self.current_token {
                Token::Eq | Token::NotEq | Token::Gt | Token::Lt | Token::GtEq | Token::LtEq => {
                    let token = self.current_token.clone();
                    self.eat(token.clone())?;
                    node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.expr()?));
                }
                _ => break,
            }
        }
        Ok(node)
    }

    fn expr(&mut self) -> Result<ASTNode, String> {
        let mut node = self.term()?;

        while matches!(self.current_token, Token::Plus | Token::Minus) {
            let token = self.current_token.clone();
            self.eat(token.clone())?;
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.term()?));
        }
        Ok(node)
    }

    fn term(&mut self) -> Result<ASTNode, String> {
        let mut node = self.factor()?;

        while matches!(self.current_token, Token::Asterisk | Token::Slash) {
            let token = self.current_token.clone();
            self.eat(token.clone())?;
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.factor()?));
        }
        Ok(node)
    }

    fn factor(&mut self) -> Result<ASTNode, String> {
        let mut node = match &self.current_token {
            Token::Number(value) => {
                let val = *value;
                self.eat(Token::Number(val))?;
                ASTNode::Number(val)
            },
            Token::FloatLiteral(value) => {
                let val = *value;
                self.eat(Token::FloatLiteral(val))?;
                ASTNode::FloatLiteral(val)
            },
            Token::StringLiteral(text) => {
                let t = text.clone();
                self.eat(Token::StringLiteral(t.clone()))?;
                ASTNode::StringLiteral(t)
            },
            Token::Identifier(name) => {
                let n = name.clone();
                self.eat(Token::Identifier(n.clone()))?;
                ASTNode::Identifier(n)
            },
            Token::LParen => {
                self.eat(Token::LParen)?;
                let n = self.comparison()?;
                self.eat(Token::RParen)?;
                n
            },
            _ => return Err(format!("Unexpected token in factor: {:?}", self.current_token)),
        };

        // Handle postfix operators (Function call, Member access)
        loop {
            if self.current_token == Token::LParen {
                // Function call: identifier(args)
                // We should only allow function calls on identifiers or maybe member access
                match node {
                    ASTNode::Identifier(func_name) => {
                        self.eat(Token::LParen)?;
                        let mut args = Vec::new();
                        if self.current_token != Token::RParen {
                            args.push(self.comparison()?);
                            while self.current_token == Token::Comma {
                                self.eat(Token::Comma)?;
                                args.push(self.comparison()?);
                            }
                        }
                        self.eat(Token::RParen)?;
                        node = ASTNode::FunctionCall(func_name, args);
                    },
                    ASTNode::MemberAccess(_, _) => {
                         // support obj.method() later maybe, for now let's assume member access implies property or method call is different
                         // For simplicity: FunctionCall is only free functions for now.
                         // But if we have `obj.method()`, parser sees `(obj . method)` then `(`
                         // The structure `MemberAccess(obj, method)` is created.
                         // We could wrap it in Call?
                         // Let's keep it simple: `buy(...)` is `FunctionCall`.
                         // `tick.bid` is `MemberAccess`.
                         return Err("Method calls not fully supported in this parser version yet".to_string());
                    }
                    _ => return Err("Expected identifier before function call".to_string()),
                }
            } else if self.current_token == Token::Dot {
                // Member access
                self.eat(Token::Dot)?;
                if let Token::Identifier(member) = &self.current_token {
                     let m = member.clone();
                     self.eat(Token::Identifier(m.clone()))?;
                     node = ASTNode::MemberAccess(Box::new(node), m);
                } else {
                    return Err(format!("Expected identifier after dot, found: {:?}", self.current_token));
                }
            } else {
                break;
            }
        }

        Ok(node)
    }
}
