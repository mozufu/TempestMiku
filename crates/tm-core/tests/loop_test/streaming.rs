use super::support::*;
use super::*;

#[tokio::test]
async fn native_tool_loop_streams_and_executes() {
    // Turn 1: an `execute` tool call whose JSON arguments are split across two deltas.
    // Turn 2: the final answer, streamed token-by-token.
    let scripts = vec![
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_1".into()),
                name: Some("execute".into()),
                arguments: Some("{\"code\":\"dis".into()),
            },
            StreamEvent::ToolCall {
                index: 0,
                id: None,
                name: None,
                arguments: Some("play(2+2)\"}".into()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".into()),
            },
        ],
        vec![
            StreamEvent::Text("The answer ".into()),
            StreamEvent::Text("is 4.".into()),
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

    let answer = agent.run("compute 2+2", &sink).await.unwrap();
    assert_eq!(answer, "The answer is 4.");

    let events = sink.events();
    // The fragmented tool call resolved to a single named call.
    assert_eq!(
        events.iter().filter(|e| *e == "tool:execute").count(),
        1,
        "events: {events:?}"
    );
    assert!(events.contains(&"cell_start".to_string()));
    assert!(events.contains(&"cell_result".to_string()));
    // Assistant text streamed in order, as discrete deltas.
    let text: Vec<_> = events.iter().filter(|e| e.starts_with("text:")).collect();
    assert_eq!(text, vec!["text:The answer ", "text:is 4."]);
    assert_eq!(events.last().unwrap(), "final:The answer is 4.");

    // The loop appended a tool result keyed to the streamed call id, carrying the echoed code.
    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 2, "two turns => two requests");
    let tool_msg = requests[1]
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("tool result appended before turn 2");
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
    assert!(
        tool_msg.content.contains("display(2+2)"),
        "tool result should echo the executed code, got: {}",
        tool_msg.content
    );
}

#[tokio::test]
async fn fallible_sink_errors_abort_every_agent_emission_point() {
    let final_script = || {
        vec![
            StreamEvent::Reasoning("private".to_string()),
            StreamEvent::Text("answer".to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let tool_script = || {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some("call_sink".to_string()),
                name: Some("execute".to_string()),
                arguments: Some(r#"{"code":"display(1)"}"#.to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };

    for point in ["reasoning", "text", "turn_end", "final"] {
        let agent = Agent::new(
            Arc::new(FakeLlm::new(vec![final_script()])),
            Arc::new(EchoSandbox),
            config(Protocol::NativeTool),
        );
        let error = agent.run("emit", &FailingSink(point)).await.unwrap_err();
        assert!(matches!(error, Error::EventSink(ref failed) if failed == point));
    }

    for point in ["tool_call", "cell_start", "cell_result"] {
        let agent = Agent::new(
            Arc::new(FakeLlm::new(vec![tool_script(), final_script()])),
            Arc::new(EchoSandbox),
            config(Protocol::NativeTool),
        );
        let error = agent.run("emit", &FailingSink(point)).await.unwrap_err();
        assert!(matches!(error, Error::EventSink(ref failed) if failed == point));
    }
}

#[tokio::test]
async fn fenced_block_loop_executes() {
    // Turn 1: a ```run fenced block, streamed in fragments. Turn 2: the final answer.
    let scripts = vec![
        vec![
            StreamEvent::Text("Let me compute.\n```run\n".into()),
            StreamEvent::Text("display(2 + 2)\n```\n".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
        vec![
            StreamEvent::Text("It is 4.".into()),
            StreamEvent::Finish {
                reason: Some("stop".into()),
            },
        ],
    ];

    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::FencedBlock),
    );
    let sink = CaptureSink::default();

    let answer = agent.run("compute 2+2", &sink).await.unwrap();
    assert_eq!(answer, "It is 4.");

    // The parsed fence ran and its result was fed back as a user message (no tool channel).
    let requests = llm.requests.lock();
    assert_eq!(requests.len(), 2);
    let fed_back = requests[1]
        .iter()
        .find(|m| m.role == Role::User && m.content.contains("[execution result]"))
        .expect("execution result fed back as a user message");
    assert!(fed_back.content.contains("display(2 + 2)"));
}

#[tokio::test]
async fn run_with_inbox_drains_before_model_turn() {
    let scripts = vec![vec![
        StreamEvent::Text("I saw the inbox.".into()),
        StreamEvent::Finish {
            reason: Some("stop".into()),
        },
    ]];

    let llm = Arc::new(FakeLlm::new(scripts));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let inbox = StaticInbox::new(vec!["from: Worker\ntext: done"]);

    let answer = agent
        .run_with_inbox("wait for worker", &sink, Some(&inbox))
        .await
        .unwrap();
    assert_eq!(answer, "I saw the inbox.");

    let requests = llm.requests.lock();
    let inbox_msg = requests[0]
        .iter()
        .find(|m| m.role == Role::User && m.content.contains("[actor inbox]"))
        .expect("inbox message should be appended before the first turn");
    assert!(inbox_msg.content.contains("from: Worker"));
    assert!(inbox_msg.content.contains("text: done"));
}
