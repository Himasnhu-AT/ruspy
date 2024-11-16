use crate::lexer::{Lexer, Token};
use log::{debug, error, info};

#[derive(Debug, PartialEq)]
pub enum ASTNode {
    Number(i64),
    Identifier(String),
    BinaryOp(Box<ASTNode>, Token, Box<ASTNode>),
    // VarDecl(String, Box<ASTNode>),
    VarAssign(String, Box<ASTNode>),
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
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

    pub fn parse(&mut self) -> Vec<ASTNode> {
        info!("Starting parsing");
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
                // Look ahead to see if this is an assignment or just a reference
                let current_ident = self.current_token.clone();
                self.eat(current_ident.clone());

                if self.current_token == Token::Assign {
                    // This is a variable assignment
                    debug!("Parsing variable assignment");
                    self.eat(Token::Assign);
                    let expr = self.expr();
                    if let Token::Identifier(name) = current_ident {
                        ASTNode::VarAssign(name, Box::new(expr))
                    } else {
                        unreachable!()
                    }
                } else {
                    // This is a variable reference
                    debug!("Parsing variable reference");
                    if let Token::Identifier(name) = current_ident {
                        ASTNode::Identifier(name)
                    } else {
                        unreachable!()
                    }
                }
            } else {
                debug!("Parsing expression");
                self.expr()
            };

            debug!("Parsed node: {:?}", node);
            nodes.push(node);

            // Handle optional semicolon
            if self.current_token == Token::Semicolon {
                self.eat(Token::Semicolon);
            }
        }

        info!("Parsing completed, found {} nodes", nodes.len());
        nodes
    }

    fn expr(&mut self) -> ASTNode {
        let mut node = self.term();

        while matches!(self.current_token, Token::Plus | Token::Minus) {
            let token = self.current_token.clone();
            self.eat(token.clone());
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.term()));
        }

        node
    }

    fn term(&mut self) -> ASTNode {
        let mut node = self.factor(); // Add this line to initialize node

        while matches!(self.current_token, Token::Asterisk | Token::Slash) {
            let token = self.current_token.clone();
            self.eat(token.clone());
            node = ASTNode::BinaryOp(Box::new(node), token, Box::new(self.factor()));
        }

        node
    }

    fn factor(&mut self) -> ASTNode {
        match self.current_token {
            Token::Number(value) => {
                self.eat(Token::Number(value));
                ASTNode::Number(value)
            }
            Token::Identifier(ref name) => {
                let name = name.clone();
                self.eat(Token::Identifier(name.clone()));
                ASTNode::Identifier(name)
            }
            Token::LParen => {
                self.eat(Token::LParen);
                let node = self.expr();
                self.eat(Token::RParen);
                node
            }
            _ => panic!("Unexpected token: {:?}", self.current_token),
        }
    }

    // fn parse_variable(&mut self) -> ASTNode {
    //     let var_name = if let Token::Identifier(name) = &self.current_token {
    //         name.clone()
    //     } else {
    //         panic!("Expected variable name");
    //     };
    //     self.eat(Token::Identifier(var_name.clone()));

    //     self.eat(Token::Assign);

    //     let expr = self.expr();
    //     ASTNode::VarAssign(var_name, Box::new(expr))
    // }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::lexer::Lexer;

//     #[test]
//     fn test_parser_simple_expression() {
//         let lexer = Lexer::new("3 + 5");
//         let mut parser = Parser::new(lexer);
//         let ast = parser.parse();
//         match ast {
//             ASTNode::BinaryOp(left, op, right) => {
//                 assert_eq!(*left, ASTNode::Number(3));
//                 assert_eq!(op, Token::Plus);
//                 assert_eq!(*right, ASTNode::Number(5));
//             }
//             _ => panic!("Unexpected AST structure"),
//         }
//     }

//     #[test]
//     fn test_parser_complex_expression() {
//         let lexer = Lexer::new("3 + 5 * (10 - 4)");
//         let mut parser = Parser::new(lexer);
//         let ast = parser.parse();
//         let original_ast = "BinaryOp(Number(3), Plus, BinaryOp(Number(5), Asterisk, BinaryOp(Number(10), Minus, Number(4))))";
//         assert_eq!(format!("{:?}", ast), original_ast);
//     }
// }
