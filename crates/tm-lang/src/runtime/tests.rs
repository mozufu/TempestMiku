#[test]
fn retained_environment_deduplicates_callable_graphs_but_charges_alias_slots() {
    let max_depth = RuntimeLimits::default().runtime_depth;
    let callable = Rc::new(Callable::Builtin {
        name: "fixture".into(),
        args: vec![Value::String("payload".repeat(32))],
        arity: 2,
    });
    let mut environment = Environment::from([("first".into(), Value::Callable(callable.clone()))]);
    let one_size = environment_size_bounded(&environment, usize::MAX, max_depth);
    environment.insert("alias".into(), Value::Callable(callable));
    let alias_size = environment_size_bounded(&environment, usize::MAX, max_depth);
    environment.insert(
        "distinct".into(),
        Value::Callable(Rc::new(Callable::Builtin {
            name: "fixture".into(),
            args: vec![Value::String("payload".repeat(32))],
            arity: 2,
        })),
    );
    let distinct_size = environment_size_bounded(&environment, usize::MAX, max_depth);

    assert!(alias_size > one_size, "the alias map slot must be charged");
    assert!(
        distinct_size - alias_size > alias_size - one_size,
        "a distinct callable graph must cost more than an Rc alias"
    );
}

#[test]
fn value_budget_charges_scalar_container_slots_and_json_trees() {
    let max_depth = RuntimeLimits::default().runtime_depth;
    let values = Value::List(vec![Value::Null; 256]);
    assert!(value_size_bounded(&values, 8 * 1024, max_depth) > 8 * 1024);
    let json_values = JsonValue::Array(vec![JsonValue::Null; 256]);
    assert!(json_value_size_bounded(&json_values, 8 * 1024, max_depth) > 8 * 1024);
}

#[test]
fn retained_sizing_and_rendering_reject_excessive_value_depth() {
    let deep = (0..64).fold(Value::Null, |value, _| Value::List(vec![value]));
    let limit = 1024 * 1024;

    assert!(value_size_bounded(&deep, limit, 16) > limit);
    assert!(value_json_bounded(&deep, limit, true, 16).is_none());
}
use super::*;
