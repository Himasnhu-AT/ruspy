mod interpreter;
mod lexer;
mod parser;

use env_logger;
use interpreter::Interpreter;
use lexer::Lexer;
use log::info;
use parser::Parser;

fn main() {
    env_logger::init();
    info!("Starting Ruspy interpreter");

    let input = "
    a = 3 + 5 * (10 - 4);
    b = a * 2;
    b;
    ";
    info!("Processing input: {}", input);

    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);

    let ast = parser.parse();
    info!("Generated AST: {:?}", ast);

    let mut interpreter = Interpreter::new();
    let result = interpreter.interpret(ast);
    info!("Final result: {}", result);
}
