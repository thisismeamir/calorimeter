//! Hand-written lexer for the Calorimeter expression language (spec §5.2).

use crate::expr::ast::Span;
use crate::expr::error::ExprError;

/// Token kinds emitted by the expression lexer (spec §5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Integer literal.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// Boolean literal (`true` or `false`).
    Bool(bool),
    /// String literal (`"..."` or `'...'`).
    Str(String),
    /// Identifier (`params`, `N`, `cbrt`, etc.).
    Ident(String),
    /// `.`
    Dot,
    /// `,`
    Comma,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `**`
    StarStar,
    /// `/`
    Slash,
    /// `//`
    SlashSlash,
    /// `%`
    Percent,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// Keyword `and`
    And,
    /// Keyword `or`
    Or,
    /// Keyword `not`
    Not,
    /// Keyword `in`
    In,
    /// Keyword `if`
    If,
    /// Keyword `else`
    Else,
}

/// A token paired with its byte `Span` in the input string (spec §5.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token category and payload.
    pub kind: TokenKind,
    /// Byte span in the source string.
    pub span: Span,
}

/// Tokenizes an expression string into a vector of `Token`s (spec §5.2).
pub fn tokenize(src: &str) -> Result<Vec<Token>, ExprError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < len {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let start = i;

        match b {
            b'.' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    span: Span::new(start, i),
                });
            }
            b',' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    span: Span::new(start, i),
                });
            }
            b'(' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    span: Span::new(start, i),
                });
            }
            b')' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    span: Span::new(start, i),
                });
            }
            b'[' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::LBracket,
                    span: Span::new(start, i),
                });
            }
            b']' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::RBracket,
                    span: Span::new(start, i),
                });
            }
            b'+' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Plus,
                    span: Span::new(start, i),
                });
            }
            b'-' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Minus,
                    span: Span::new(start, i),
                });
            }
            b'*' => {
                if i + 1 < len && bytes[i + 1] == b'*' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::StarStar,
                        span: Span::new(start, i),
                    });
                } else {
                    i += 1;
                    tokens.push(Token {
                        kind: TokenKind::Star,
                        span: Span::new(start, i),
                    });
                }
            }
            b'/' => {
                if i + 1 < len && bytes[i + 1] == b'/' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::SlashSlash,
                        span: Span::new(start, i),
                    });
                } else {
                    i += 1;
                    tokens.push(Token {
                        kind: TokenKind::Slash,
                        span: Span::new(start, i),
                    });
                }
            }
            b'%' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Percent,
                    span: Span::new(start, i),
                });
            }
            b'=' => {
                if i + 1 < len && bytes[i + 1] == b'=' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::EqEq,
                        span: Span::new(start, i),
                    });
                } else {
                    return Err(ExprError::Lex {
                        span: Span::new(start, start + 1),
                        message:
                            "assignment `=` is not allowed in expressions (use `==` for equality)"
                                .to_string(),
                    });
                }
            }
            b'!' => {
                if i + 1 < len && bytes[i + 1] == b'=' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::BangEq,
                        span: Span::new(start, i),
                    });
                } else {
                    return Err(ExprError::Lex {
                        span: Span::new(start, start + 1),
                        message: "unexpected `!` (use `not` or `!=`)".to_string(),
                    });
                }
            }
            b'<' => {
                if i + 1 < len && bytes[i + 1] == b'=' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::Le,
                        span: Span::new(start, i),
                    });
                } else {
                    i += 1;
                    tokens.push(Token {
                        kind: TokenKind::Lt,
                        span: Span::new(start, i),
                    });
                }
            }
            b'>' => {
                if i + 1 < len && bytes[i + 1] == b'=' {
                    i += 2;
                    tokens.push(Token {
                        kind: TokenKind::Ge,
                        span: Span::new(start, i),
                    });
                } else {
                    i += 1;
                    tokens.push(Token {
                        kind: TokenKind::Gt,
                        span: Span::new(start, i),
                    });
                }
            }
            b'"' | b'\'' => {
                let quote = b;
                i += 1;
                let mut s = String::new();
                let mut closed = false;
                while i < len {
                    let ch = bytes[i];
                    if ch == quote {
                        i += 1;
                        closed = true;
                        break;
                    } else if ch == b'\\' {
                        i += 1;
                        if i >= len {
                            return Err(ExprError::Lex {
                                span: Span::new(start, i),
                                message: "unterminated escape sequence in string literal"
                                    .to_string(),
                            });
                        }
                        let esc = bytes[i];
                        match esc {
                            b'n' => s.push('\n'),
                            b'r' => s.push('\r'),
                            b't' => s.push('\t'),
                            b'\\' => s.push('\\'),
                            b'"' => s.push('"'),
                            b'\'' => s.push('\''),
                            other => {
                                return Err(ExprError::Lex {
                                    span: Span::new(i - 1, i + 1),
                                    message: format!(
                                        "unsupported escape sequence `\\{}`",
                                        other as char
                                    ),
                                });
                            }
                        }
                        i += 1;
                    } else {
                        // Advance full UTF-8 char
                        let rest = &src[i..];
                        let Some(c) = rest.chars().next() else {
                            break;
                        };
                        s.push(c);
                        i += c.len_utf8();
                    }
                }
                if !closed {
                    return Err(ExprError::Lex {
                        span: Span::new(start, len),
                        message: "unterminated string literal".to_string(),
                    });
                }
                tokens.push(Token {
                    kind: TokenKind::Str(s),
                    span: Span::new(start, i),
                });
            }
            c if c.is_ascii_digit() => {
                let mut is_float = false;
                while i < len && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                    i += 1;
                }
                if i < len && bytes[i] == b'.' && (i + 1 >= len || bytes[i + 1] != b'.') {
                    is_float = true;
                    i += 1;
                    while i < len && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                        i += 1;
                    }
                }
                if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
                    is_float = true;
                    i += 1;
                    if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                    let exp_start = i;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == exp_start {
                        return Err(ExprError::Lex {
                            span: Span::new(start, i),
                            message: "missing digits in float exponent".to_string(),
                        });
                    }
                }
                let raw_num: String = src[start..i].chars().filter(|&ch| ch != '_').collect();
                let span = Span::new(start, i);
                if is_float {
                    let val: f64 = raw_num.parse().map_err(|_| ExprError::Lex {
                        span,
                        message: format!("invalid float literal `{}`", &src[start..i]),
                    })?;
                    tokens.push(Token {
                        kind: TokenKind::Float(val),
                        span,
                    });
                } else {
                    let val: i64 = raw_num.parse().map_err(|_| ExprError::Lex {
                        span,
                        message: format!("invalid integer literal `{}`", &src[start..i]),
                    })?;
                    tokens.push(Token {
                        kind: TokenKind::Int(val),
                        span,
                    });
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &src[start..i];
                let span = Span::new(start, i);
                let kind = match word {
                    "true" | "True" => TokenKind::Bool(true),
                    "false" | "False" => TokenKind::Bool(false),
                    "and" => TokenKind::And,
                    "or" => TokenKind::Or,
                    "not" => TokenKind::Not,
                    "in" => TokenKind::In,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    _ => TokenKind::Ident(word.to_string()),
                };
                tokens.push(Token { kind, span });
            }
            _ => {
                let rest = &src[i..];
                let ch_len = rest.chars().next().map_or(1, char::len_utf8);
                return Err(ExprError::Lex {
                    span: Span::new(start, start + ch_len),
                    message: format!("unexpected character `{}`", &src[start..start + ch_len]),
                });
            }
        }
    }

    Ok(tokens)
}
