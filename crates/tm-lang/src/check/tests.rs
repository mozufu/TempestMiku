use super::*;
use crate::{EffectSignature, parse};

fn catalog() -> CapabilityCatalog {
    CapabilityCatalog::new()
        .scheme("workspace")
        .register(EffectSignature::new(
            "fs.read",
            ValueType::Uri,
            ValueType::String,
        ))
        .allow("fs.read")
}

#[test]
fn rejects_unknown_and_ungranted_authority_before_eval() {
    let source = "@fs.patch {patch: \"x\"}";
    let cell = parse(source).unwrap();
    assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3008");
    let catalog = catalog().register(EffectSignature::new(
        "fs.patch",
        ValueType::Any,
        ValueType::Null,
    ));
    assert_eq!(check(source, &cell, &catalog).unwrap_err().code, "TM3009");
}

#[test]
fn rejects_unknown_scheme_and_non_exhaustive_closed_match() {
    let source = "@fs.read mystery:path";
    let cell = parse(source).unwrap();
    assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3005");
    let source = "match true { | true -> 1 }";
    let cell = parse(source).unwrap();
    assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3015");
}

#[test]
fn rejects_duplicate_records_and_patterns() {
    for source in ["{x: 1, x: 2}", "let {x, x} = {x: 1}"] {
        let cell = parse(source).unwrap();
        assert!(check(source, &cell, &catalog()).is_err());
    }
}

#[test]
fn infers_separate_authority_error_and_presentation_rows() {
    let source = "@fs.read workspace:a |> display {kind: \"text\"}";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert_eq!(
        checked.effects.authority,
        BTreeSet::from(["fs.read".into()])
    );
    assert_eq!(
        checked.effects.presentation,
        BTreeSet::from(["display".into()])
    );

    let source = "1 / 0";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert!(checked.effects.errors.contains("DivisionByZero"));
}

#[test]
fn rethrow_restores_the_handled_error_row() {
    let source = "handle 1 / 0 with error { | DivisionByZero _ -> rethrow null }";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert_eq!(
        checked.effects.errors,
        BTreeSet::from(["DivisionByZero".into()])
    );

    let source = "handle 1 / 0 with error { | error -> 0 }";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert!(checked.effects.errors.is_empty());
}

#[test]
fn rejects_local_sum_type_escape() {
    let source = "let escaped = do { type Local = | Only; Only }";
    let cell = parse(source).unwrap();
    assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3001");
}

#[test]
fn preserves_types_for_destructured_bindings() {
    let source = "let {a} = {a: 1}; a + 1";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert_eq!(checked.bindings.get("a"), Some(&ValueType::Int));
    assert_eq!(checked.result_type, ValueType::Int);

    let source = "let head :: tail = [1, 2]; head + length tail";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert_eq!(checked.bindings.get("head"), Some(&ValueType::Int));
    assert_eq!(
        checked.bindings.get("tail"),
        Some(&ValueType::List(Box::new(ValueType::Int)))
    );
    assert_eq!(checked.result_type, ValueType::Int);
}

#[test]
fn sum_is_available_in_the_checked_prelude() {
    let source = "let values = [1, 2]; values |> sum";
    assert!(check(source, &parse(source).unwrap(), &catalog()).is_ok());
}

#[test]
fn retains_errors_not_covered_by_handle_arms() {
    let mut signature = EffectSignature::new("test.risky", ValueType::Any, ValueType::Null);
    signature.errors = BTreeSet::from(["ApprovalDeniedError".into(), "InvalidPathError".into()]);
    let catalog = CapabilityCatalog::new()
        .register(signature)
        .allow("test.risky");
    let source = "handle @test.risky null with error { | ApprovalDeniedError _ -> null }";
    let checked = check(source, &parse(source).unwrap(), &catalog).unwrap();

    assert_eq!(
        checked.effects.errors,
        BTreeSet::from(["InvalidPathError".into()])
    );
}

#[test]
fn rejects_statically_impossible_exact_patterns() {
    for source in [
        "let {a} = {a: 1, b: 2}; a",
        "let [value] = 1; value",
        "let {missing, ...} = {present: 1}; missing",
    ] {
        let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
        assert_eq!(error.code, "TM3022", "{source}: {error:?}");
    }

    let source = "let {a, ...} = {a: 1, b: 2}; a";
    assert!(check(source, &parse(source).unwrap(), &catalog()).is_ok());
}

