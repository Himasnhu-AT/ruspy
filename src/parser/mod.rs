/// Parser module for the Ruspy language
///
/// This module is responsible for parsing tokens from the lexer into an Abstract Syntax Tree (AST).
/// It implements a recursive descent parser that handles variable declarations, assignments,
/// and arithmetic expressions.
use crate::lexer::{Lexer, Token};
use crate::types::RuspyType;
use log::{debug, error};

/// Represents nodes in the Abstract Syntax Tree (AST)
///
/// Each variant represents a different kind of program construct that can appear
/// in the source code.
#[derive(Debug, PartialEq)]
pub enum ASTNode {
    /// Represents a numeric literal value
    Number(i64),
    /// Represents a string literal value
    StringLiteral(String),
    /// Represents a variable reference
    Identifier(String),
    /// Represents a binary operation (e.g., addition, multiplication)
    BinaryOp(Box<ASTNode>, Token, Box<ASTNode>),
    /// Represents an untyped variable assignment
    VarAssign(String, Box<ASTNode>),
    /// Represents a typed variable assignment with type annotation
    TypedVarAssign(String, RuspyType, Box<ASTNode>),
    /// Represents a print statement
    Print(Box<ASTNode>),
}

/// Parser struct that maintains the state during parsing
pub struct Parser<'a> {
    /// The lexer that provides tokens
    lexer: Lexer<'a>,
    /// The current token being processed
    current_token: Token,
}

impl<'a> Parser<'a> {
    /// Creates a new Parser instance
    ///
    /// # Arguments
    /// * `lexer` - The lexer that will provide tokens to parse
    ///
    /// # Returns
    /// * A new Parser instance initialized with the first token
    pub fn new(lexer: Lexer<'a>) -> Self {
        let mut parser = Parser {
            lexer,
            current_token: Token::EOF,
        };
        parser.current_token = parser.lexer.get_next_token();
        debug!("Initial token: {:?}", parser.current_token);
        parser
    }

    /// Consumes the current token if it matches the expected token
    ///
    /// # Arguments
    /// * `expected_token` - The token type that is expected at this point
    ///
    /// # Returns
    /// * Result<(), String> - Ok if token is consumed, Err with message if not
    fn eat(&mut self, expected_token: Token) -> Result<(), String> {
        debug!("Attempting to eat token: {:?}", expected_token);
        debug!("Current token: {:?}", self.current_token);

        if std::mem::discriminant(&self.current_token) == std::mem::discriminant(&expected_token) {
            self.current_token = self.lexer.get_next_token();
            debug!("Token eaten successfully, next token: {:?}", self.current_token);
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

    /// Parses the input stream into a vector of AST nodes
    ///
    /// This is the main entry point for parsing. It processes the input
    /// until it reaches EOF, handling variable declarations, assignments,
    /// and expressions.
    ///
    /// # Returns
    /// * `Result<Vec<ASTNode>, String>` - Either a vector of AST nodes or an error message
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

    fn variable_declaration_with_type(&mut self) -> Result<ASTNode, String> {
        // Get variable name
        let name = match &self.current_token {
            Token::Identifier(name) => name.clone(),
            _ => return Err("Expected identifier".to_string()),
        };
        self.eat(Token::Identifier(name.clone()))?;
        
        // Expect colon
        self.eat(Token::Colon)?;
        
        // Get type
        let var_type = self.parse_type()?;
        
        // Expect assignment
        self.eat(Token::Assign)?;
        
        // Get expression value
        let value = self.expr()?;
        
        // Expect semicolon
        self.eat(Token::Semicolon)?;
        
        Ok(ASTNode::TypedVarAssign(name, var_type, Box::new(value)))
    }

    fn variable_declaration_without_type(&mut self) -> Result<ASTNode, String> {
        // Get variable name
        let name = match &self.current_token {
            Token::Identifier(name) => name.clone(),
            _ => return Err("Expected identifier".to_string()),
        };
        self.eat(Token::Identifier(name.clone()))?;
        
        // Expect assignment
        self.eat(Token::Assign)?;
        
        // Get expression value
        let value = self.expr()?;
        
        // Expect semicolon
        self.eat(Token::Semicolon)?;
        
        Ok(ASTNode::VarAssign(name, Box::new(value)))
    }

    fn parse_type(&mut self) -> Result<RuspyType, String> {
        match &self.current_token {
            Token::TypeInt => {
                self.eat(Token::TypeInt)?;
                Ok(RuspyType::Int(0))
            },
            Token::TypeInt32 => {
                self.eat(Token::TypeInt32)?;
                Ok(RuspyType::Int32(0))
            },
            Token::TypeInt64 => {
                self.eat(Token::TypeInt64)?;
                Ok(RuspyType::Int64(0))
            },
            Token::TypeFloat => {
                self.eat(Token::TypeFloat)?;
                Ok(RuspyType::Float(0.0))
            },
            Token::TypeFloat32 => {
                self.eat(Token::TypeFloat32)?;
                Ok(RuspyType::Float32(0.0))
            },
            Token::TypeFloat64 => {
                self.eat(Token::TypeFloat64)?;
                Ok(RuspyType::Float64(0.0))
            },
            Token::TypeStr => {
                self.eat(Token::TypeStr)?;
                Ok(RuspyType::Str(String::new()))
            },
            Token::TypeChar => {
                self.eat(Token::TypeChar)?;
                Ok(RuspyType::Char('\0'))
            },
            _ => Err(format!("Invalid type: {:?}", self.current_token)),
        }
    }

    // Helper method to peek at the next token without consuming it
    fn peek_next(&self) -> Option<Token> {
        let mut lexer_clone = self.lexer.clone();
        let current_token_clone = self.current_token.clone();
        
        // Advance and get the next token
        let next_token = lexer_clone.get_next_token();
        
        Some(next_token)
    }

    fn print_statement(&mut self) -> Result<ASTNode, String> {
        self.eat(Token::Print)?;
        let expr = self.expr()?;
        self.eat(Token::Semicolon)?;
        Ok(ASTNode::Print(Box::new(expr)))
    }

    fn expression_statement(&mut self) -> Result<ASTNode, String> {
        let expr = self.expr()?;
        self.eat(Token::Semicolon)?;
        Ok(expr)
    }

    /// Parses an expression
    ///
    /// Handles addition and subtraction operations, delegating to term()
    /// for higher precedence operations.
    ///
    /// # Returns
    /// * `Result<ASTNode, String>` - The parsed expression or an error
    fn expr(&mut self) -> Result<ASTNode, String> {
        let mut node = self.term()?;

        while matches!(self.current_token, Token::Plus | Token::Minus) {
            let token = self.current_token.clone();
            self.eat(token.clone())?;
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.term()?));
        }

        Ok(node)
    }

