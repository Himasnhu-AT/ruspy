/// The Interpreter module handles the execution of the Abstract Syntax Tree (AST)
/// and maintains the state of variables during program execution.
use crate::lexer::Token;
use crate::parser::ASTNode;
use crate::types::RuspyType;
use std::collections::HashMap;
use log::info;

/// Trait to allow the interpreter to interact with the host environment
pub trait RuspyHost {
    fn call_function(&mut self, name: &str, args: Vec<RuspyType>) -> Result<RuspyType, String>;
    fn get_member(&mut self, obj_id: &str, member: &str) -> Result<RuspyType, String>;
}

/// A default host implementation that provides no external functionality
pub struct DefaultHost;
impl RuspyHost for DefaultHost {
    fn call_function(&mut self, name: &str, _args: Vec<RuspyType>) -> Result<RuspyType, String> {
        Err(format!("Function '{}' not found", name))
    }
    fn get_member(&mut self, obj_id: &str, member: &str) -> Result<RuspyType, String> {
        Err(format!("Member '{}' of object '{}' not found", member, obj_id))
    }
}

/// Represents the interpreter state and execution environment
pub struct Interpreter {
    variables: HashMap<String, RuspyType>,
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            variables: HashMap::new(),
        }
    }

    /// Set a variable from outside (e.g. "tick")
    pub fn set_variable(&mut self, name: String, value: RuspyType) {
        self.variables.insert(name, value);
    }

    pub fn interpret(&mut self, nodes: Vec<ASTNode>, host: &mut dyn RuspyHost) -> Result<RuspyType, String> {
        let mut last_result = Ok(RuspyType::Void);
        for node in nodes {
            match self.interpret_node(node, host) {
                Ok(result) => last_result = Ok(result),
                Err(e) => return Err(e),
            }
        }
        last_result
    }

    fn interpret_node(&mut self, node: ASTNode, host: &mut dyn RuspyHost) -> Result<RuspyType, String> {
        match node {
            ASTNode::Number(value) => Ok(RuspyType::Int64(value)),
            ASTNode::FloatLiteral(value) => Ok(RuspyType::Float64(value)),
            ASTNode::StringLiteral(value) => Ok(RuspyType::Str(value)),
            ASTNode::Identifier(name) => {
                let value = self.variables.get(&name).cloned();
                value.ok_or_else(|| format!("Undefined variable: {}", name))
            },
            ASTNode::VarAssign(name, expr) => {
                let value = self.interpret_node(*expr, host)?;
                self.variables.insert(name, value.clone());
                Ok(value)
            },
            ASTNode::TypedVarAssign(name, _declared_type, expr) => {
                let value = self.interpret_node(*expr, host)?;
                // Type check could be improved
                self.variables.insert(name, value.clone());
                Ok(value)
            },
            ASTNode::BinaryOp(left, op, right) => {
                let left_val = self.interpret_node(*left, host)?;
                let right_val = self.interpret_node(*right, host)?;
                self.evaluate_binary_op(left_val, op, right_val)
            },
            ASTNode::Print(expr) => {
                let value = self.interpret_node(*expr, host)?;
                info!("Output: {}", self.format_value(&value));
                Ok(value)
            },
            ASTNode::Block(statements) => {
                let mut result = RuspyType::Void;
                for stmt in statements {
                    result = self.interpret_node(stmt, host)?;
                }
                Ok(result)
            },
            ASTNode::If(cond, then_branch, else_branch) => {
                let cond_val = self.interpret_node(*cond, host)?;
                match cond_val {
                    RuspyType::Bool(true) => self.interpret_node(*then_branch, host),
                    RuspyType::Bool(false) => {
                        if let Some(else_branch) = else_branch {
                            self.interpret_node(*else_branch, host)
                        } else {
                            Ok(RuspyType::Void)
                        }
                    },
                    _ => Err("Condition must evaluate to boolean".to_string()),
                }
            },
            ASTNode::FunctionCall(name, args) => {
                let mut evaluated_args = Vec::new();
                for arg in args {
                    evaluated_args.push(self.interpret_node(arg, host)?);
                }
                host.call_function(&name, evaluated_args)
            },
            ASTNode::MemberAccess(obj_expr, member) => {
                let obj_val = self.interpret_node(*obj_expr, host)?;
                match obj_val {
                    RuspyType::Object(id) => host.get_member(&id, &member),
                    _ => Err("Cannot access member of non-object type".to_string()),
                }
            },
        }
    }

    fn evaluate_binary_op(&self, left: RuspyType, op: Token, right: RuspyType) -> Result<RuspyType, String> {
        // Delegate arithmetic to types
         match op {
            Token::Plus | Token::Minus | Token::Asterisk | Token::Slash => {
                // We'll rely on operator trait impls in types mod, or do it here?
                // types/mod.rs impls Add etc, but they panic or handle it.
                // Let's use the Add trait if we can, or just keep it simple
                match op {
                    Token::Plus => Ok(left + right),
                    Token::Minus => Ok(left - right),
                    Token::Asterisk => Ok(left * right),
                    Token::Slash => Ok(left / right),
                    _ => unreachable!(),
                }
            },
            // Comparisons
            Token::Eq => Ok(RuspyType::Bool(left == right)),
            Token::NotEq => Ok(RuspyType::Bool(left != right)),
            Token::Gt => self.compare_values(left, right, |l, r| l > r),
            Token::Lt => self.compare_values(left, right, |l, r| l < r),
            Token::GtEq => self.compare_values(left, right, |l, r| l >= r),
            Token::LtEq => self.compare_values(left, right, |l, r| l <= r),
            _ => Err("Unknown operator".to_string()),
        }
    }

    fn compare_values<F>(&self, left: RuspyType, right: RuspyType, cmp: F) -> Result<RuspyType, String> 
    where F: Fn(f64, f64) -> bool {
        // Simplify to f64 for comparison for now
        let l = self.to_f64(&left)?;
        let r = self.to_f64(&right)?;
        Ok(RuspyType::Bool(cmp(l, r)))
    }

    fn to_f64(&self, val: &RuspyType) -> Result<f64, String> {
        match val {
            RuspyType::Int(v) => Ok(*v as f64),
            RuspyType::Int32(v) => Ok(*v as f64),
            RuspyType::Int64(v) => Ok(*v as f64),
            RuspyType::Float(v) => Ok(*v),
            RuspyType::Float32(v) => Ok(*v as f64),
            RuspyType::Float64(v) => Ok(*v),
            _ => Err(format!("Cannot convert {:?} to number for comparison", val)),
        }
    }
    
    fn format_value(&self, value: &RuspyType) -> String {
        match value {
            RuspyType::Int(n) => n.to_string(),
            RuspyType::Int32(n) => n.to_string(),
            RuspyType::Int64(n) => n.to_string(),
            RuspyType::Float(n) => n.to_string(),
            RuspyType::Float32(n) => n.to_string(),
            RuspyType::Float64(n) => n.to_string(),
            RuspyType::Str(s) => s.clone(),
            RuspyType::Char(c) => c.to_string(),
            RuspyType::Bool(b) => b.to_string(),
            RuspyType::Object(id) => format!("<Object {}>", id),
            RuspyType::Void => "void".to_string(),
        }
    }
}
