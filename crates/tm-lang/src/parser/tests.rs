use super::*;

#[test]
fn application_and_pipe_follow_precedence() {
    let cell = parse("text |> split \",\" |> filter predicate").unwrap();
    assert!(matches!(
        cell.forms[0].node,
        FormKind::Expr(Expr {
            node: ExprKind::Pipe { .. },
            ..
        })
    ));
}

#[test]
fn parses_local_sum_match_and_effect() {
    let source = "do { type Finding = | Hit {file: String, line: Int} | Ignored Path; let x = Hit {file: \"a\", line: 1}; match x { | Hit {file, line} -> @fs.read workspace:a | Ignored _ -> null } }";
    parse(source).unwrap();
}

#[test]
fn parses_handled_effect_with_record_arguments() {
    parse(
            r#"handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error { | error -> error }"#,
        )
        .unwrap();
}

#[test]
fn rejects_implicit_top_level_sequence() {
    let error = parse("let x = 1 let y = 2").unwrap_err();
    assert_eq!(error.code, "TM2004");
}

#[test]
fn nesting_budget_covers_every_recursive_pattern_form() {
    let nested_parens = format!("let {}x{} = null", "(".repeat(16), ")".repeat(16));
    let nested_lists = format!("let {}x{} = null", "[".repeat(16), "]".repeat(16));
    let nested_constructors = format!("let {}x = null", "Some ".repeat(16));
    let nested_cons = format!("let {}x = null", "x :: ".repeat(16));

    for source in [
        nested_parens,
        nested_lists,
        nested_constructors,
        nested_cons,
    ] {
        let error = parse_bounded(&source, 4096, 4096, 8).unwrap_err();
        assert_eq!(error.code, "TM2021", "{source}");
    }
}

#[test]
fn nesting_budget_covers_flat_left_associative_expression_chains() {
    let fields = format!("value{}", ".field".repeat(32));
    let applications = format!("function{}", " argument".repeat(32));
    let infix = std::iter::repeat_n("1", 32).collect::<Vec<_>>().join(" + ");
    let pipes = std::iter::repeat_n("value", 32)
        .collect::<Vec<_>>()
        .join(" |> ");

    for source in [fields, applications, infix, pipes] {
        let error = parse_bounded(&source, 4096, 4096, 8).unwrap_err();
        assert_eq!(error.code, "TM2021", "{source}");
    }
}
