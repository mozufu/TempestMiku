use super::support::*;
use super::*;

#[tokio::test]
async fn open_session_path_includes_prior_history_and_reuses_session_state() {
    let execute_turn = |id: &str| {
        vec![
            StreamEvent::ToolCall {
                index: 0,
                id: Some(id.to_string()),
                name: Some("execute".to_string()),
                arguments: Some(r#"{"code":"display('count')"}"#.to_string()),
            },
            StreamEvent::Finish {
                reason: Some("tool_calls".to_string()),
            },
        ]
    };
    let final_turn = |text: &str| {
        vec![
            StreamEvent::Text(text.to_string()),
            StreamEvent::Finish {
                reason: Some("stop".to_string()),
            },
        ]
    };
    let llm = Arc::new(FakeLlm::new(vec![
        execute_turn("call_1"),
        final_turn("first"),
        execute_turn("call_2"),
        final_turn("second"),
    ]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let prior = vec![
        Message::user("earlier question"),
        Message::assistant("earlier answer"),
    ];
    let mut session = CountingSession { evaluations: 0 };

    assert_eq!(
        agent
            .run_with_session("first current question", &prior, &mut session, &sink)
            .await
            .unwrap(),
        "first"
    );
    assert_eq!(
        agent
            .run_with_session("second current question", &[], &mut session, &sink)
            .await
            .unwrap(),
        "second"
    );
    assert_eq!(session.evaluations, 2);

    let requests = llm.requests.lock();
    assert_eq!(requests[0][0].role, Role::System);
    assert_eq!(&requests[0][1..3], prior.as_slice());
    assert_eq!(requests[0][3], Message::user("first current question"));
}

#[tokio::test]
async fn run_with_controls_stops_before_model_turn_when_cancelled() {
    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::Text("should not stream".into()),
        StreamEvent::Finish {
            reason: Some("stop".into()),
        },
    ]]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::cancelled();

    let err = agent
        .run_with_controls("please do work", &sink, None, Some(&cancellation))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Cancelled));
    assert!(llm.requests.lock().is_empty());
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_stream() {
    struct PendingLlm;

    #[async_trait]
    impl LlmClient for PendingLlm {
        async fn chat_stream(
            &self,
            _req: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            Ok(Box::pin(stream::pending()))
        }
    }

    let agent = Agent::new(
        Arc::new(PendingLlm),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("please wait forever", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_llm_connection() {
    struct PendingConnectLlm;

    #[async_trait]
    impl LlmClient for PendingConnectLlm {
        async fn chat_stream(
            &self,
            _req: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            futures::future::pending::<()>().await;
            unreachable!("pending LLM connection returned")
        }
    }

    let agent = Agent::new(
        Arc::new(PendingConnectLlm),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("connect", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_inbox_wait() {
    struct PendingInbox;

    #[async_trait]
    impl InboxDrain for PendingInbox {
        async fn drain(&self) -> Result<Vec<String>> {
            futures::future::pending().await
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::Text("must not run".to_string()),
        StreamEvent::Finish {
            reason: Some("stop".to_string()),
        },
    ]]));
    let agent = Agent::new(
        llm.clone(),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let inbox = PendingInbox;
    let run = agent.run_with_controls("wait", &sink, Some(&inbox), Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(llm.requests.lock().is_empty());
    assert!(sink.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_controls_cancels_a_pending_session_eval() {
    struct PendingEvalSandbox;
    struct PendingEvalSession;

    #[async_trait]
    impl Sandbox for PendingEvalSandbox {
        async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
            Ok(Box::new(PendingEvalSession))
        }
    }

    #[async_trait(?Send)]
    impl Session for PendingEvalSession {
        async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            futures::future::pending().await
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_pending_eval".to_string()),
            name: Some("execute".to_string()),
            arguments: Some(r#"{"code":"await pendingHostWait()"}"#.to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]]));
    let agent = Agent::new(
        llm,
        Arc::new(PendingEvalSandbox),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let cancellation = FlagCancellation::active();
    let run = agent.run_with_controls("evaluate", &sink, None, Some(&cancellation));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(sink.events().contains(&"cell_start".to_string()));
    assert!(!sink.events().contains(&"cell_result".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_aware_session_finishes_its_terminal_protocol_before_agent_returns() {
    struct CooperativeSandbox {
        cancellation: Arc<FlagCancellation>,
        terminalized: Arc<AtomicBool>,
    }
    struct CooperativeSession {
        cancellation: Arc<FlagCancellation>,
        terminalized: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Sandbox for CooperativeSandbox {
        async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
            Ok(Box::new(CooperativeSession {
                cancellation: Arc::clone(&self.cancellation),
                terminalized: Arc::clone(&self.terminalized),
            }))
        }
    }

    #[async_trait(?Send)]
    impl Session for CooperativeSession {
        fn handles_cancellation(&self) -> bool {
            true
        }

        async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
            self.cancellation.cancelled().await;
            // Model the runtime's bounded terminal-persistence/commit shield. An outer race would
            // return on cancellation and drop this future before the protocol completes.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.terminalized.store(true, Ordering::SeqCst);
            Err(Error::Cancelled)
        }

        async fn reset(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let llm = Arc::new(FakeLlm::new(vec![vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some("call_cooperative_eval".to_string()),
            name: Some("execute".to_string()),
            arguments: Some(r#"{"code":"await cooperativeHostWait()"}"#.to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]]));
    let cancellation = Arc::new(FlagCancellation::active());
    let terminalized = Arc::new(AtomicBool::new(false));
    let agent = Agent::new(
        llm,
        Arc::new(CooperativeSandbox {
            cancellation: Arc::clone(&cancellation),
            terminalized: Arc::clone(&terminalized),
        }),
        config(Protocol::NativeTool),
    );
    let sink = CaptureSink::default();
    let run = agent.run_with_controls("evaluate", &sink, None, Some(cancellation.as_ref()));
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        cancellation.cancel();
    };

    let (result, ()) = tokio::join!(run, cancel);

    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(
        terminalized.load(Ordering::SeqCst),
        "agent returned before the session completed terminal persistence"
    );
}
