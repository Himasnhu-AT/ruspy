use std::str::Chars;

/// Token enumeration representing different lexical elements in the Ruspy language
/// Each variant corresponds to a specific type of token that can be recognized by the lexer
#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    // identifiers
    Identifier(String),
    Number(i64),

    // operators
    Plus,
    Minus,
    Asterisk,
    Slash,

    // brackets
    LParen,
    RParen,

    // assignment
    Assign,

    // statement terminator
    Semicolon,
    Colon,
    EOF,

    // Variable type tokens
    TypeInt,
    TypeInt32,
    TypeInt64,
    TypeFloat,
    TypeFloat32,
    TypeFloat64,
    TypeChar,
    TypeStr,
    TypeStr8,
    TypeStr32,
    TypeStr64,

    // Add print keyword
    Print,

    // Add string literals
    StringLiteral(String),
}

/// Lexer struct responsible for tokenizing input source code
/// Maintains state about the current position in the input stream
#[derive(Clone)]
pub struct Lexer<'a> {
    /// Iterator over the input characters
    input: Chars<'a>,
    /// Current character being processed
    current_char: Option<char>,
    /// Source text for cloning purposes
    source: &'a str,
    /// Current position in the input
    position: usize,
}

impl<'a> Lexer<'a> {
    /// Creates a new Lexer instance from the input string
    ///
    /// # Arguments
    /// * `input` - The source code string to be tokenized
    ///
    /// # Returns
    /// * A new Lexer instance initialized with the input
    pub fn new(input: &'a str) -> Self {
        let mut lexer = Lexer {
            input: input.chars(),
            current_char: None,
            source: input,
            position: 0,
        };
        lexer.advance();
        lexer
    }

    /// Advances the lexer to the next character in the input stream
    ///
    /// This method updates the current_char field with the next character
    /// or None if we've reached the end of input
    fn advance(&mut self) {
        self.current_char = self.input.next();
    }

    /// Returns the next token from the input stream
    ///
    /// # Returns
    /// * The next Token in the sequence
    ///
    /// # Panics
    /// * When encountering unexpected characters in the input
    pub fn get_next_token(&mut self) -> Token {
        // Skip whitespace and process the next meaningful character
        while let Some(c) = self.current_char {
            if c.is_whitespace() {
                self.advance();
                continue;
            }

            // Handle different character types
            if c.is_alphabetic() {
                return self.identifier();
            }

            if c.is_digit(10) {
                return self.number();
            }

            // Match single-character tokens
            match c {
                '+' => {
                    self.advance();
                    return Token::Plus;
                }
                '-' => {
                    self.advance();
                    return Token::Minus;
                }
                '*' => {
                    self.advance();
                    return Token::Asterisk;
                }
                '/' => {
                    self.advance();
                    // Check if it's a comment (// for line comment)
                    if let Some('/') = self.current_char {
                        self.advance(); // consume the second '/'
                        // Skip everything until end of line or end of file
                        while let Some(c) = self.current_char {
                            if c == '\n' {
                                self.advance(); // consume the newline
                                break;
                            }
                            self.advance(); // consume the comment character
                        }
                        // After skipping the comment, get the next token
                        return self.get_next_token();
                    } else {
                        return Token::Slash;
                    }
                }
                '(' => {
                    self.advance();
                    return Token::LParen;
                }
                ')' => {
                    self.advance();
                    return Token::RParen;
                }
                '=' => {
                    self.advance();
                    return Token::Assign;
                }
                ';' => {
                    self.advance();
                    return Token::Semicolon;
                }
                ':' => {
                    self.advance();
                    return Token::Colon;
                }
                '"' => {
                    return self.string_literal();
                }
                _ => panic!("Unexpected character: {}", c),
            }
        }
        Token::EOF
    }

    /// Processes and returns an identifier or keyword token
    ///
    /// # Returns
    /// * Either a keyword Token or an Identifier Token containing the lexeme
    fn identifier(&mut self) -> Token {
        let mut result = String::new();
        // Collect all alphanumeric characters
        while let Some(c) = self.current_char {
            if c.is_alphanumeric() {
                result.push(c);
                self.advance();
            } else {
                break;
            }
        }

        // Match keywords for types, otherwise return as identifier
        match result.as_str() {
            "int" => Token::TypeInt,
            "int32" => Token::TypeInt32,
            "int64" => Token::TypeInt64,
            "float" => Token::TypeFloat,
            "float32" => Token::TypeFloat32,
            "float64" => Token::TypeFloat64,
            "char" => Token::TypeChar,
            "str" => Token::TypeStr,
            "str8" => Token::TypeStr8,
            "str32" => Token::TypeStr32,
            "str64" => Token::TypeStr64,
            "print" => Token::Print,
            _ => Token::Identifier(result),
        }
    }

