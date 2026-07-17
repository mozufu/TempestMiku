use super::support::*;
use super::*;

#[tokio::test]
async fn native_tool_loop_executes_parallel_batch_and_shapes_results_in_response_order() {
    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_1".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"display('first')"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_2".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"display('second')"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("done".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ];
    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let mut session = BatchSession::default();

    assert_eq!(
        agent
            .run_with_session("run both", &[], &mut session, &sink)
            .await
            .unwrap(),
        "done"
    );
    assert_eq!(
        session.batches,
        vec![vec![
            "display('first')".to_string(),
            "display('second')".to_string()
        ]]
    );

    let events = sink.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "cell_start")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "cell_result")
            .count(),
        2
    );

    let requests = llm.requests.lock();
    let tool_results: Vec<_> = requests[1]
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0].tool_call_id.as_deref(), Some("call_1"));
    assert!(tool_results[0].content.contains("display('first')"));
    assert_eq!(tool_results[1].tool_call_id.as_deref(), Some("call_2"));
    assert!(tool_results[1].content.contains("display('second')"));
}

#[tokio::test]
async fn rejects_oversized_or_malformed_tool_call_batches_before_execution() {
    let mut oversized = (0..17)
        .map(|index| StreamEvent::ToolCall {
            index,
            id: Some(format!("call_{index}")),
            name: Some("execute".into()),
            arguments: Some(format!(r#"{{"code":"{index}"}}"#)),
        })
        .collect::<Vec<_>>();
    oversized.push(StreamEvent::Finish {
        reason: Some("tool_calls".into()),
    });
    let cases = vec![
        oversized,
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_valid".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_missing_name".into()),
                name: None,
                arguments: Some(r#"{"code":"2"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_duplicate".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::ToolCall {
                index: 1,
                id: Some("call_duplicate".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"2"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("must not become a final answer".into()),
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_missing_name".into()),
                name: None,
                arguments: Some(r#"{"code":"1"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_other".into()),
                name: Some("mode_suggest".into()),
                arguments: Some(r#"{"target_mode":"serious_engineer"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_bad".into()),
                name: Some("execute".into()),
                arguments: Some("{}".into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_extra".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"1","extra":true}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_empty".into()),
                name: Some("execute".into()),
                arguments: Some(r#"{"code":"  \n"}"#.into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
    ];

    for script in cases {
        let llm = Arc::new(FakeLlm::new(vec![script]));
        let agent = Agent::new(llm, Arc::new(EchoSandbox), config(Protocol::NativeTool));
        let sink = CaptureSink::default();
        let result = agent.run("bad tool turn", &sink).await;
        assert!(matches!(result, Err(Error::Protocol(_))));
        assert!(!sink.events().contains(&"cell_start".to_string()));
    }
}

#[tokio::test]
async fn compatibility_eval_batch_stops_after_first_session_failure() {
    struct FailsSecondSession {
        calls: Vec<String>,
    }

    #[async_trait(?Send)]
    impl Session for FailsSecondSession {
        async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            self.calls.push(code.to_string());
            if self.calls.len() == 2 {
                return Err(Error::Sandbox("second cell failed".to_string()));
            }
            Ok(EvalOutput {
                result: Some(Value::String(code.to_string())),
                ..EvalOutput::default()
            })
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let script = vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_1".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"first"}"#.into()),
        },
        StreamEvent::ToolCall {
            index: 1,
            id: Some("call_2".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"second"}"#.into()),
        },
        StreamEvent::ToolCall {
            index: 2,
            id: Some("call_3".into()),
            name: Some("execute".into()),
            arguments: Some(r#"{"code":"third"}"#.into()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".into()),
        },
    ];
    let agent = Agent::new(
        Arc::new(FakeLlm::new(vec![script])),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let mut session = FailsSecondSession { calls: Vec::new() };

    let result = agent
        .run_with_session("run batch", &[], &mut session, &sink)
        .await;

    assert!(matches!(result, Err(Error::Sandbox(ref message)) if message == "second cell failed"));
    assert_eq!(session.calls, vec!["first", "second"]);
    assert_eq!(
        sink.events()
            .iter()
            .filter(|event| event.as_str() == "cell_start")
            .count(),
        3
    );
}
