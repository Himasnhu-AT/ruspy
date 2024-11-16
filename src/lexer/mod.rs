use std::str::Chars;

#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    Identifier(String),
    Number(i64),
    Plus,
    Minus,
    Asterisk,
    Slash,
    LParen,
    RParen,
    EOF,
    Assign,
    Semicolon,
}

pub struct Lexer<'a> {
    input: Chars<'a>,
    current_char: Option<char>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        let mut lexer = Lexer {
            input: input.chars(),
            current_char: None,
        };
        lexer.advance();
        lexer
    }

    fn advance(&mut self) {
        self.current_char = self.input.next();
    }

    pub fn get_next_token(&mut self) -> Token {
        while let Some(c) = self.current_char {
            if c.is_whitespace() {
                self.advance();
                continue;
            }

            if c.is_alphabetic() {
                return self.identifier();
            }

            if c.is_digit(10) {
                return self.number();
            }

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
                    return Token::Slash;
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
                _ => panic!("Unexpected character: {}", c),
            }
        }
        Token::EOF
    }

    fn identifier(&mut self) -> Token {
        let mut result = String::new();
        while let Some(c) = self.current_char {
            if c.is_alphanumeric() {
                result.push(c);
                self.advance();
            } else {
                break;
            }
        }
        Token::Identifier(result)
    }

    fn number(&mut self) -> Token {
        let mut result = String::new();
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
    fn test_lexer_parentheses() {
        let mut lexer = Lexer::new("( )");
        assert_eq!(lexer.get_next_token(), Token::LParen);
        assert_eq!(lexer.get_next_token(), Token::RParen);
        assert_eq!(lexer.get_next_token(), Token::EOF);
    }
}
