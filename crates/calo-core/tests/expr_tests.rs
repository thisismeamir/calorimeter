use calo_core::expr::{
    eval, interpolate, parse, references, render_template, ExprError, MapEnv, Span, Value,
};
use calo_schema::ScalarValue;
use std::collections::{BTreeMap, BTreeSet};

fn eval_str(src: &str, env: &MapEnv) -> Result<Value, ExprError> {
    let expr = parse(src)?;
    eval(&expr, env)
}

#[test]
fn precedence_and_associativity() {
    let env = MapEnv::new();

    // Right-associative exponentiation: 2 ** 3 ** 2 == 2 ** 9 == 512
    assert_eq!(eval_str("2**3**2", &env).unwrap(), Value::Int(512));

    // Unary minus binds looser than `**` on the left, and works in the exponent
    assert_eq!(eval_str("-2**2", &env).unwrap(), Value::Int(-4));
    assert_eq!(eval_str("(-2)**2", &env).unwrap(), Value::Int(4));
    assert_eq!(eval_str("2**-1", &env).unwrap(), Value::Float(0.5));

    // Standard arithmetic precedence
    assert_eq!(eval_str("2 + 3 * 4", &env).unwrap(), Value::Int(14));
    assert_eq!(eval_str("(2 + 3) * 4", &env).unwrap(), Value::Int(20));

    // `/` is float division, `//` is floor division
    assert_eq!(eval_str("7 / 2", &env).unwrap(), Value::Float(3.5));
    assert_eq!(eval_str("7 // 2", &env).unwrap(), Value::Int(3));
    assert_eq!(eval_str("-7 // 2", &env).unwrap(), Value::Int(-4));
    assert_eq!(eval_str("7 % 4", &env).unwrap(), Value::Int(3));
}

#[test]
fn division_by_zero_is_error_with_span() {
    let env = MapEnv::new();
    let err = eval_str("10 / 0", &env).expect_err("div by zero must fail");
    assert_eq!(
        err,
        ExprError::DivisionByZero {
            span: Span::new(0, 6)
        }
    );

    let err_floor = eval_str("10 // (5 - 5)", &env).expect_err("floor div by zero must fail");
    assert_eq!(
        err_floor,
        ExprError::DivisionByZero {
            span: Span::new(0, 13)
        }
    );
}

