use super::*;
pub(super) fn error_name(error: &RuntimeError) -> &str {
    match error {
        RuntimeError::Effect { name, .. } => name,
        RuntimeError::Type(_) => "TypeError",
        RuntimeError::Limit(_) => "ResourceLimitError",
        RuntimeError::Persistence(_) => "RuntimePersistenceError",
        RuntimeError::Cancelled => "CancellationError",
        RuntimeError::Diagnostic(_) => "DiagnosticError",
    }
}

pub(super) fn logical_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        _ => unreachable!("logical_name only accepts boolean operators"),
    }
}
pub(super) fn prelude() -> Environment {
    [
        ("print", 1),
        ("display", 2),
        ("length", 1),
        ("lines", 1),
        ("split", 2),
        ("contains", 2),
        ("take", 2),
        ("merge", 2),
        ("map", 2),
        ("flatmap", 2),
        ("filter", 2),
        ("where", 2),
        ("select", 2),
        ("sort_by", 3),
        ("group_by", 2),
        ("aggregate", 2),
        ("sum", 1),
        ("table", 1),
        ("par", 1),
        ("help", 1),
        ("rethrow", 1),
    ]
    .into_iter()
    .map(|(name, arity)| {
        (
            name.into(),
            Value::Callable(Rc::new(Callable::Builtin {
                name: name.into(),
                args: Vec::new(),
                arity,
            })),
        )
    })
    .chain([
        ("asc".into(), Value::String("asc".into())),
        ("desc".into(), Value::String("desc".into())),
    ])
    .collect()
}
pub(super) fn list_call_args(args: Vec<Value>, name: &str) -> RuntimeResult<(Value, Vec<Value>)> {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (Some(function @ Value::Callable(_)), Some(Value::List(values))) => Ok((function, values)),
        _ => Err(RuntimeError::Type(format!(
            "{name} requires function and list"
        ))),
    }
}
pub(super) fn match_pattern_counted(
    pattern: &Pattern,
    value: &Value,
    max_visits: usize,
    max_depth: usize,
) -> (Option<Environment>, usize) {
    let mut env = Environment::new();
    let mut visits = 0usize;
    fn visit(visits: &mut usize, max_visits: usize) -> bool {
        if *visits >= max_visits {
            *visits = visits.saturating_add(1);
            false
        } else {
            *visits += 1;
            true
        }
    }
    fn walk_list(
        pattern: &Pattern,
        values: &[Value],
        env: &mut Environment,
        visits: &mut usize,
        max_visits: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            *visits = max_visits.saturating_add(1);
            return false;
        }
        if !visit(visits, max_visits) {
            return false;
        }
        match &pattern.node {
            PatternKind::Wildcard => true,
            PatternKind::Bind(name) => {
                // A bound tail must own a value, but nested cons matching otherwise walks the
                // original slice and never repeatedly clones progressively smaller tails.
                let next = visits.saturating_add(values.len());
                if next > max_visits {
                    *visits = max_visits.saturating_add(1);
                    return false;
                }
                *visits = next;
                env.insert(name.clone(), Value::List(values.to_vec()));
                true
            }
            PatternKind::List(patterns) if patterns.len() == values.len() => {
                patterns.iter().zip(values).all(|(pattern, value)| {
                    walk(
                        pattern,
                        value,
                        env,
                        visits,
                        max_visits,
                        depth + 1,
                        max_depth,
                    )
                })
            }
            PatternKind::Cons { head, tail } if !values.is_empty() => {
                walk(
                    head,
                    &values[0],
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                ) && walk_list(
                    tail,
                    &values[1..],
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                )
            }
            _ => false,
        }
    }

    fn walk(
        pattern: &Pattern,
        value: &Value,
        env: &mut Environment,
        visits: &mut usize,
        max_visits: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            *visits = max_visits.saturating_add(1);
            return false;
        }
        if !visit(visits, max_visits) {
            return false;
        }
        match (&pattern.node, value) {
            (PatternKind::Wildcard, _) => true,
            (PatternKind::Bind(name), value) => {
                env.insert(name.clone(), value.clone());
                true
            }
            (PatternKind::String(a), Value::String(b)) => a == b,
            (PatternKind::Int(a), Value::Int(b)) => a == b,
            (PatternKind::Bool(a), Value::Bool(b)) => a == b,
            (PatternKind::Null, Value::Null) => true,
            (
                PatternKind::Constructor { name, payload },
                Value::Tagged {
                    name: actual,
                    payload: value,
                },
            ) if name == actual => match (payload, value) {
                (None, None) => true,
                (Some(pattern), Some(value)) => walk(
                    pattern,
                    value,
                    env,
                    visits,
                    max_visits,
                    depth + 1,
                    max_depth,
                ),
                _ => false,
            },
            (PatternKind::List(patterns), Value::List(values))
                if patterns.len() == values.len() =>
            {
                patterns.iter().zip(values).all(|(pattern, value)| {
                    walk(
                        pattern,
                        value,
                        env,
                        visits,
                        max_visits,
                        depth + 1,
                        max_depth,
                    )
                })
            }
            (PatternKind::Cons { .. }, Value::List(values)) => walk_list(
                pattern,
                values,
                env,
                visits,
                max_visits,
                depth + 1,
                max_depth,
            ),
            (PatternKind::Record { fields, rest }, Value::Record(values)) => {
                (*rest || fields.len() == values.len())
                    && fields.iter().all(|(name, pattern)| {
                        values.get(name).is_some_and(|value| {
                            walk(
                                pattern,
                                value,
                                env,
                                visits,
                                max_visits,
                                depth + 1,
                                max_depth,
                            )
                        })
                    })
            }
            _ => false,
        }
    }
    let matched = walk(
        pattern,
        value,
        &mut env,
        &mut visits,
        max_visits,
        0,
        max_depth,
    );
    (matched.then_some(env), visits)
}
pub(super) fn unary(op: UnaryOp, value: Value) -> RuntimeResult<Value> {
    match (op, value) {
        (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnaryOp::Negate, Value::Int(value)) => value
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type("integer overflow in negation".into())),
        (UnaryOp::Negate, Value::Decimal(value)) => Ok(Value::Decimal(-value)),
        (_, value) => Err(RuntimeError::Type(format!(
            "invalid unary operand {}",
            value.kind()
        ))),
    }
}
pub(super) fn binary(op: BinaryOp, left: Value, right: Value) -> RuntimeResult<Value> {
    use BinaryOp::*;
    match op {
        Equal => Ok(Value::Bool(left == right)),
        NotEqual => Ok(Value::Bool(left != right)),
        And | Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(match op {
                And => left && right,
                Or => left || right,
                _ => unreachable!(),
            })),
            (left, right) => Err(RuntimeError::Type(format!(
                "{} requires boolean operands; found {} and {}",
                logical_name(op),
                left.kind(),
                right.kind()
            ))),
        },
        Cons => match right {
            Value::List(mut values) => {
                values.insert(0, left);
                Ok(Value::List(values))
            }
            _ => Err(RuntimeError::Type(":: tail must be a list".into())),
        },
        Add => match (left, right) {
            (Value::String(mut left), Value::String(right)) => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (left, right) => {
                numeric_binary(left, right, i64::checked_add, |a, b| a + b, "addition")
            }
        },
        Subtract => numeric_binary(left, right, i64::checked_sub, |a, b| a - b, "subtraction"),
        Multiply => numeric_binary(
            left,
            right,
            i64::checked_mul,
            |a, b| a * b,
            "multiplication",
        ),
        Divide if is_numeric_zero(&right) => Err(RuntimeError::Effect {
            name: "DivisionByZero".into(),
            message: "division by zero".into(),
            payload: Some(json!({"operation": "division"})),
        }),
        Divide => numeric_binary(left, right, i64::checked_div, |a, b| a / b, "division"),
        Modulo if is_numeric_zero(&right) => Err(RuntimeError::Effect {
            name: "DivisionByZero".into(),
            message: "modulo by zero".into(),
            payload: Some(json!({"operation": "modulo"})),
        }),
        Modulo => numeric_binary(left, right, i64::checked_rem, |a, b| a % b, "modulo"),
        Less | LessEqual | Greater | GreaterEqual => {
            let (a, b) = numbers(left, right)?;
            Ok(Value::Bool(match op {
                Less => a < b,
                LessEqual => a <= b,
                Greater => a > b,
                GreaterEqual => a >= b,
                _ => unreachable!(),
            }))
        }
    }
}
pub(super) fn is_numeric_zero(value: &Value) -> bool {
    match value {
        Value::Int(value) => *value == 0,
        Value::Decimal(value) => *value == 0.0,
        _ => false,
    }
}
pub(super) fn numeric_binary(
    left: Value,
    right: Value,
    ints: impl FnOnce(i64, i64) -> Option<i64>,
    decimals: impl FnOnce(f64, f64) -> f64,
    operation: &str,
) -> RuntimeResult<Value> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => ints(a, b)
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type(format!("integer overflow in {operation}"))),
        (a, b) => {
            let (a, b) = numbers(a, b)?;
            Ok(Value::Decimal(decimals(a, b)))
        }
    }
}
pub(super) fn numbers(left: Value, right: Value) -> RuntimeResult<(f64, f64)> {
    let number = |value| match value {
        Value::Int(v) => Some(v as f64),
        Value::Decimal(v) => Some(v),
        _ => None,
    };
    let a = number(left).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    let b = number(right).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    Ok((a, b))
}

pub(super) fn normalize_effect_args(name: &str, argument: Value) -> JsonValue {
    match (name, argument) {
        ("fs.read", Value::String(path) | Value::Uri(path)) => json!({"path": path}),
        (_, argument) => argument.to_json(),
    }
}

pub(super) const MAP_SLOT_OVERHEAD: usize =
    std::mem::size_of::<String>() + 3 * std::mem::size_of::<usize>();