#[test]
fn rejects_invalid_ordered_comparisons_and_cons_tails() {
    for source in ["true < false", "1 < 2 < 3", "1 :: 2"] {
        let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
        assert_eq!(error.code, "TM3014", "{source}");
    }

    for source in ["1 < 2", "1 :: [2, 3]"] {
        assert!(
            check(source, &parse(source).unwrap(), &catalog()).is_ok(),
            "{source}"
        );
    }
}

#[test]
fn under_applied_data_last_pipeline_remains_a_function() {
    let source = "[1, 2] |> map";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert!(matches!(checked.result_type, ValueType::Function(_, _)));
}

#[test]
fn lexical_deltas_restore_shadowed_bindings_and_local_types() {
    let source = "let value = 1; do { let value = true; value }; value + 1";
    let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
    assert_eq!(checked.result_type, ValueType::Int);

    let source = "do { type Local = | Only; Only }; Only";
    let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
    assert_eq!(error.code, "TM3007");
}

#[test]
fn lexical_deltas_are_rolled_back_when_a_scope_errors() {
    let source = "do { type Local = | Only; let leaked = Only; missing }";
    let cell = parse(source).unwrap();
    let catalog = catalog();
    let mut checker = Checker::new_bounded(
        source,
        &catalog,
        DEFAULT_MAX_SOURCE_BYTES,
        DEFAULT_MAX_SYNTAX_NODES,
        DEFAULT_MAX_PARSE_DEPTH,
    );

    assert_eq!(
        checker.form(&cell.forms[0], true).unwrap_err().code,
        "TM3006"
    );
    assert!(!checker.env.contains_key("leaked"));
    assert!(!checker.local_types.contains_key("Local"));
    assert!(!checker.constructors.contains_key("Only"));
    assert!(checker.env.undo.is_empty());
    assert!(checker.local_types.undo.is_empty());
    assert!(checker.constructors.undo.is_empty());
    assert!(checker.env.scopes.is_empty());
    assert!(checker.local_types.scopes.is_empty());
    assert!(checker.constructors.scopes.is_empty());
}

#[test]
fn many_function_scopes_do_not_copy_the_accumulated_environment() {
    let mut source = String::new();
    for index in 0..2_000 {
        source.push_str(&format!("fun f{index} item = item;"));
    }
    source.push_str("f1999 1");

    let checked = check(&source, &parse(&source).unwrap(), &catalog()).unwrap();
    assert_eq!(checked.bindings.len(), 2_000);
    assert_eq!(checked.result_type, ValueType::Any);
}

#[test]
fn deeply_nested_scopes_restore_each_lexical_delta() {
    let depth = 48;
    let mut source = String::new();
    for index in 0..depth {
        let value = if index == 0 {
            "1".into()
        } else {
            format!("v{}", index - 1)
        };
        source.push_str(&format!("do {{ let v{index} = {value}; "));
    }
    source.push_str(&format!("v{}", depth - 1));
    for _ in 0..depth {
        source.push_str(" }");
    }

    let checked = check(&source, &parse(&source).unwrap(), &catalog()).unwrap();
    assert_eq!(checked.result_type, ValueType::Int);
}

#[test]
fn recursively_nested_interpolations_share_the_check_depth_budget() {
    let mut source = "1".to_string();
    for _ in 0..12 {
        source = serde_json::to_string(&format!("#{{{source}}}")).unwrap();
    }
    let cell = parse(&source).unwrap();
    let max_source_bytes = source.len().saturating_mul(4);

    let error = check_with_bindings_bounded(
        &source,
        &cell,
        &catalog(),
        std::iter::empty::<String>(),
        max_source_bytes,
        1_024,
        8,
    )
    .unwrap_err();

    assert_eq!(error.code, "TM2021", "{error:?}");
}

#[test]
fn flat_function_arity_is_rejected_before_building_nested_function_types() {
    let parameters = std::iter::repeat_n("_", 30_000)
        .collect::<Vec<_>>()
        .join(" ");
    for source in [
        format!("fun too_wide {parameters} = null"),
        format!("let too_wide = fun {parameters} -> null"),
    ] {
        let cell = parse(&source).unwrap();
        let error = check(&source, &cell, &catalog()).unwrap_err();
        assert_eq!(error.code, "TM3023", "{error:?}");
    }
}
