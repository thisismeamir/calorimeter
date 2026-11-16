//! Evaluator and static reference extractor for `Expr` ASTs (spec §5.2).

use crate::expr::ast::{BinaryOp, Expr, ExprKind, Span, UnaryOp};
use crate::expr::error::ExprError;
use crate::expr::value::{Env, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Simple `BTreeMap`-backed implementation of `Env` for evaluating expressions (spec §5.2).
#[derive(Debug, Clone, Default)]
pub struct MapEnv {
    values: BTreeMap<String, Value>,
}

impl MapEnv {
    /// Creates an empty `MapEnv`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a dotted key and `Value` into the environment.
    pub fn insert(&mut self, key: impl Into<String>, val: Value) -> &mut Self {
        self.values.insert(key.into(), val);
        self
    }
}

impl Env for MapEnv {
    fn get(&self, path: &str) -> Option<Value> {
        self.values.get(path).cloned()
    }
}

impl Env for BTreeMap<String, Value> {
    fn get(&self, path: &str) -> Option<Value> {
        BTreeMap::get(self, path).cloned()
    }
}

/// Extracts all dotted reference names (e.g. `"params.N"`, `"wall_s"`) referenced in `expr` (spec §5.2).
pub fn references(expr: &Expr) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    collect_refs(expr, &mut set);
    set
}

fn collect_refs(expr: &Expr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => {}
        ExprKind::Ref(name) => {
            out.insert(name.clone());
        }
        ExprKind::List(items) => {
            for item in items {
                collect_refs(item, out);
            }
        }
        ExprKind::Unary { operand, .. } => collect_refs(operand, out),
        ExprKind::Binary { left, right, .. } => {
            collect_refs(left, out);
            collect_refs(right, out);
        }
        ExprKind::Conditional {
            then_branch,
            cond,
            else_branch,
        } => {
            collect_refs(then_branch, out);
            collect_refs(cond, out);
            collect_refs(else_branch, out);
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                collect_refs(arg, out);
            }
        }
    }
}

/// Evaluates an `Expr` against `env` (spec §5.2).
pub fn eval(expr: &Expr, env: &dyn Env) -> Result<Value, ExprError> {
    match &expr.kind {
        ExprKind::Int(v) => Ok(Value::Int(*v)),
        ExprKind::Float(v) => Ok(Value::Float(*v)),
        ExprKind::Bool(v) => Ok(Value::Bool(*v)),
        ExprKind::Str(s) => Ok(Value::Str(s.clone())),
        ExprKind::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(eval(item, env)?);
            }
            Ok(Value::List(out))
        }
        ExprKind::Ref(name) => env.get(name).ok_or_else(|| ExprError::UnresolvedReference {
            name: name.clone(),
            span: expr.span,
        }),
        ExprKind::Unary { op, operand } => {
            let val = eval(operand, env)?;
            eval_unary(*op, val, expr.span)
        }
        ExprKind::Binary { op, left, right } => {
            // Short-circuit logical `and` / `or`
            match op {
                BinaryOp::And => {
                    let lval = eval(left, env)?;
                    let Value::Bool(lb) = lval else {
                        return Err(ExprError::TypeError {
                            span: left.span,
                            message: format!(
                                "`and` requires bool operands, found {}",
                                lval.type_name()
                            ),
                        });
                    };
                    if !lb {
                        return Ok(Value::Bool(false));
                    }
                    let rval = eval(right, env)?;
                    let Value::Bool(rb) = rval else {
                        return Err(ExprError::TypeError {
                            span: right.span,
                            message: format!(
                                "`and` requires bool operands, found {}",
                                rval.type_name()
                            ),
                        });
                    };
                    return Ok(Value::Bool(rb));
                }
                BinaryOp::Or => {
                    let lval = eval(left, env)?;
                    let Value::Bool(lb) = lval else {
                        return Err(ExprError::TypeError {
                            span: left.span,
                            message: format!(
                                "`or` requires bool operands, found {}",
                                lval.type_name()
                            ),
                        });
                    };
                    if lb {
                        return Ok(Value::Bool(true));
                    }
                    let rval = eval(right, env)?;
                    let Value::Bool(rb) = rval else {
                        return Err(ExprError::TypeError {
                            span: right.span,
                            message: format!(
                                "`or` requires bool operands, found {}",
                                rval.type_name()
                            ),
                        });
                    };
                    return Ok(Value::Bool(rb));
                }
                _ => {}
            }
            let lval = eval(left, env)?;
            let rval = eval(right, env)?;
            eval_binary(*op, lval, rval, expr.span)
        }
        ExprKind::Conditional {
            then_branch,
            cond,
            else_branch,
        } => {
            let cval = eval(cond, env)?;
            let Value::Bool(b) = cval else {
                return Err(ExprError::TypeError {
                    span: cond.span,
                    message: format!(
                        "conditional requires bool condition, found {}",
                        cval.type_name()
                    ),
                });
            };
            if b {
                eval(then_branch, env)
            } else {
                eval(else_branch, env)
            }
        }
        ExprKind::Call { func, args } => {
            let mut evaluated_args = Vec::with_capacity(args.len());
            for a in args {
                evaluated_args.push(eval(a, env)?);
            }
            eval_call(func, &evaluated_args, expr.span)
        }
    }
}