#[test]
fn conditionals_and_in_operator() {
    let mut env = MapEnv::new();
    env.insert("params.N", Value::Int(256));
    env.insert("params.backend", Value::Str("cuda".to_string()));

    assert_eq!(
        eval_str("'large' if params.N >= 128 else 'small'", &env).unwrap(),
        Value::Str("large".to_string())
    );
    assert_eq!(
        eval_str("10 if false else 20 if true else 30", &env).unwrap(),
        Value::Int(20)
    );

    // `in` on lists and strings
    assert_eq!(
        eval_str("params.N in [64, 128, 256]", &env).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_str("params.N in [64, 128]", &env).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        eval_str("'cu' in params.backend", &env).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn builtin_functions_and_cbrt() {
    let mut env = MapEnv::new();
    env.insert("ranks", Value::Int(8));

    // Spec weak scaling example: `128 * cbrt(ranks)` and `cbrt(512*8)`
    assert_eq!(eval_str("cbrt(512*8)", &env).unwrap(), Value::Int(16));
    assert_eq!(
        eval_str("128 * cbrt(ranks)", &env).unwrap(),
        Value::Int(256)
    );

    // Every built-in function
    assert_eq!(eval_str("min(5, 2, 9)", &env).unwrap(), Value::Int(2));
    assert_eq!(eval_str("min([5, 2, 9])", &env).unwrap(), Value::Int(2));
    assert_eq!(eval_str("max(5, 2, 9)", &env).unwrap(), Value::Int(9));
    assert_eq!(eval_str("abs(-42)", &env).unwrap(), Value::Int(42));
    assert_eq!(eval_str("sqrt(16)", &env).unwrap(), Value::Float(4.0));
    assert_eq!(eval_str("log(exp(1.0))", &env).unwrap(), Value::Float(1.0));
    assert_eq!(eval_str("log2(8)", &env).unwrap(), Value::Float(3.0));
    assert_eq!(eval_str("log10(1000)", &env).unwrap(), Value::Float(3.0));
    assert_eq!(eval_str("floor(3.9)", &env).unwrap(), Value::Int(3));
    assert_eq!(eval_str("ceil(3.1)", &env).unwrap(), Value::Int(4));
    assert_eq!(eval_str("round(3.6)", &env).unwrap(), Value::Int(4));
    assert_eq!(eval_str("int('42')", &env).unwrap(), Value::Int(42));
    assert_eq!(eval_str("float(4)", &env).unwrap(), Value::Float(4.0));
    assert_eq!(eval_str("len([10, 20, 30])", &env).unwrap(), Value::Int(3));
    assert_eq!(eval_str("len('calo')", &env).unwrap(), Value::Int(4));
    assert_eq!(eval_str("sum([1, 2, 3, 4])", &env).unwrap(), Value::Int(10));
}

#[test]
fn unresolved_reference_carries_exact_byte_span() {
    let mut env = MapEnv::new();
    env.insert("params.N", Value::Int(128));

    let src = "params.N + machine.node.gpu.per_node";
    let err = eval_str(src, &env).expect_err("must fail on missing reference");
    assert_eq!(
        err,
        ExprError::UnresolvedReference {
            name: "machine.node.gpu.per_node".to_string(),
            span: Span::new(11, 36),
        }
    );
    assert_eq!(
        &src[err.span().start..err.span().end],
        "machine.node.gpu.per_node"
    );
}

#[test]
fn references_extraction() {
    let expr = parse(
        "params.N**3 * reported.steps / phase.main if machine.node.gpu.per_node >= 4 else region.fft.incl_s",
    )
    .unwrap();
    let refs = references(&expr);
    let expected = BTreeSet::from([
        "params.N".to_string(),
        "reported.steps".to_string(),
        "phase.main".to_string(),
        "machine.node.gpu.per_node".to_string(),
        "region.fft.incl_s".to_string(),
    ]);
    assert_eq!(refs, expected);
}

#[test]
fn interpolation_and_template_rendering() {
    let mut env = MapEnv::new();
    env.insert("params.nodes", Value::Int(4));
    env.insert("params.rpn", Value::Int(8));
    env.insert("files.input", Value::Str("run/model.in".to_string()));

    let cmd = interpolate(
        "srun -N ${{ params.nodes }} -n ${{ params.nodes * params.rpn }} ./model input=${{ files.input }}",
        &env,
    )
    .unwrap();
    assert_eq!(cmd, "srun -N 4 -n 32 ./model input=run/model.in");

    let vars = BTreeMap::from([
        ("N".to_string(), ScalarValue::Int(256)),
        ("dt".to_string(), ScalarValue::Float(0.01)),
    ]);
    let rendered = render_template("grid_size = {{ N }}\ntime_step = {{ dt }}\n", &vars).unwrap();
    assert_eq!(rendered, "grid_size = 256\ntime_step = 0.01\n");

    // Missing template variable is an error (R-STP-8)
    let missing_err = render_template("N = {{ N }}, k = {{ missing_var }}", &vars)
        .expect_err("missing template var must fail");
    assert!(matches!(
        missing_err,
        ExprError::UnresolvedReference { name, .. } if name == "missing_var"
    ));

    // Logic blocks in templates are rejected
    assert!(render_template("{% if N > 64 %}big{% endif %}", &vars).is_err());
}

#[test]
fn rejects_statements_and_method_calls() {
    // Assignment statement rejected
    assert!(parse("params.N = 128").is_err());
    // Semicolon statement separator rejected
    assert!(parse("1 + 2; 3").is_err());
    // Attribute/method call on dotted path rejected
    assert!(parse("params.N.to_string()").is_err());
    assert!(parse("os.system('sh')").is_err());
    // Calling an arbitrary expression rejected
    assert!(parse("(1 + 2)(3)").is_err());
    // Unknown function rejected
    assert!(parse("eval('1+1')").is_err());
}
