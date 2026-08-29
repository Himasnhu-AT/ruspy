//! Lexer — turns source into a `Vec<Spanned<Token>>` plus a list of `Diag`s.
//!
//! Unlike the original pull-model lexer (which *panicked* on an unexpected char or
//! an unterminated string), this pass **never panics and never stops**: bad input
//! produces a diagnostic and lexing recovers, so an editor gets every error at once
//! and the parser gets a clean token stream ending in `Eof`.

use crate::diagnostics::{Diag, Span, Spanned};
use serde::{Deserialize, Serialize};

/// A lexical token. Data-carrying variants hold their literal value.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum Token {
    // literals + identifiers
    Identifier(String),
    Number(i64),
    FloatLiteral(f64),
    StringLiteral(String),

    // operators
    Plus,
    Minus,
    Asterisk,
    Slash,
    Percent,
    Bang,
    AmpAmp,
    PipePipe,
    // compound assignment
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,

    // brackets / punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Assign,
    Semicolon,
    Colon,

    // type keywords
    TypeInt,
    TypeInt32,
    TypeInt64,
    TypeFloat,
    TypeFloat32,
    TypeFloat64,
    TypeChar,
    TypeBool,
    TypeStr,
    TypeStr8,
    TypeStr32,
    TypeStr64,

    // keywords
    Print,
    If,
    Else,
    True,
    False,
    Fn,
    Def,
    Return,
    While,
    For,
    In,
    Break,
    Continue,
    Struct,
    Import,

    // range operators
    DotDot,
    DotDotEq,
    Arrow,

    // comparisons
    Eq,
    NotEq,
    Gt,
    Lt,
    GtEq,
    LtEq,

    /// End of input (always the last token; carries a zero-width span at EOF).
    Eof,
}

impl Token {
    /// The keyword/type token for an identifier word, or `None` if it's a plain ident.
    fn keyword(word: &str) -> Option<Token> {
        Some(match word {
            "int" => Token::TypeInt,
            "int32" => Token::TypeInt32,
            "int64" => Token::TypeInt64,
            "float" => Token::TypeFloat,
            "float32" => Token::TypeFloat32,
            "float64" => Token::TypeFloat64,
            "char" => Token::TypeChar,
            "bool" => Token::TypeBool,
            "str" => Token::TypeStr,
            "str8" => Token::TypeStr8,
            "str32" => Token::TypeStr32,
            "str64" => Token::TypeStr64,
            "print" => Token::Print,
            "if" => Token::If,
            "else" => Token::Else,
            "true" => Token::True,
            "false" => Token::False,
            "fn" => Token::Fn,
            "def" => Token::Def,
            "return" => Token::Return,
            "while" => Token::While,
            "for" => Token::For,
            "in" => Token::In,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "struct" => Token::Struct,
            "import" => Token::Import,
            _ => return None,
        })
    }
}