fn eval_unary(op: UnaryOp, val: Value, span: Span) -> Result<Value, ExprError> {
    match (op, val) {
        (UnaryOp::Neg, Value::Int(i)) => i.checked_neg().map(Value::Int).ok_or_else(|| {
            ExprError::TypeError {
                span,
                message: "integer negation overflow".to_string(),
            }
        }),
        (UnaryOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
        (UnaryOp::Pos, Value::Int(i)) => Ok(Value::Int(i)),
        (UnaryOp::Pos, Value::Float(f)) => Ok(Value::Float(f)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (op, other) => Err(ExprError::TypeError {
            span,
            message: format!("invalid operand type `{}` for unary {op:?}", other.type_name()),
        }),
    }
}

fn eval_binary(op: BinaryOp, left: Value, right: Value, span: Span) -> Result<Value, ExprError> {
    match op {
        BinaryOp::Add => match (left, right) {
            (Value::Int(a), Value::Int(b)) => a.checked_add(b).map(Value::Int).ok_or_else(|| {
                ExprError::TypeError {
                    span,
                    message: "integer addition overflow".to_string(),
                }
            }),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{a}{b}"))),
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b);
                Ok(Value::List(a))
            }
            (l, r) => Err(ExprError::TypeError {
                span,
                message: format!("cannot add `{}` and `{}`", l.type_name(), r.type_name()),
            }),
        },
        BinaryOp::Sub => match (left, right) {
            (Value::Int(a), Value::Int(b)) => a.checked_sub(b).map(Value::Int).ok_or_else(|| {
                ExprError::TypeError {
                    span,
                    message: "integer subtraction overflow".to_string(),
                }
            }),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
            (l, r) => Err(ExprError::TypeError {
                span,
                message: format!(
                    "cannot subtract `{}` from `{}`",
                    r.type_name(),
                    l.type_name()
                ),
            }),
        },
        BinaryOp::Mul => match (left, right) {
            (Value::Int(a), Value::Int(b)) => a.checked_mul(b).map(Value::Int).ok_or_else(|| {
                ExprError::TypeError {
                    span,
                    message: "integer multiplication overflow".to_string(),
                }
            }),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
            (l, r) => Err(ExprError::TypeError {
                span,
                message: format!(
                    "cannot multiply `{}` and `{}`",
                    l.type_name(),
                    r.type_name()
                ),
            }),
        },
        BinaryOp::Div => {
            // Spec §5.2: `/` is float division. Division by zero is an error.
            let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) else {
                return Err(ExprError::TypeError {
                    span,
                    message: format!(
                        "`/` requires numeric operands, found `{}` and `{}`",
                        left.type_name(),
                        right.type_name()
                    ),
                });
            };
            if b == 0.0 {
                return Err(ExprError::DivisionByZero { span });
            }
            Ok(Value::Float(a / b))
        }
        BinaryOp::FloorDiv => {
            // Spec §5.2: `//` is floor division. Division by zero is an error.
            match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    if b == 0 {
                        return Err(ExprError::DivisionByZero { span });
                    }
                    Ok(Value::Int(a.div_euclid(b) - if (a ^ b) < 0 && a % b != 0 {
                        // Python floor division semantics: floor(a / b)
                        // Note: div_euclid is non-negative remainder; compute exact floor(a/b):
                        0
                    } else {
                        0
                    }))
                    .map(|_| {
                        let d = a / b;
                        let r = a % b;
                        let floored = if (r != 0) && ((r < 0) != (b < 0)) {
                            d - 1
                        } else {
                            d
                        };
                        Value::Int(floored)
                    })
                }
                (l, r) => {
                    let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) else {
                        return Err(ExprError::TypeError {
                            span,
                            message: format!(
                                "`//` requires numeric operands, found `{}` and `{}`",
                                l.type_name(),
                                r.type_name()
                            ),
                        });
                    };
                    if b == 0.0 {
                        return Err(ExprError::DivisionByZero { span });
                    }
                    Ok(Value::Float((a / b).floor()))
                }
            }
        }
        BinaryOp::Mod => match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    return Err(ExprError::DivisionByZero { span });
                }
                let r = a % b;
                let m = if (r != 0) && ((r < 0) != (b < 0)) {
                    r + b
                } else {
                    r
                };
                Ok(Value::Int(m))
            }
            (l, r) => {
                let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) else {
                    return Err(ExprError::TypeError {
                        span,
                        message: format!(
                            "`%` requires numeric operands, found `{}` and `{}`",
                            l.type_name(),
                            r.type_name()
                        ),
                    });
                };
                if b == 0.0 {
                    return Err(ExprError::DivisionByZero { span });
                }
                let rem = a - (a / b).floor() * b;
                Ok(Value::Float(rem))
            }
        },
        BinaryOp::Pow => {
            // Spec §5.2: `**` with integer operands and a non-negative exponent stays integer.
            match (left, right) {
                (Value::Int(base), Value::Int(exp)) if exp >= 0 => {
                    let exp_u32 = u32::try_from(exp).map_err(|_| ExprError::TypeError {
                        span,
                        message: "integer exponent too large".to_string(),
                    })?;
                    base.checked_pow(exp_u32)
                        .map(Value::Int)
                        .ok_or_else(|| ExprError::TypeError {
                            span,
                            message: "integer exponentiation overflow".to_string(),
                        })
                }
                (l, r) => {
                    let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) else {
                        return Err(ExprError::TypeError {
                            span,
                            message: format!(
                                "`**` requires numeric operands, found `{}` and `{}`",
                                l.type_name(),
                                r.type_name()
                            ),
                        });
                    };
                    if a == 0.0 && b < 0.0 {
                        return Err(ExprError::DivisionByZero { span });
                    }
                    Ok(Value::Float(a.powf(b)))
                }
            }
        }
        BinaryOp::Eq => Ok(Value::Bool(values_equal(&left, &right))),
        BinaryOp::Ne => Ok(Value::Bool(!values_equal(&left, &right))),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            compare_values(op, &left, &right, span)
        }
        BinaryOp::In => match right {
            Value::List(items) => Ok(Value::Bool(items.iter().any(|item| values_equal(&left, item)))),
            Value::Str(haystack) => match left {
                Value::Str(needle) => Ok(Value::Bool(haystack.contains(&needle))),
                other => Err(ExprError::TypeError {
                    span,
                    message: format!(
                        "`in` on a string requires a string left operand, found `{}`",
                        other.type_name()
                    ),
                }),
            },
            other => Err(ExprError::TypeError {
                span,
                message: format!(
                    "right operand of `in` must be a list or string, found `{}`",
                    other.type_name()
                ),
            }),
        },
        BinaryOp::And | BinaryOp::Or => Err(ExprError::TypeError {
            span,
            message: "logical operator handled in short-circuit branch".to_string(),
        }),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Int(x), Value::Float(y)) => (*x as f64) == *y,
        (Value::Float(x), Value::Int(y)) => *x == (*y as f64),
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(xs), Value::List(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| values_equal(x, y))
        }
        _ => false,
    }
}