    /// Parses a term
    ///
    /// Handles multiplication and division operations, delegating to factor()
    /// for higher precedence operations.
    ///
    /// # Returns
    /// * `Result<ASTNode, String>` - The parsed term or an error
    fn term(&mut self) -> Result<ASTNode, String> {
        let mut node = self.factor()?;

        while matches!(self.current_token, Token::Asterisk | Token::Slash) {
            let token = self.current_token.clone();
            self.eat(token.clone())?;
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.factor()?));
        }

        Ok(node)
    }

    /// Parses a factor
    ///
    /// Handles the highest precedence operations: numbers, identifiers,
    /// and parenthesized expressions.
    ///
    /// # Returns
    /// * `Result<ASTNode, String>` - The parsed factor or an error
    fn factor(&mut self) -> Result<ASTNode, String> {
        match &self.current_token {
            Token::Number(value) => {
                let value = *value;
                self.eat(Token::Number(value))?;
                Ok(ASTNode::Number(value))
            },
            Token::StringLiteral(text) => {
                let text = text.clone();
                self.eat(Token::StringLiteral(text.clone()))?;
                Ok(ASTNode::StringLiteral(text))
            },
            Token::Identifier(ref name) => {
                let name = name.clone();
                self.eat(Token::Identifier(name.clone()))?;
                Ok(ASTNode::Identifier(name))
            },
            Token::LParen => {
                self.eat(Token::LParen)?;
                let node = self.expr()?;
                self.eat(Token::RParen)?;
                Ok(node)
            },
            _ => Err(format!("Unexpected token: {:?}", self.current_token)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arithmetic_expression() {
        let input = "3 + 5 * 2;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let result = parser.parse().unwrap();
        assert!(matches!(result[0], ASTNode::BinaryOp(..)));
    }

    #[test]
    fn test_typed_variable_declaration() {
        let input = "x: int = 42;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let result = parser.parse().unwrap();
        assert_eq!(
            result[0],
            ASTNode::TypedVarAssign(
                "x".to_string(),
                RuspyType::Int(0),
                Box::new(ASTNode::Number(42))
            )
        );
    }

    #[test]
    fn test_invalid_type_declaration() {
        let input = "x: invalid = 42;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        assert!(parser.parse().is_err());
    }

    #[test]
    fn test_complex_expression() {
        let input = "(3 + 5) * 2;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let result = parser.parse().unwrap();
        assert!(matches!(result[0], ASTNode::BinaryOp(..)));
    }

    #[test]
    fn test_multiple_statements() {
        let input = "
            x: int = 42;
            y: int = x + 8;
        ";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let result = parser.parse().unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_missing_semicolon() {
        let input = "x: int = 42";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let result = parser.parse().unwrap();
        assert_eq!(result.len(), 1);
    }
}
