mod interpreter;
mod lexer;
mod parser;

use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;

fn main() {
    let input = "3 + 5 * (10 - 4)";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);

    let ast = parser.parse();
    println!("AST: {:?}", ast);

    let interpreter = Interpreter::new();
    let result = interpreter.interpret(ast);
    println!("Result: {}", result);
}
