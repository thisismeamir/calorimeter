//! Pratt parser for the Calorimeter expression language (spec §5.2).

use crate::expr::ast::{BinaryOp, Expr, ExprKind, Span, UnaryOp};
use crate::expr::error::ExprError;
use crate::expr::lexer::{tokenize, Token, TokenKind};

/// Allowed built-in functions in the expression language (spec §5.2).
pub const BUILTIN_FUNCTIONS: &[&str] = &[
    "min", "max", "abs", "sqrt", "cbrt", "log", "log2", "log10", "exp", "floor", "ceil", "round",
    "int", "float", "len", "sum",
];

/// Parses an expression string into an `Expr` AST (spec §5.2).
pub fn parse(src: &str) -> Result<Expr, ExprError> {
    let tokens = tokenize(src)?;
    if tokens.is_empty() {
        return Err(ExprError::Parse {
            span: Span::new(0, src.len()),
            message: "empty expression".to_string(),
        });
    }
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        eof_pos: src.len(),
    };
    let expr = parser.parse_expr(0)?;
    if let Some(tok) = parser.peek() {
        return Err(ExprError::Parse {
            span: tok.span,
            message: format!("unexpected token {:?}", tok.kind),
        });
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    eof_pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Token> {
        let tok = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(tok)
    }

    fn current_span(&self) -> Span {
        self.peek()
            .map_or_else(|| Span::new(self.eof_pos, self.eof_pos), |t| t.span)
    }

    fn expect(&mut self, expected: &TokenKind, desc: &str) -> Result<&'a Token, ExprError> {
        let span = self.current_span();
        match self.next() {
            Some(tok) if &tok.kind == expected => Ok(tok),
            Some(tok) => Err(ExprError::Parse {
                span: tok.span,
                message: format!("expected {desc}, found {:?}", tok.kind),
            }),
            None => Err(ExprError::Parse {
                span,
                message: format!("expected {desc}, found end of expression"),
            }),
        }
    }

    /// Pratt parser with binding power `min_bp`.
    ///
    /// Binding powers:
    /// - Conditional (`if ... else ...`): 2 (right-associative: r_bp = 1)
    /// - `or`: 3..4
    /// - `and`: 5..6
    /// - Prefix `not`: r_bp = 7
    /// - Comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`, `in`): 9..10
    /// - `+`, `-`: 11..12
    /// - `*`, `/`, `//`, `%`: 13..14
    /// - Prefix `-`, `+`: r_bp = 15 (binds looser than `**`, so `-2**2 == -(2**2) == -4`)
    /// - `**`: 18..17 (right-associative: `2**3**2 == 2**(3**2) == 512`)
    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, ExprError> {
        let mut lhs = self.parse_prefix()?;

        while let Some(tok) = self.peek() {
            // Check ternary conditional `lhs if cond else else_branch`
            if tok.kind == TokenKind::If {
                let l_bp = 2;
                let r_bp = 1;
                if l_bp < min_bp {
                    break;
                }
                self.next(); // consume `if`
                let cond = self.parse_expr(l_bp + 1)?;
                self.expect(&TokenKind::Else, "`else` in conditional expression")?;
                let else_branch = self.parse_expr(r_bp)?;
                let span = lhs.span.merge(else_branch.span);
                lhs = Expr {
                    kind: ExprKind::Conditional {
                        then_branch: Box::new(lhs),
                        cond: Box::new(cond),
                        else_branch: Box::new(else_branch),
                    },
                    span,
                };
                continue;
            }

            // Reject postfix calls on arbitrary expressions or dotted paths (e.g. `a.b(x)` or `(f)(x)`).
            if tok.kind == TokenKind::LParen {
                return Err(ExprError::Parse {
                    span: tok.span,
                    message: "method or arbitrary expression calls are not allowed; only built-in functions may be called".to_string(),
                });
            }

            let Some((op, l_bp, r_bp)) = infix_binding_power(&tok.kind) else {
                break;
            };

            if l_bp < min_bp {
                break;
            }

            self.next(); // consume operator token
            let rhs = self.parse_expr(r_bp)?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                span,
            };
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ExprError> {
        let span = self.current_span();
        let Some(tok) = self.next() else {
            return Err(ExprError::Parse {
                span,
                message: "unexpected end of expression".to_string(),
            });
        };

        match &tok.kind {
            TokenKind::Int(v) => Ok(Expr {
                kind: ExprKind::Int(*v),
                span: tok.span,
            }),
            TokenKind::Float(v) => Ok(Expr {
                kind: ExprKind::Float(*v),
                span: tok.span,
            }),
            TokenKind::Bool(v) => Ok(Expr {
                kind: ExprKind::Bool(*v),
                span: tok.span,
            }),
            TokenKind::Str(s) => Ok(Expr {
                kind: ExprKind::Str(s.clone()),
                span: tok.span,
            }),
            TokenKind::Minus => {
                let operand = self.parse_expr(15)?;
                let full_span = tok.span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span: full_span,
                })
            }
            TokenKind::Plus => {
                let operand = self.parse_expr(15)?;
                let full_span = tok.span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Pos,
                        operand: Box::new(operand),
                    },
                    span: full_span,
                })
            }
            TokenKind::Not => {
                let operand = self.parse_expr(7)?;
                let full_span = tok.span.merge(operand.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span: full_span,
                })
            }
            TokenKind::LParen => {
                let inner = self.parse_expr(0)?;
                let close = self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Expr {
                    kind: inner.kind,
                    span: tok.span.merge(close.span),
                })
            }
            TokenKind::LBracket => {
                let mut items = Vec::new();
                if let Some(next_tok) = self.peek() {
                    if next_tok.kind == TokenKind::RBracket {
                        let close = self.next().ok_or_else(|| ExprError::Parse {
                            span: tok.span,
                            message: "unclosed `[`".to_string(),
                        })?;
                        return Ok(Expr {
                            kind: ExprKind::List(items),
                            span: tok.span.merge(close.span),
                        });
                    }
                }
                loop {
                    let elem = self.parse_expr(0)?;
                    items.push(elem);
                    match self.peek() {
                        Some(t) if t.kind == TokenKind::Comma => {
                            self.next();
                            if let Some(after_comma) = self.peek() {
                                if after_comma.kind == TokenKind::RBracket {
                                    break;
                                }
                            }
                        }
                        _ => break,
                    }
                }
                let close = self.expect(&TokenKind::RBracket, "`]`")?;
                Ok(Expr {
                    kind: ExprKind::List(items),
                    span: tok.span.merge(close.span),
                })
            }
            TokenKind::Ident(first_ident) => {
                // Check for function call `func(...)` (only allowed on bare identifier, not dotted path).
                if let Some(next_tok) = self.peek() {
                    if next_tok.kind == TokenKind::LParen {
                        if !BUILTIN_FUNCTIONS.contains(&first_ident.as_str()) {
                            return Err(ExprError::FunctionError {
                                func: first_ident.clone(),
                                span: tok.span,
                                message: format!("unknown built-in function `{first_ident}`"),
                            });
                        }
                        self.next(); // consume `(`
                        let mut args = Vec::new();
                        if let Some(pk) = self.peek() {
                            if pk.kind != TokenKind::RParen {
                                loop {
                                    args.push(self.parse_expr(0)?);
                                    if let Some(comma_cand) = self.peek() {
                                        if comma_cand.kind == TokenKind::Comma {
                                            self.next();
                                            continue;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        let close =
                            self.expect(&TokenKind::RParen, "`)` after function arguments")?;
                        let full_span = tok.span.merge(close.span);
                        if self.peek().is_some_and(|t| t.kind == TokenKind::Dot) {
                            return Err(ExprError::Parse {
                                span: full_span,
                                message: "attribute access on function call result is not allowed"
                                    .to_string(),
                            });
                        }
                        return Ok(Expr {
                            kind: ExprKind::Call {
                                func: first_ident.clone(),
                                args,
                            },
                            span: full_span,
                        });
                    }
                }

                // Dotted reference path: `ident (.ident)*` (also allowing integer segments like `.0` if needed)
                let mut path = first_ident.clone();
                let mut end_span = tok.span;

                while let Some(dot_tok) = self.peek() {
                    if dot_tok.kind != TokenKind::Dot {
                        break;
                    }
                    self.next(); // consume `.`
                    let seg_span = self.current_span();
                    let Some(seg_tok) = self.next() else {
                        return Err(ExprError::Parse {
                            span: dot_tok.span,
                            message: "expected identifier after `.`".to_string(),
                        });
                    };
                    match &seg_tok.kind {
                        TokenKind::Ident(seg) => {
                            path.push('.');
                            path.push_str(seg);
                            end_span = seg_tok.span;
                        }
                        TokenKind::In => {
                            // Allow `.in` as a path segment (e.g. `files.in`)
                            path.push_str(".in");
                            end_span = seg_tok.span;
                        }
                        _ => {
                            return Err(ExprError::Parse {
                                span: seg_span,
                                message: format!(
                                    "expected identifier after `.`, found {:?}",
                                    seg_tok.kind
                                ),
                            });
                        }
                    }
                }

                let full_span = tok.span.merge(end_span);
                if let Some(after) = self.peek() {
                    if after.kind == TokenKind::LParen {
                        return Err(ExprError::Parse {
                            span: full_span.merge(after.span),
                            message: format!(
                                "method call on `{path}` is not allowed; only built-in functions may be called"
                            ),
                        });
                    }
                }

                Ok(Expr {
                    kind: ExprKind::Ref(path),
                    span: full_span,
                })
            }
            other => Err(ExprError::Parse {
                span: tok.span,
                message: format!("unexpected token {other:?} at start of expression"),
            }),
        }
    }
}

fn infix_binding_power(kind: &TokenKind) -> Option<(BinaryOp, u8, u8)> {
    let res = match kind {
        TokenKind::Or => (BinaryOp::Or, 3, 4),
        TokenKind::And => (BinaryOp::And, 5, 6),
        TokenKind::EqEq => (BinaryOp::Eq, 9, 10),
        TokenKind::BangEq => (BinaryOp::Ne, 9, 10),
        TokenKind::Lt => (BinaryOp::Lt, 9, 10),
        TokenKind::Le => (BinaryOp::Le, 9, 10),
        TokenKind::Gt => (BinaryOp::Gt, 9, 10),
        TokenKind::Ge => (BinaryOp::Ge, 9, 10),
        TokenKind::In => (BinaryOp::In, 9, 10),
        TokenKind::Plus => (BinaryOp::Add, 11, 12),
        TokenKind::Minus => (BinaryOp::Sub, 11, 12),
        TokenKind::Star => (BinaryOp::Mul, 13, 14),
        TokenKind::Slash => (BinaryOp::Div, 13, 14),
        TokenKind::SlashSlash => (BinaryOp::FloorDiv, 13, 14),
        TokenKind::Percent => (BinaryOp::Mod, 13, 14),
        // Right-associative exponentiation, higher binding power than prefix unary minus (15)
        TokenKind::StarStar => (BinaryOp::Pow, 18, 17),
        _ => return None,
    };
    Some(res)
}
