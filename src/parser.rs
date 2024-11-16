use crate::lexer::{Lexer, Token};

#[derive(Debug)]
pub enum ASTNode {
    Number(i64),
    Identifier(String),
    BinaryOp(Box<ASTNode>, Token, Box<ASTNode>),
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
        parser
    }

    fn eat(&mut self, token: Token) {
        if self.current_token == token {
            self.current_token = self.lexer.get_next_token();
        } else {
            panic!("Unexpected token: {:?}", self.current_token);
        }
    }

    pub fn parse(&mut self) -> ASTNode {
        self.expr()
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
        let mut node = self.factor();

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
}
