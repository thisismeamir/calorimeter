//! Safe, side-effect-free expression language and template interpolation (spec §5.2, §8.4 R-STP-8).

pub mod ast;
pub mod error;
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod template;
pub mod value;

pub use ast::{BinaryOp, Expr, ExprKind, Span, UnaryOp};
pub use error::ExprError;
pub use eval::{eval, references, MapEnv};
pub use parser::parse;
pub use template::{interpolate, render_template};
pub use value::{Env, Value};
