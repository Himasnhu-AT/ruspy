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
    /// Represents a variable reference
    Identifier(String),
    /// Represents a binary operation (e.g., addition, multiplication)
    BinaryOp(Box<ASTNode>, Token, Box<ASTNode>),
    /// Represents an untyped variable assignment
    VarAssign(String, Box<ASTNode>),
    /// Represents a typed variable assignment with type annotation
    TypedVarAssign(String, RuspyType, Box<ASTNode>),
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
    /// # Panics
    /// * When the current token doesn't match the expected token
    fn eat(&mut self, expected_token: Token) {
        debug!("Attempting to eat token: {:?}", expected_token);
        debug!("Current token: {:?}", self.current_token);

        if self.current_token == expected_token {
            self.current_token = self.lexer.get_next_token();
            debug!(
                "Token eaten successfully, next token: {:?}",
                self.current_token
            );
        } else {
            error!(
                "Parser error: Expected token {:?}, found {:?}",
                expected_token, self.current_token
            );
            panic!("Unexpected token: {:?}", self.current_token);
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
        let mut nodes = Vec::new();

        while self.current_token != Token::EOF {
            debug!("Processing token: {:?}", self.current_token);

            // Skip any extra semicolons
            while self.current_token == Token::Semicolon {
                debug!("Skipping semicolon");
                self.eat(Token::Semicolon);
            }

            // If we've reached EOF after skipping semicolons, break
            if self.current_token == Token::EOF {
                debug!("Reached EOF, breaking");
                break;
            }

            let node = if let Token::Identifier(_) = self.current_token {
                let current_ident = self.current_token.clone();
                self.eat(current_ident.clone());

                match self.current_token {
                    Token::Assign => {
                        // This is a variable assignment
                        debug!("Parsing variable assignment");
                        self.eat(Token::Assign);
                        let expr = self.expr()?;
                        if let Token::Identifier(name) = current_ident {
                            Ok(ASTNode::VarAssign(name, Box::new(expr)))
                        } else {
                            unreachable!()
                        }
                    }
                    Token::Colon => {
                        // This is a typed variable assignment
                        debug!("Parsing typed variable assignment");
                        self.eat(Token::Colon);
                        let var_type = match self.current_token {
                            Token::TypeInt => RuspyType::Int(0),
                            Token::TypeInt32 => RuspyType::Int32(0),
                            Token::TypeInt64 => RuspyType::Int64(0),
                            Token::TypeFloat => RuspyType::Float(0.0),
                            Token::TypeFloat32 => RuspyType::Float32(0.0),
                            Token::TypeFloat64 => RuspyType::Float64(0.0),
                            Token::TypeChar => RuspyType::Char('\0'),
                            Token::TypeStr => RuspyType::Str(String::new()),
                            Token::TypeStr8 => RuspyType::Str8(String::new()),
                            Token::TypeStr32 => RuspyType::Str32(String::new()),
                            Token::TypeStr64 => RuspyType::Str64(String::new()),
                            _ => return Err("Invalid type annotation".to_string()),
                        };
                        self.eat(self.current_token.clone());
                        self.eat(Token::Assign);
                        let expr = self.expr()?;
                        if let Token::Identifier(name) = current_ident {
                            Ok(ASTNode::TypedVarAssign(name, var_type, Box::new(expr)))
                        } else {
                            unreachable!()
                        }
                    }
                    _ => {
                        // This is a variable reference
                        debug!("Parsing variable reference");
                        if let Token::Identifier(name) = current_ident {
                            Ok(ASTNode::Identifier(name))
                        } else {
                            unreachable!()
                        }
                    }
                }
            } else {
                debug!("Parsing expression");
                self.expr()
            }?;

            nodes.push(node);

            if self.current_token == Token::Semicolon {
                self.eat(Token::Semicolon);
            }
        }

        Ok(nodes)
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
            self.eat(token.clone());
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
            self.eat(token.clone());
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
        match self.current_token {
            Token::Number(value) => {
                self.eat(Token::Number(value));
                Ok(ASTNode::Number(value))
            }
            Token::Identifier(ref name) => {
                let name = name.clone();
                self.eat(Token::Identifier(name.clone()));
                Ok(ASTNode::Identifier(name))
            }
            Token::LParen => {
                self.eat(Token::LParen);
                let node = self.expr()?;
                self.eat(Token::RParen);
                Ok(node)
            }
            _ => Err("Unexpected token: {:?}".to_string()),
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
