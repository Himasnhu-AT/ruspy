mod interpreter;
mod lexer;
mod parser;
mod types;

use clap::{Parser as ClapParser, ArgAction};
use env_logger;
use interpreter::Interpreter;
use lexer::Lexer;
use log::{debug, info, error};
use parser::Parser;
use std::fs;
use std::process;

#[derive(ClapParser)]
#[command(version, about = "A simple interpreter written in Rust")]
struct Cli {
    /// Enable debug mode
    #[arg(short = 'd', long = "debug", action = ArgAction::SetTrue)]
    debug: bool,

    /// Source file to interpret
    file: String,
}

fn main() -> Result<(), String> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Initialize logger with appropriate level
    if cli.debug {
        env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .init();
    } else {
        env_logger::builder()
            .filter_level(log::LevelFilter::Info)
            .init();
    }

    info!("Starting Ruspy interpreter");
    
    // Read the source file
    let source = match fs::read_to_string(&cli.file) {
        Ok(content) => content,
        Err(e) => {
            error!("Error reading file '{}': {}", cli.file, e);
            process::exit(1);
        }
    };

    info!("Processing file: {}", cli.file);

    // Create lexer and generate tokens
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);

    // Parse the source code
    let ast = match parser.parse() {
        Ok(ast) => {
            if cli.debug {
                debug!("Generated AST: {:?}", ast);
            }
            ast
        },
        Err(e) => {
            error!("Parsing error: {}", e);
            process::exit(1);
        }
    };

    // Create interpreter and execute the code
    let mut interpreter = Interpreter::new();
    match interpreter.interpret(ast) {
        Ok(result) => {
            info!("Execution completed successfully");
            info!("Final result: {:?}", result);
            Ok(())
        },
        Err(e) => {
            error!("Runtime error: {}", e);
            process::exit(1);
        }
    }
}
