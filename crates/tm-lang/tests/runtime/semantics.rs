use super::*;

#[tokio::test(flavor = "current_thread")]
async fn integer_overflow_returns_diagnostics_without_poisoning_the_session() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();

    for (source, operation) in [
        ("9223372036854775807 + 1", "addition"),
        ("(-9223372036854775807 - 1) / -1", "division"),
        ("-(-9223372036854775807 - 1)", "negation"),
    ] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains(&format!("integer overflow in {operation}"))),
            "{source}: {output:?}"
        );
    }

    assert_eq!(
        session
            .eval("40 + 2", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(42))
    );

    assert_eq!(
        session
            .eval("[9223372036854775807, -1] |> sum", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(9223372036854775806_i64))
    );
    let overflow = session
        .eval("[9223372036854775807, 1] |> sum", CellBudget::default())
        .await
        .unwrap();
    assert!(
        overflow
            .error
            .as_deref()
            .is_some_and(|error| error.contains("integer overflow in sum")),
        "{overflow:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dynamic_boolean_operands_are_checked_at_runtime() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    session
        .eval("let dynamic = \"not-a-bool\"", CellBudget::default())
        .await
        .unwrap();

    for source in ["dynamic or true", "false or dynamic"] {
        let output = session.eval(source, CellBudget::default()).await.unwrap();
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("or requires boolean operands")),
            "{source}: {output:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn top_level_type_constructors_do_not_escape_their_cell() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let local = session
        .eval("type Choice = | No; No", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(local.result, Some(json!({"tag": "No"})), "{local:?}");

    let escaped = session.eval("No", CellBudget::default()).await.unwrap();
    assert!(
        escaped
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown constructor No")),
        "{escaped:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn host_error_handlers_receive_structured_payload_fields() {
    let events = Arc::new(Events::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut session = sandbox(events, calls)
        .open(SessionConfig::default())
        .await
        .unwrap();
    let output = session
        .eval(
            "handle (@fs.patch {patch: \"denied\"}) with error { | CapabilityDeniedError {capability, ...} -> capability }",
            CellBudget::default(),
        )
        .await
        .unwrap();

    assert_eq!(output.result, Some(json!("fs.patch")), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn functions_patterns_and_table_prelude_match_frozen_semantics() {
    let mut session = TmSandbox::new(TmSandboxOptions {
        grants: CapabilityGrants::default(),
        ..TmSandboxOptions::default()
    })
    .open(SessionConfig::default())
    .await
    .unwrap();
    let factorial = session
        .eval(
            "fun fact n = if n == 0 then 1 else n * fact (n - 1);\nfact 5",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(factorial.result, Some(json!(120)), "{factorial:?}");

    let partial_map = session
        .eval(
            "([1, 2] |> map) (fun value -> value + 1)",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(partial_map.result, Some(json!([2, 3])), "{partial_map:?}");

    let partial_display = session
        .eval("([1] |> display) {kind: \"json\"}", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(
        partial_display.result,
        Some(json!([1])),
        "{partial_display:?}"
    );

    let block_scope = session
        .eval(
            "let outer = 7; handle do { let outer = 1; rethrow \"boom\" } with error { | Rethrown _ -> outer }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(block_scope.result, Some(json!(7)), "{block_scope:?}");

    let lexical_closure = session
        .eval(
            "fun helper value = value + 1; fun captured value = helper value; fun helper value = value + 100; captured 1",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        lexical_closure.result,
        Some(json!(2)),
        "{lexical_closure:?}"
    );

    let pattern_error = session
        .eval(
            "let x = 1; let f = fun [a] -> a; let x = 2; handle f 1 with error { | TypeError _ -> x }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(pattern_error.result, Some(json!(2)), "{pattern_error:?}");
    assert_eq!(
        session
            .eval("x", CellBudget::default())
            .await
            .unwrap()
            .result,
        Some(json!(2))
    );

    let none = session
        .eval(
            "match None { | Some value -> value | None -> 42 }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(none.result, Some(json!(42)), "{none:?}");

    let local = session
        .eval(
            "do { type Choice = | Yes Int | No; let choice = Yes 3; match choice { | Yes value -> \"value #{value + 1}\" | No -> \"none\" } }",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(local.result, Some(json!("value 4")), "{local:?}");
    let escaped = session
        .eval("\"literal \\# hash\"", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(escaped.result, Some(json!("literal # hash")), "{escaped:?}");
    let concatenated = session
        .eval("\"tempest\" + \"miku\"", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(concatenated.result, Some(json!("tempestmiku")));

    let summed = session
        .eval("let values = [1, 2]; values |> sum", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(summed.result, Some(json!(3)), "{summed:?}");

    let lexical_row = session
        .eval(
            "let age = 100; table [{age:10}] |> select {lexical: age, column: row.age}",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        lexical_row.result,
        Some(json!([{"column": 10, "lexical": 100}])),
        "{lexical_row:?}"
    );

    let numeric_sort = session
        .eval(
            "table [{n: 2}, {n: 10}, {n: 1}] |> sort_by n asc",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        numeric_sort.result,
        Some(json!([{"n": 1}, {"n": 2}, {"n": 10}])),
        "{numeric_sort:?}"
    );

    let typed_groups = session
        .eval(
            "table [{key: 1}, {key: \"1\"}, {key: true}, {key: \"true\"}] |> group_by key",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        typed_groups.result,
        Some(json!([
            {"key": true, "rows": [{"key": true}]},
            {"key": 1, "rows": [{"key": 1}]},
            {"key": "1", "rows": [{"key": "1"}]},
            {"key": "true", "rows": [{"key": "true"}]}
        ])),
        "{typed_groups:?}"
    );

    let table = session
        .eval(
            "table [{file: \"a\", todos: 2}, {file: \"a\", todos: 3}, {file: \"b\", todos: 1}] |> group_by [file] |> aggregate {file: key, todos: sum todos, hits: count} |> sort_by hits desc",
            CellBudget::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        table.result,
        Some(json!([
            {"file": ["a"], "hits": 2, "todos": 5},
            {"file": ["b"], "hits": 1, "todos": 1}
        ])),
        "{table:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn display_is_bounded_and_help_returns_capability_docs() {
    let events = Arc::new(Events::default());
    let mut session = sandbox(Arc::clone(&events), Arc::new(AtomicUsize::new(0)))
        .open(SessionConfig::default())
        .await
        .unwrap();

    let help = session
        .eval("help @fs.patch", CellBudget::default())
        .await
        .unwrap();
    assert_eq!(help.result.as_ref().unwrap()["name"], json!("fs.patch"));
    assert_eq!(
        help.result.as_ref().unwrap()["signature"],
        json!("fs.patch(args)")
    );
    assert_eq!(help.result.as_ref().unwrap()["approval"], json!("on-write"));

    let display = session
        .eval(
            "\"0123456789012345678901234567890123456789\" |> display {kind: \"text\"}",
            CellBudget {
                wall_ms: 1_000,
                output_bytes: 32,
            },
        )
        .await
        .unwrap();
    let display_error = display.error.as_deref().unwrap();
    assert!(display_error.len() <= 32, "{display_error}");
    assert!(
        display_error.starts_with("ResourceLimitError"),
        "{display_error}"
    );
    assert!(
        events
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|(event, _)| event != "display")
    );
}