fn compare_values(op: BinaryOp, left: &Value, right: &Value, span: Span) -> Result<Value, ExprError> {
    let ord = match (left, right) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (l, r) => {
            let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) else {
                return Err(ExprError::TypeError {
                    span,
                    message: format!(
                        "cannot compare `{}` and `{}`",
                        l.type_name(),
                        r.type_name()
                    ),
                });
            };
            a.partial_cmp(&b).ok_or_else(|| ExprError::TypeError {
                span,
                message: "cannot compare NaN values".to_string(),
            })?
        }
    };

    let res = match op {
        BinaryOp::Lt => ord.is_lt(),
        BinaryOp::Le => ord.is_le(),
        BinaryOp::Gt => ord.is_gt(),
        BinaryOp::Ge => ord.is_ge(),
        _ => false,
    };
    Ok(Value::Bool(res))
}

fn eval_call(func: &str, args: &[Value], span: Span) -> Result<Value, ExprError> {
    let require_arity = |expected: usize| -> Result<(), ExprError> {
        if args.len() != expected {
            return Err(ExprError::FunctionError {
                func: func.to_string(),
                span,
                message: format!("expected {expected} argument(s), got {}", args.len()),
            });
        }
        Ok(())
    };

    let require_f64 = |val: &Value| -> Result<f64, ExprError> {
        val.as_f64().ok_or_else(|| ExprError::FunctionError {
            func: func.to_string(),
            span,
            message: format!("expected numeric argument, found `{}`", val.type_name()),
        })
    };

    match func {
        "abs" => {
            require_arity(1)?;
            match &args[0] {
                Value::Int(i) => Ok(Value::Int(i.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                other => Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: format!("expected numeric argument, found `{}`", other.type_name()),
                }),
            }
        }
        "sqrt" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            if x < 0.0 {
                return Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: "sqrt of negative number".to_string(),
                });
            }
            Ok(Value::Float(x.sqrt()))
        }
        "cbrt" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            let r = x.cbrt();
            // If input was an exact integer cube (e.g. `cbrt(512*8)` = 16), preserve clean integer if exact.
            if matches!(args[0], Value::Int(_)) {
                let rounded = r.round() as i64;
                if let Some(cubed) = rounded.checked_mul(rounded).and_then(|v| v.checked_mul(rounded)) {
                    if Value::Int(cubed) == args[0] {
                        return Ok(Value::Int(rounded));
                    }
                }
            }
            Ok(Value::Float(r))
        }
        "log" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            if x <= 0.0 {
                return Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: "log domain error (argument must be > 0)".to_string(),
                });
            }
            Ok(Value::Float(x.ln()))
        }
        "log2" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            if x <= 0.0 {
                return Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: "log2 domain error (argument must be > 0)".to_string(),
                });
            }
            Ok(Value::Float(x.log2()))
        }
        "log10" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            if x <= 0.0 {
                return Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: "log10 domain error (argument must be > 0)".to_string(),
                });
            }
            Ok(Value::Float(x.log10()))
        }
        "exp" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            Ok(Value::Float(x.exp()))
        }
        "floor" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            Ok(Value::Int(x.floor() as i64))
        }
        "ceil" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            Ok(Value::Int(x.ceil() as i64))
        }
        "round" => {
            require_arity(1)?;
            let x = require_f64(&args[0])?;
            Ok(Value::Int(x.round() as i64))
        }
        "int" => {
            require_arity(1)?;
            match &args[0] {
                Value::Int(i) => Ok(Value::Int(*i)),
                Value::Float(f) => Ok(Value::Int(*f as i64)),
                Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
                Value::Str(s) => s.trim().parse::<i64>().map(Value::Int).map_err(|_| {
                    ExprError::FunctionError {
                        func: func.to_string(),
                        span,
                        message: format!("cannot parse `{s}` as int"),
                    }
                }),
                other => Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: format!("cannot convert `{}` to int", other.type_name()),
                }),
            }
        }
        "float" => {
            require_arity(1)?;
            match &args[0] {
                Value::Int(i) => Ok(Value::Float(*i as f64)),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                Value::Str(s) => s.trim().parse::<f64>().map(Value::Float).map_err(|_| {
                    ExprError::FunctionError {
                        func: func.to_string(),
                        span,
                        message: format!("cannot parse `{s}` as float"),
                    }
                }),
                other => Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: format!("cannot convert `{}` to float", other.type_name()),
                }),
            }
        }
        "len" => {
            require_arity(1)?;
            match &args[0] {
                Value::List(items) => Ok(Value::Int(items.len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                other => Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: format!("`len` requires list or str, found `{}`", other.type_name()),
                }),
            }
        }
        "sum" => {
            let items: &[Value] = if args.len() == 1 {
                if let Value::List(list) = &args[0] {
                    list.as_slice()
                } else {
                    args
                }
            } else {
                args
            };
            if items.is_empty() {
                return Ok(Value::Int(0));
            }
            let mut all_int = true;
            let mut int_sum: i64 = 0;
            let mut float_sum: f64 = 0.0;
            for item in items {
                match item {
                    Value::Int(i) => {
                        int_sum = int_sum.saturating_add(*i);
                        float_sum += *i as f64;
                    }
                    Value::Float(f) => {
                        all_int = false;
                        float_sum += *f;
                    }
                    other => {
                        return Err(ExprError::FunctionError {
                            func: func.to_string(),
                            span,
                            message: format!(
                                "`sum` requires numeric elements, found `{}`",
                                other.type_name()
                            ),
                        });
                    }
                }
            }
            if all_int {
                Ok(Value::Int(int_sum))
            } else {
                Ok(Value::Float(float_sum))
            }
        }
        "min" | "max" => {
            let items: &[Value] = if args.len() == 1 {
                if let Value::List(list) = &args[0] {
                    list.as_slice()
                } else {
                    args
                }
            } else {
                args
            };
            let Some((first, rest)) = items.split_first() else {
                return Err(ExprError::FunctionError {
                    func: func.to_string(),
                    span,
                    message: format!("`{func}` requires at least one element"),
                });
            };
            let _ = require_f64(first)?;
            let mut best = first.clone();
            for item in rest {
                let cmp_op = if func == "min" {
                    BinaryOp::Lt
                } else {
                    BinaryOp::Gt
                };
                if let Value::Bool(true) = compare_values(cmp_op, item, &best, span)? {
                    best = item.clone();
                }
            }
            Ok(best)
        }
        _ => Err(ExprError::FunctionError {
            func: func.to_string(),
            span,
            message: format!("unknown function `{func}`"),
        }),
    }
}
