use super::*;
pub(super) fn pure(ty: ValueType) -> Fact {
    Fact {
        ty,
        effects: EffectRows::default(),
    }
}
pub(super) fn contains_local(ty: &ValueType) -> bool {
    match ty {
        ValueType::Local(_) => true,
        ValueType::List(inner) => contains_local(inner),
        ValueType::Record(fields) => fields.values().any(contains_local),
        ValueType::Function(a, b) => contains_local(a) || contains_local(b),
        _ => false,
    }
}
pub(super) fn numeric(ty: &ValueType) -> bool {
    matches!(ty, ValueType::Int | ValueType::Decimal | ValueType::Any)
}
pub(super) fn numeric_compatible(expected: &ValueType, actual: &ValueType) -> bool {
    numeric(expected) && numeric(actual)
}
pub(super) fn unify(left: ValueType, right: ValueType) -> ValueType {
    if left == ValueType::Never {
        right
    } else if right == ValueType::Never || left == right {
        left
    } else if numeric(&left) && numeric(&right) {
        ValueType::Decimal
    } else {
        ValueType::Any
    }
}
pub(super) fn ensure_assignable(
    source: &str,
    expr: &Expr,
    expected: &ValueType,
    actual: &ValueType,
) -> Result<()> {
    if expected == &ValueType::Any
        || actual == &ValueType::Any
        || expected == actual
        || numeric_compatible(expected, actual)
    {
        Ok(())
    } else {
        Err(Diagnostic::new(
            "TM3016",
            format!("expected {expected:?}, found {actual:?}"),
            expr.span,
            source,
        ))
    }
}
pub(super) fn apply_last_type(
    source: &str,
    expr: &Expr,
    target: ValueType,
    piped: &ValueType,
) -> Result<ValueType> {
    match target {
        ValueType::Function(argument, result) => match *result {
            result @ ValueType::Function(_, _) => Ok(ValueType::Function(
                argument,
                Box::new(apply_last_type(source, expr, result, piped)?),
            )),
            result => {
                ensure_assignable(source, expr, &argument, piped)?;
                Ok(result)
            }
        },
        ValueType::Any => Ok(ValueType::Any),
        other => Err(Diagnostic::new(
            "TM3017",
            format!("pipeline target is not callable: {other:?}"),
            expr.span,
            source,
        )),
    }
}
pub(super) fn type_from_term(term: &TypeTerm) -> ValueType {
    match term {
        TypeTerm::Named(name) => match name.as_str() {
            "String" | "Path" => ValueType::String,
            "Int" => ValueType::Int,
            "Decimal" => ValueType::Decimal,
            "Bool" => ValueType::Bool,
            other => ValueType::Local(other.into()),
        },
        TypeTerm::List(inner) => ValueType::List(Box::new(type_from_term(inner))),
        TypeTerm::Option(_) => ValueType::Any,
        TypeTerm::Record(fields) => ValueType::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), type_from_term(ty)))
                .collect(),
        ),
    }
}
pub(super) fn pattern_names(source: &str, pattern: &Pattern) -> Result<BTreeSet<String>> {
    fn walk(source: &str, pattern: &Pattern, names: &mut BTreeSet<String>) -> Result<()> {
        match &pattern.node {
            PatternKind::Bind(name) if !names.insert(name.clone()) => {
                return Err(Diagnostic::new(
                    "TM3018",
                    format!("duplicate pattern binding {name}"),
                    pattern.span,
                    source,
                ));
            }
            PatternKind::Bind(_) => {}
            PatternKind::Constructor {
                payload: Some(payload),
                ..
            } => walk(source, payload, names)?,
            PatternKind::List(values) => {
                for value in values {
                    walk(source, value, names)?;
                }
            }
            PatternKind::Cons { head, tail } => {
                walk(source, head, names)?;
                walk(source, tail, names)?;
            }
            PatternKind::Record { fields, .. } => {
                let mut field_names = BTreeSet::new();
                for (name, value) in fields {
                    if !field_names.insert(name) {
                        return Err(Diagnostic::new(
                            "TM3019",
                            format!("duplicate pattern field {name}"),
                            pattern.span,
                            source,
                        ));
                    }
                    walk(source, value, names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut names = BTreeSet::new();
    walk(source, pattern, &mut names)?;
    Ok(names)
}
pub(super) fn prelude_types() -> BTreeMap<String, ValueType> {
    let a = ValueType::Any;
    let list = ValueType::List(Box::new(a.clone()));
    let unary = |result| ValueType::Function(Box::new(a.clone()), Box::new(result));
    let binary = |result| ValueType::Function(Box::new(a.clone()), Box::new(unary(result)));
    BTreeMap::from([
        ("print".into(), unary(ValueType::Null)),
        ("display".into(), binary(ValueType::Null)),
        ("length".into(), unary(ValueType::Int)),
        ("sum".into(), unary(a.clone())),
        ("lines".into(), unary(list.clone())),
        ("split".into(), binary(list.clone())),
        ("contains".into(), binary(ValueType::Bool)),
        ("map".into(), binary(list.clone())),
        ("flatmap".into(), binary(list.clone())),
        ("filter".into(), binary(list.clone())),
        ("take".into(), binary(list.clone())),
        ("merge".into(), binary(a.clone())),
        ("table".into(), unary(a.clone())),
        ("where".into(), binary(a.clone())),
        ("select".into(), binary(a.clone())),
        (
            "sort_by".into(),
            ValueType::Function(Box::new(a.clone()), Box::new(binary(a.clone()))),
        ),
        ("group_by".into(), binary(a.clone())),
        ("aggregate".into(), binary(a.clone())),
        ("par".into(), unary(a.clone())),
        ("help".into(), unary(a.clone())),
        ("rethrow".into(), unary(ValueType::Never)),
        ("asc".into(), a.clone()),
        ("desc".into(), a.clone()),
    ])
}

pub(super) enum Interpolation {
    Name(String),
    Expression(String),
}

pub(super) fn interpolations(source: &str) -> Vec<Interpolation> {
    let chars: Vec<char> = source.chars().collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] != '#' {
            index += 1;
            continue;
        }
        if chars.get(index + 1) == Some(&'{') {
            let start = index + 2;
            let mut end = start;
            let mut depth = 1;
            while end < chars.len() && depth > 0 {
                match chars[end] {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    end += 1;
                }
            }
            if depth == 0 {
                result.push(Interpolation::Expression(
                    chars[start..end].iter().collect(),
                ));
                index = end + 1;
            } else {
                break;
            }
        } else {
            let start = index + 1;
            let mut end = start;
            while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                end += 1;
            }
            if end > start {
                result.push(Interpolation::Name(chars[start..end].iter().collect()));
                index = end;
            } else {
                index += 1;
            }
        }
    }
    result
}