/// Lex `src` into spanned tokens (always ending in `Eof`) plus any diagnostics.
/// Never panics; on bad input it records a `Diag` and continues.
pub fn tokenize(src: &str) -> (Vec<Spanned<Token>>, Vec<Diag>) {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut toks = Vec::new();
    let mut diags = Vec::new();
    let mut i = 0usize;

    let is_ident_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    while i < n {
        let b = bytes[i];

        // whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // line comment `// ...`
        if b == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        let start = i;

        // identifiers / keywords / type keywords
        if is_ident_start(b) {
            i += 1;
            while i < n && is_ident(bytes[i]) {
                i += 1;
            }
            let word = &src[start..i];
            let tok = Token::keyword(word).unwrap_or_else(|| Token::Identifier(word.to_string()));
            toks.push(Spanned::new(tok, Span::new(start, i)));
            continue;
        }

        // numbers — a dot only continues the number when a digit follows, so `x.bid`
        // stays member access (matches the language contract).
        if b.is_ascii_digit() {
            i += 1;
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let mut is_float = false;
            if i + 1 < n && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                is_float = true;
                i += 1;
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text = &src[start..i];
            let span = Span::new(start, i);
            if is_float {
                match text.parse::<f64>() {
                    Ok(v) => toks.push(Spanned::new(Token::FloatLiteral(v), span)),
                    Err(_) => diags.push(Diag::error(span, "E0002", format!("invalid float literal `{text}`"))),
                }
            } else {
                match text.parse::<i64>() {
                    Ok(v) => toks.push(Spanned::new(Token::Number(v), span)),
                    Err(_) => diags.push(
                        Diag::error(span, "E0003", format!("integer literal `{text}` is out of range for int64"))
                            .with_help("use a smaller value"),
                    ),
                }
            }
            continue;
        }

        // string literal — an unterminated string is a *diagnostic*, not a panic.
        if b == b'"' {
            i += 1;
            let content_start = i;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            if i >= n {
                diags.push(
                    Diag::error(Span::new(start, n), "E0004", "unterminated string literal")
                        .with_help("add a closing `\"`"),
                );
                // recover: treat the rest of input as the string body
                toks.push(Spanned::new(Token::StringLiteral(src[content_start..n].to_string()), Span::new(start, n)));
                break;
            }
            let value = src[content_start..i].to_string();
            i += 1; // consume closing quote
            toks.push(Spanned::new(Token::StringLiteral(value), Span::new(start, i)));
            continue;
        }

        // multi-char then single-char operators / punctuation
        let three = if i + 2 < n { &src[i..i + 3] } else { "" };
        let two = if i + 1 < n { &src[i..i + 2] } else { "" };
        let (tok, len) = match three {
            "..=" => (Some(Token::DotDotEq), 3),
            _ => match two {
            ".." => (Some(Token::DotDot), 2),
            "->" => (Some(Token::Arrow), 2),
            "==" => (Some(Token::Eq), 2),
            "!=" => (Some(Token::NotEq), 2),
            ">=" => (Some(Token::GtEq), 2),
            "<=" => (Some(Token::LtEq), 2),
            "&&" => (Some(Token::AmpAmp), 2),
            "||" => (Some(Token::PipePipe), 2),
            "+=" => (Some(Token::PlusEq), 2),
            "-=" => (Some(Token::MinusEq), 2),
            "*=" => (Some(Token::StarEq), 2),
            "/=" => (Some(Token::SlashEq), 2),
            "%=" => (Some(Token::PercentEq), 2),
            _ => match b {
                b'+' => (Some(Token::Plus), 1),
                b'-' => (Some(Token::Minus), 1),
                b'*' => (Some(Token::Asterisk), 1),
                b'/' => (Some(Token::Slash), 1),
                b'%' => (Some(Token::Percent), 1),
                b'!' => (Some(Token::Bang), 1),
                b'(' => (Some(Token::LParen), 1),
                b')' => (Some(Token::RParen), 1),
                b'{' => (Some(Token::LBrace), 1),
                b'}' => (Some(Token::RBrace), 1),
                b'[' => (Some(Token::LBracket), 1),
                b']' => (Some(Token::RBracket), 1),
                b',' => (Some(Token::Comma), 1),
                b'.' => (Some(Token::Dot), 1),
                b'=' => (Some(Token::Assign), 1),
                b'>' => (Some(Token::Gt), 1),
                b'<' => (Some(Token::Lt), 1),
                b';' => (Some(Token::Semicolon), 1),
                b':' => (Some(Token::Colon), 1),
                _ => (None, 1),
            },
            },
        };
        match tok {
            Some(t) => {
                toks.push(Spanned::new(t, Span::new(i, i + len)));
                i += len;
            }
            None => {
                // Unknown character: report and skip one whole char (recovery).
                let ch = src[i..].chars().next().unwrap_or('\u{fffd}');
                let clen = ch.len_utf8();
                diags.push(Diag::error(Span::new(i, i + clen), "E0001", format!("unexpected character `{ch}`")));
                i += clen;
            }
        }
    }

    toks.push(Spanned::new(Token::Eof, Span::point(n)));
    (toks, diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Just the token kinds, dropping spans/EOF, for terse assertions.
    fn kinds(src: &str) -> Vec<Token> {
        let (toks, diags) = tokenize(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        toks.into_iter().map(|t| t.node).filter(|t| *t != Token::Eof).collect()
    }

    #[test]
    fn numbers_and_floats() {
        assert_eq!(kinds("123 456"), vec![Token::Number(123), Token::Number(456)]);
        assert_eq!(kinds("3.14"), vec![Token::FloatLiteral(3.14)]);
        // a dot with no trailing digit is member access, not a float
        assert_eq!(kinds("x.bid"), vec![Token::Identifier("x".into()), Token::Dot, Token::Identifier("bid".into())]);
    }

    #[test]
    fn operators_and_two_char() {
        assert_eq!(kinds("+ - * /"), vec![Token::Plus, Token::Minus, Token::Asterisk, Token::Slash]);
        assert_eq!(
            kinds("a >= b <= c == d != e"),
            vec![
                Token::Identifier("a".into()), Token::GtEq, Token::Identifier("b".into()), Token::LtEq,
                Token::Identifier("c".into()), Token::Eq, Token::Identifier("d".into()), Token::NotEq,
                Token::Identifier("e".into()),
            ]
        );
    }

    #[test]
    fn identifiers_types_and_keywords() {
        assert_eq!(
            kinds("x int str64 float32 print if else"),
            vec![
                Token::Identifier("x".into()), Token::TypeInt, Token::TypeStr64, Token::TypeFloat32,
                Token::Print, Token::If, Token::Else,
            ]
        );
    }

    #[test]
    fn typed_declaration_and_string() {
        assert_eq!(
            kinds("x: int = 42;"),
            vec![
                Token::Identifier("x".into()), Token::Colon, Token::TypeInt, Token::Assign,
                Token::Number(42), Token::Semicolon,
            ]
        );
        assert_eq!(kinds("\"hello\""), vec![Token::StringLiteral("hello".into())]);
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(kinds("42 // comment\n+ 58"), vec![Token::Number(42), Token::Plus, Token::Number(58)]);
        assert_eq!(kinds("42 + 58 // trailing"), vec![Token::Number(42), Token::Plus, Token::Number(58)]);
    }

    #[test]
    fn invalid_character_is_a_diagnostic_not_a_panic() {
        // The old lexer panicked here; now it recovers with a Diag and keeps lexing.
        let (toks, diags) = tokenize("1 @ 2");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E0001");
        assert!(diags[0].message.contains('@'));
        let nums: Vec<_> = toks.iter().filter(|t| matches!(t.node, Token::Number(_))).collect();
        assert_eq!(nums.len(), 2, "lexing recovered — both numbers tokenized");
    }

    #[test]
    fn unterminated_string_recovers() {
        let (_toks, diags) = tokenize("x = \"oops");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E0004");
    }

    #[test]
    fn spans_point_at_the_right_bytes() {
        let (toks, _) = tokenize("ab + 12");
        assert_eq!(toks[0].span, Span::new(0, 2)); // ab
        assert_eq!(toks[1].span, Span::new(3, 4)); // +
        assert_eq!(toks[2].span, Span::new(5, 7)); // 12
        assert_eq!(toks[3].node, Token::Eof);
        assert!(toks[3].span.is_empty());
    }

    #[test]
    fn multiple_errors_all_reported() {
        let (_toks, diags) = tokenize("@ # $");
        assert_eq!(diags.len(), 3, "every bad char reported, not just the first");
    }
}
