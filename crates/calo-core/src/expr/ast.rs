//! Abstract syntax tree and byte span definitions for the expression language (spec §5.2).

use std::fmt;

/// Half-open byte span `[start, end)` within an expression or template source string (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive start byte index.
    pub start: usize,
    /// Exclusive end byte index.
    pub end: usize,
}

impl Span {
    /// Creates a new `Span` covering `[start, end)`.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Merges two spans into a span covering from the minimum start to the maximum end.
    pub fn merge(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// Unary prefix operators (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Numeric negation (`-x`).
    Neg,
    /// Numeric unary plus (`+x`).
    Pos,
    /// Logical negation (`not x`).
    Not,
}

/// Binary operators (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// Addition (`+`) or string/list concatenation.
    Add,
    /// Subtraction (`-`).
    Sub,
    /// Multiplication (`*`).
    Mul,
    /// Floating-point division (`/`).
    Div,
    /// Floor division (`//`).
    FloorDiv,
    /// Modulo (`%`).
    Mod,
    /// Exponentiation (`**`).
    Pow,
    /// Equality (`==`).
    Eq,
    /// Inequality (`!=`).
    Ne,
    /// Less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Le,
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Ge,
    /// Containment (`x in y`).
    In,
    /// Logical conjunction (`and`).
    And,
    /// Logical disjunction (`or`).
    Or,
}

/// A spanned expression node (spec §5.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Expression variant.
    pub kind: ExprKind,
    /// Byte span in the input string.
    pub span: Span,
}

/// AST node variants for the Calorimeter expression language (spec §5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// 64-bit signed integer literal.
    Int(i64),
    /// 64-bit floating-point literal.
    Float(f64),
    /// Boolean literal (`true` / `false`).
    Bool(bool),
    /// String literal.
    Str(String),
    /// List literal (`[e1, e2, ...]`).
    List(Vec<Expr>),
    /// Dotted variable/metric reference (e.g. `params.N`, `machine.node.gpu.per_node`, `wall_s`).
    Ref(String),
    /// Unary operation.
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        operand: Box<Expr>,
    },
    /// Binary operation.
    Binary {
        /// Binary operator.
        op: BinaryOp,
        /// Left-hand operand.
        left: Box<Expr>,
        /// Right-hand operand.
        right: Box<Expr>,
    },
    /// Ternary conditional expression (`then_branch if cond else else_branch`).
    Conditional {
        /// Value evaluated if `cond` is true.
        then_branch: Box<Expr>,
        /// Boolean condition expression.
        cond: Box<Expr>,
        /// Value evaluated if `cond` is false.
        else_branch: Box<Expr>,
    },
    /// Built-in function call (`func(arg1, arg2, ...)`).
    Call {
        /// Function name.
        func: String,
        /// Positional argument expressions.
        args: Vec<Expr>,
    },
}
