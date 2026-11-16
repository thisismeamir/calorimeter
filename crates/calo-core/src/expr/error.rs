//! Spanned error types for the expression language and template renderer (spec §5.2).

use crate::expr::ast::Span;
use thiserror::Error;

/// Errors produced when lexing, parsing, evaluating, or interpolating expressions (spec §5.2).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExprError {
    /// Lexical error at a specific byte span.
    #[error("syntax error at {span}: {message}")]
    Lex {
        /// Byte span in the expression source string.
        span: Span,
        /// Description of the lexical error.
        message: String,
    },

    /// Syntax/parse error at a specific byte span.
    #[error("parse error at {span}: {message}")]
    Parse {
        /// Byte span in the expression source string.
        span: Span,
        /// Description of the parse error.
        message: String,
    },

    /// Reference to an unknown identifier or dotted path (spec §5.2).
    #[error("unresolved reference `{name}` at {span}")]
    UnresolvedReference {
        /// Dotted reference path that could not be found in `Env`.
        name: String,
        /// Byte span of the reference in the expression source string.
        span: Span,
    },

    /// Division or modulo by zero (spec §5.2).
    #[error("division by zero at {span}")]
    DivisionByZero {
        /// Byte span of the division/modulo expression.
        span: Span,
    },

    /// Type mismatch or invalid operand types during evaluation.
    #[error("type error at {span}: {message}")]
    TypeError {
        /// Byte span of the offending operation.
        span: Span,
        /// Explanation of the type mismatch.
        message: String,
    },

    /// Unknown function name or invalid argument count/domain.
    #[error("function error in `{func}` at {span}: {message}")]
    FunctionError {
        /// Function name.
        func: String,
        /// Byte span of the call expression.
        span: Span,
        /// Detailed reason.
        message: String,
    },

    /// Template rendering or interpolation error (spec §5.2, §8.4 R-STP-8).
    #[error("template error at {span}: {message}")]
    TemplateError {
        /// Byte span in the template string.
        span: Span,
        /// Detailed error message.
        message: String,
    },
}

impl ExprError {
    /// Returns the byte `Span` associated with this error.
    pub const fn span(&self) -> Span {
        match self {
            Self::Lex { span, .. }
            | Self::Parse { span, .. }
            | Self::UnresolvedReference { span, .. }
            | Self::DivisionByZero { span, .. }
            | Self::TypeError { span, .. }
            | Self::FunctionError { span, .. }
            | Self::TemplateError { span, .. } => *span,
        }
    }

    /// Offsets the byte span of this error by `offset` bytes (used when embedding `${{ ... }}` inside templates).
    pub fn with_offset(self, offset: usize) -> Self {
        let shift = |s: Span| Span::new(s.start + offset, s.end + offset);
        match self {
            Self::Lex { span, message } => Self::Lex {
                span: shift(span),
                message,
            },
            Self::Parse { span, message } => Self::Parse {
                span: shift(span),
                message,
            },
            Self::UnresolvedReference { name, span } => Self::UnresolvedReference {
                name,
                span: shift(span),
            },
            Self::DivisionByZero { span } => Self::DivisionByZero { span: shift(span) },
            Self::TypeError { span, message } => Self::TypeError {
                span: shift(span),
                message,
            },
            Self::FunctionError {
                func,
                span,
                message,
            } => Self::FunctionError {
                func,
                span: shift(span),
                message,
            },
            Self::TemplateError { span, message } => Self::TemplateError {
                span: shift(span),
                message,
            },
        }
    }
}
