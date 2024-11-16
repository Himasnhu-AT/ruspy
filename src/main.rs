mod interpreter;
mod lexer;
mod parser;
mod types;

use env_logger;
use interpreter::Interpreter;
use lexer::Lexer;
use log::info;
use parser::Parser;

fn main() -> Result<(), String> {
    env_logger::init();
    info!("Starting Ruspy interpreter");

    let input = "
    abc: int = 10-4;
    a: int = 3 + 5 * abc;
    b: int = a * 2;
    b;
    ";
    info!("Processing input: {}", input);

    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);

    let ast = parser.parse()?;
    println!("Generated AST: {:?}", ast);

    let mut interpreter = Interpreter::new();
    let result = interpreter.interpret(ast)?;
    println!("Final result: {:?}", result);

    Ok(())
}
