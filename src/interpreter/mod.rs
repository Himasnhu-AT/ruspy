/// The Interpreter module handles the execution of the Abstract Syntax Tree (AST)
/// and maintains the state of variables during program execution.
use crate::lexer::Token;
use crate::parser::ASTNode;
use crate::types::RuspyType;
use std::collections::HashMap;

/// Represents the interpreter state and execution environment
///
/// # Fields
/// * `variables` - A HashMap storing variable names and their corresponding values
pub struct Interpreter {
    variables: HashMap<String, RuspyType>,
}

impl Interpreter {
    /// Creates a new instance of the Interpreter with an empty variable store
    ///
    /// # Returns
    /// * A new Interpreter instance
    pub fn new() -> Self {
        Interpreter {
            variables: HashMap::new(),
        }
    }

    /// Interprets a vector of AST nodes and returns the result of the last expression
    ///
    /// # Arguments
    /// * `nodes` - Vector of AST nodes to interpret
    ///
    /// # Returns
    /// * The result of the last evaluated expression
    pub fn interpret(&mut self, nodes: Vec<ASTNode>) -> Result<RuspyType, String> {
        let mut last_result = Ok(RuspyType::Int(0));
        for node in nodes {
            match self.interpret_node(node) {
                Ok(result) => last_result = Ok(result),
                Err(e) => return Err(e),
            }
        }
        last_result
    }

    /// Interprets a single AST node
    ///
    /// # Arguments
    /// * `node` - The AST node to interpret
    ///
    /// # Returns
    /// * The result of evaluating the node
    ///
    /// # Panics
    /// * When encountering undefined variables
    /// * When encountering unexpected operators
    fn interpret_node(&mut self, node: ASTNode) -> Result<RuspyType, String> {
        match node {
            // Handle literal numbers
            ASTNode::Number(value) => Ok(RuspyType::Int64(value)),

            // Handle variable assignment without type annotation
            ASTNode::VarAssign(name, expr) => {
                let value = self.interpret_node(*expr)?;
                self.variables.insert(name, value.clone());
                Ok(value)
            }

            // Handle typed variable assignment
            ASTNode::TypedVarAssign(name, declared_type, expr) => {
                let value = self.interpret_node(*expr)?;
                match declared_type {
                    RuspyType::Str(_) => {
                        if matches!(
                            value,
                            RuspyType::Int(_)
                                | RuspyType::Int32(_)
                                | RuspyType::Int64(_)
                                | RuspyType::Float(_)
                        ) {
                            return Err(format!(
                                "Cannot assign numeric result to string variable '{}'",
                                name
                            ));
                        }
                    }
                    _ => {}
                }
                self.check_type_compatibility(&declared_type, &value)?;
                self.variables.insert(name, value.clone());
                Ok(value)
            }

            // Handle variable references
            ASTNode::Identifier(name) => {
                let value = self.variables.get(&name).cloned();
                value.ok_or_else(|| format!("Undefined variable: {}", name))
            }

            // Handle binary operations
            ASTNode::BinaryOp(left, op, right) => {
                let left_val = self.interpret_node(*left)?;
                let right_val = self.interpret_node(*right)?;
                match op {
                    Token::Plus => Ok(left_val + right_val),
                    Token::Minus => Ok(left_val - right_val),
                    Token::Asterisk => Ok(left_val * right_val),
                    Token::Slash => Ok(left_val / right_val),
                    _ => Err(format!("Unexpected operator in binary operation")),
                }
            }
        }
    }

    fn check_type_compatibility(
        &self,
        var_type: &RuspyType,
        value: &RuspyType,
    ) -> Result<(), String> {
        if !var_type.is_compatible_with(value) {
            return Err(format!(
                "Type mismatch: Cannot assign {:?} to {:?}",
                value, var_type
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn test_simple_arithmetic() {
        let input = "3 + 5;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert_eq!(interpreter.interpret(ast), Ok(RuspyType::Int64(8)));
    }

    #[test]
    fn test_variables() {
        let input = "
            x: int64 = 10;
            y: int64 = 5;
            z = x + y;
            z;
        ";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert_eq!(interpreter.interpret(ast), Ok(RuspyType::Int64(15)));
    }

    #[test]
    fn test_complex_expression() {
        let input = "3 + 5 * (10 - 4);";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert_eq!(interpreter.interpret(ast), Ok(RuspyType::Int64(33)));
    }

    #[test]
    fn test_typed_variables() {
        let input = "
            x: int64 = 42;
            y: int64 = x * 2;
            y;
        ";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert_eq!(interpreter.interpret(ast), Ok(RuspyType::Int64(84)));
    }

    #[test]
    fn test_type_mismatch() {
        let input = "x: int = 42;
                    y: float = x;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert!(interpreter.interpret(ast).is_err());
    }

    #[test]
    fn test_undefined_variable() {
        let input = "a: int64 = b + c;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let ast = parser.parse().unwrap();
        let mut interpreter = Interpreter::new();
        assert!(interpreter.interpret(ast).is_err());
    }
}