    /// Processes and returns a number token
    ///
    /// # Returns
    /// * A Number Token containing the parsed value
    ///
    /// # Panics
    /// * When the number cannot be parsed as an i64
    fn number(&mut self) -> Token {
        let mut result = String::new();
        // Collect all consecutive digits
        while let Some(c) = self.current_char {
            if c.is_digit(10) {
                result.push(c);
                self.advance();
            } else {
                break;
            }
        }
        Token::Number(result.parse::<i64>().unwrap())
    }

    /// Processes and returns a string literal token
    ///
    /// # Returns
    /// * A StringLiteral Token containing the parsed string
    ///
    /// # Panics
    /// * When the string is not properly terminated
    fn string_literal(&mut self) -> Token {
        self.advance(); // Skip the opening quote
        let mut result = String::new();

        // Collect characters until closing quote
        while let Some(c) = self.current_char {
            if c == '"' {
                self.advance(); // Skip the closing quote
                return Token::StringLiteral(result);
            }
            result.push(c);
            self.advance();
        }

        panic!("Unterminated string literal");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_numbers() {
        let mut lexer = Lexer::new("123 456");
        assert_eq!(lexer.get_next_token(), Token::Number(123));
        assert_eq!(lexer.get_next_token(), Token::Number(456));
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }

    #[test]
    fn test_lexer_operators() {
        let mut lexer = Lexer::new("+ - * /");
        assert_eq!(lexer.get_next_token(), Token::Plus);
        assert_eq!(lexer.get_next_token(), Token::Minus);
        assert_eq!(lexer.get_next_token(), Token::Asterisk);
        assert_eq!(lexer.get_next_token(), Token::Slash);
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }

    #[test]
    fn test_lexer_identifiers_and_types() {
        let mut lexer = Lexer::new("x int str64 float32");
        assert_eq!(lexer.get_next_token(), Token::Identifier("x".to_string()));
        assert_eq!(lexer.get_next_token(), Token::TypeInt);
        assert_eq!(lexer.get_next_token(), Token::TypeStr64);
        assert_eq!(lexer.get_next_token(), Token::TypeFloat32);
    }

    #[test]
    fn test_lexer_complex_expression() {
        let mut lexer = Lexer::new("x: int = 42;");
        assert_eq!(lexer.get_next_token(), Token::Identifier("x".to_string()));
        assert_eq!(lexer.get_next_token(), Token::Colon);
        assert_eq!(lexer.get_next_token(), Token::TypeInt);
        assert_eq!(lexer.get_next_token(), Token::Assign);
        assert_eq!(lexer.get_next_token(), Token::Number(42));
        assert_eq!(lexer.get_next_token(), Token::Semicolon);
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }

    #[test]
    fn test_lexer_whitespace_handling() {
        let mut lexer = Lexer::new("   42   +   58   ");
        assert_eq!(lexer.get_next_token(), Token::Number(42));
        assert_eq!(lexer.get_next_token(), Token::Plus);
        assert_eq!(lexer.get_next_token(), Token::Number(58));
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }

    #[test]
    fn test_lexer_invalid_character() {
        let mut lexer = Lexer::new("@");
        let result = std::panic::catch_unwind(move || {
            lexer.get_next_token();
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_lexer_comments() {
        let mut lexer = Lexer::new("42 // This is a comment\n+ 58");
        assert_eq!(lexer.get_next_token(), Token::Number(42));
        assert_eq!(lexer.get_next_token(), Token::Plus);
        assert_eq!(lexer.get_next_token(), Token::Number(58));
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }

    #[test]
    fn test_lexer_comments_at_end() {
        let mut lexer = Lexer::new("42 + 58 // Final comment");
        assert_eq!(lexer.get_next_token(), Token::Number(42));
        assert_eq!(lexer.get_next_token(), Token::Plus);
        assert_eq!(lexer.get_next_token(), Token::Number(58));
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }
}
