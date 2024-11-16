use crate::lexer::Token;
use crate::parser::ASTNode;

pub struct Interpreter;

impl Interpreter {
    pub fn new() -> Self {
        Interpreter
    }

    pub fn interpret(&self, node: ASTNode) -> i64 {
        match node {
            ASTNode::Number(value) => value,
            ASTNode::BinaryOp(left, op, right) => {
                let left_val = self.interpret(*left);
                let right_val = self.interpret(*right);
                match op {
                    Token::Plus => left_val + right_val,
                    Token::Minus => left_val - right_val,
                    Token::Asterisk => left_val * right_val,
                    Token::Slash => left_val / right_val,
                    _ => panic!("Unexpected operator: {:?}", op),
                }
            }
            _ => panic!("Unexpected AST node: {:?}", node),
        }
    }
}
