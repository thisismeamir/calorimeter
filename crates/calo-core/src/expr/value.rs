//! Runtime `Value` representation and `Env` lookup trait for expressions (spec §5.2).

use calo_schema::ScalarValue;
use std::fmt;

/// Runtime value produced by evaluating an `Expr` (spec §5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 64-bit signed integer.
    Int(i64),
    /// 64-bit IEEE-754 floating-point number.
    Float(f64),
    /// Boolean value.
    Bool(bool),
    /// UTF-8 string.
    Str(String),
    /// Homogeneous or heterogeneous list of values.
    List(Vec<Value>),
}

impl Value {
    /// Returns a human-readable type name for error messages.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Bool(_) => "bool",
            Self::Str(_) => "str",
            Self::List(_) => "list",
        }
    }

    /// Converts a `Value` to `f64` if it is `Int` or `Float`.
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Converts a `calo_schema::ScalarValue` into a runtime `Value`.
    pub fn from_scalar(s: &ScalarValue) -> Self {
        match s {
            ScalarValue::Int(i) => Self::Int(*i),
            ScalarValue::Float(f) => Self::Float(*f),
            ScalarValue::Bool(b) => Self::Bool(*b),
            ScalarValue::Str(st) => Self::Str(st.clone()),
        }
    }

    /// Converts this `Value` into a `calo_schema::ScalarValue` if it is a scalar.
    pub fn to_scalar(&self) -> Option<ScalarValue> {
        match self {
            Self::Int(i) => Some(ScalarValue::Int(*i)),
            Self::Float(f) => Some(ScalarValue::Float(*f)),
            Self::Bool(b) => Some(ScalarValue::Bool(*b)),
            Self::Str(s) => Some(ScalarValue::Str(s.clone())),
            Self::List(_) => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(fl) => write!(f, "{fl}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Str(s) => write!(f, "{s}"),
            Self::List(items) => {
                write!(f, "[")?;
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
        }
    }
}

/// Environment trait for resolving dotted reference paths during expression evaluation (spec §5.2).
pub trait Env {
    /// Looks up a reference path such as `"params.N"`, `"machine.node.gpu.per_node"`, or `"wall_s"`.
    fn get(&self, path: &str) -> Option<Value>;
}
