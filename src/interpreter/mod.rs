use crate::lexer::Token;
use crate::parser::ASTNode;
use std::collections::HashMap;

pub struct Interpreter {
    variables: HashMap<String, i64>,
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            variables: HashMap::new(),
        }
    }

    pub fn interpret(&mut self, nodes: Vec<ASTNode>) -> i64 {
        let mut last_result = 0;
        for node in nodes {
            last_result = self.interpret_node(node);
        }
        last_result
    }

    fn interpret_node(&mut self, node: ASTNode) -> i64 {
        match node {
            ASTNode::Number(value) => value,
            ASTNode::VarAssign(name, expr) => {
                let value = self.interpret_node(*expr);
                self.variables.insert(name, value);
                value
            }
            ASTNode::Identifier(name) => *self.variables.get(&name).expect("Undefined variable"),
            ASTNode::BinaryOp(left, op, right) => {
                let left_val = self.interpret_node(*left);
                let right_val = self.interpret_node(*right);
                match op {
                    Token::Plus => left_val + right_val,
                    Token::Minus => left_val - right_val,
                    Token::Asterisk => left_val * right_val,
                    Token::Slash => left_val / right_val,
                    _ => panic!("Unexpected operator in binary operation"),
                }
            }
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::lexer::Lexer;
//     use crate::parser::Parser;

//     #[test]
//     fn test_interpreter_simple_expression() {
//         let lexer = Lexer::new("3 + 5");
//         let mut parser = Parser::new(lexer);
//         let ast = parser.parse();
//         let interpreter = Interpreter::new();
//         let result = interpreter.interpret(ast);
//         assert_eq!(result, 8);
//     }

//     #[test]
//     fn test_interpreter_complex_expression() {
//         let lexer = Lexer::new("3 + 5 * (10 - 4)");
//         let mut parser = Parser::new(lexer);
//         let ast = parser.parse();
//         let interpreter = Interpreter::new();
//         let result = interpreter.interpret(ast);
//         assert_eq!(result, 33);
//     }
// }
