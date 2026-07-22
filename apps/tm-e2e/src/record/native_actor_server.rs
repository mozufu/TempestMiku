use super::*;

pub(super) struct NativeActorRecordingServer {
    pub(super) base_url: String,
    pub(super) artifact_root: PathBuf,
    handle: tokio::task::JoinHandle<()>,
    pub(super) live_tail_calls: Arc<AtomicUsize>,
}

impl NativeActorRecordingServer {
    pub(super) async fn start(run_root: &Path, live_model: String) -> Result<Self> {
        let artifact_root = run_root.join("native-actor-artifacts");
        fs::create_dir_all(&artifact_root)
            .with_context(|| format!("creating {}", artifact_root.display()))?;
        let parent_code = native_parent_coordination_code();
        let child_code = native_child_coordination_code();
        let live_tail_calls = Arc::new(AtomicUsize::new(0));
        let llm = Arc::new(ScriptedThenLiveLlm::new(
            vec![
                execute_script("parent_call", &parent_code),
                execute_script("child_call_0", &child_code),
                execute_script("child_call_1", &child_code),
            ],
            OpenAiClient::from_env().context("creating native actor live-tail LLM")?,
            live_model,
            Arc::clone(&live_tail_calls),
        ));
        let cfg = AgentConfig {
            model: "scripted-then-env-live".to_string(),
            max_turns: 6,
            cell_budget: CellBudget {
                wall_ms: 240_000,
                output_bytes: 50_000,
            },
            ..AgentConfig::default()
        };

        let roster = Arc::new(MailboxRegistry::new());
        let mut sandbox_options = TmSandboxOptions {
            artifact_root: artifact_root.clone(),
            approval_timeout: Duration::from_secs(5),
            ..TmSandboxOptions::default()
        };
        tm_agents::register(
            &mut sandbox_options.host_registry,
            &mut sandbox_options.resource_registry,
            Arc::clone(&roster),
        );

        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let mut state = AppState::new(
            store,
            memory,
            chat,
            tm_server::ModesConfig::default(),
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(true)
        .with_artifact_root(artifact_root.clone())
        .with_actor_roster(Arc::clone(&roster));
        let broker = Arc::clone(&state.approval_broker);

        let executor_options = sandbox_options.clone();
        let executor_roster = Arc::clone(&roster);
        let executor_approval_roster = Arc::clone(&roster);
        let executor_broker = Arc::clone(&broker);
        let llm_for_executor: Arc<dyn LlmClient> = llm.clone();
        let executor: Arc<dyn tm_agents::ActorExecutor> =
            Arc::new(ChatActorExecutor::with_actor_context(
                llm_for_executor,
                cfg.clone(),
                move |session_id, actor_id, grants, project_id, cancellation| {
                    let mut opts = executor_options.clone();
                    opts.session_id = session_id.to_string();
                    opts.actor_id = actor_id.map(str::to_string);
                    opts.project_id = project_id.map(str::to_string);
                    opts.cancellation = cancellation;
                    opts.grants =
                        CapabilityGrants::default().allow_many(grants.names().map(str::to_string));
                    let sink: Arc<dyn CodingEventSink> = Arc::new(RosterCodingEventSink::new(
                        session_id,
                        Arc::clone(&executor_approval_roster),
                    ));
                    opts.approval_policy = Arc::new(
                        HttpApprovalPolicy::new(Arc::clone(&executor_broker), session_id, sink)
                            .with_actor_id(actor_id.map(str::to_string)),
                    );
                    Arc::new(TmSandbox::new(opts)) as Arc<dyn tm_core::Sandbox>
                },
                Some(artifact_root.clone()),
                executor_roster,
            ));
        roster.set_executor(executor);

        let llm_for_backend: Arc<dyn LlmClient> = llm.clone();
        let backend = NativeTmBackend::new(
            llm_for_backend,
            cfg,
            sandbox_options,
            NativeApprovalMode::Manual,
            broker,
        );
        state = state.with_coding_backend(Arc::new(backend));
        state.wire_lifecycle_sink();

        let router = app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding tm-e2e native actor server")?;
        let addr = listener
            .local_addr()
            .context("reading native actor server addr")?;
        let handle = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("tm-e2e native actor server exited: {err}");
            }
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            artifact_root,
            handle,
            live_tail_calls,
        })
    }
}

impl Drop for NativeActorRecordingServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn native_parent_coordination_code() -> String {
    format!(
        r#"
let alpha = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report.", opts: {{capabilities: ["agents.*"]}}}};
let beta = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report.", opts: {{capabilities: ["agents.*"]}}}};
let readyA = @agents.wait {{from: alpha, timeoutMs: 15000}};
let readyB = @agents.wait {{from: beta, timeoutMs: 15000}};
let receipts = @agents.broadcast {{text: "{broadcast}"}};
let first = @agents.wait {{from: alpha, timeoutMs: 15000}};
let second = @agents.wait {{from: beta, timeoutMs: 15000}};
let roster = @agents.list {{}};
{{
  receipts,
  ready: [readyA.text, readyB.text],
  reports: [first.text, second.text],
  roster
}} |> display {{kind: "json"}}
"#,
        broadcast = NATIVE_P3_BROADCAST_TEXT
    )
}

fn native_child_coordination_code() -> String {
    r#"
let ready = @agents.send {to: "Root", text: "child ready for native broadcast"};
let msg = @agents.wait {from: "Root", timeoutMs: 15000};
let text = msg.text;
let artifact = @artifacts.put {data: "native child saw: #{text}", title: "native p3 child"};
let report = @agents.send {to: "Root", text: "child report #{artifact.uri}: #{text}"};
{text, artifact: artifact.uri} |> display {kind: "json"}
"#
    .to_string()
}

fn execute_script(id: &str, code: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some(id.to_string()),
            name: Some("execute".to_string()),
            arguments: Some(json!({ "code": code }).to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]
}

struct ScriptedThenLiveLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    live: OpenAiClient,
    live_model: String,
    live_tail_calls: Arc<AtomicUsize>,
}

impl ScriptedThenLiveLlm {
    fn new(
        scripts: Vec<Vec<StreamEvent>>,
        live: OpenAiClient,
        live_model: String,
        live_tail_calls: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            live,
            live_model,
            live_tail_calls,
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedThenLiveLlm {
    async fn chat_stream(
        &self,
        _req: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        let scripted = self
            .scripts
            .lock()
            .map_err(|_| CoreError::Llm("scripted native actor LLM lock poisoned".to_string()))?
            .pop_front();
        if let Some(events) = scripted {
            return Ok(Box::pin(stream::iter(
                events.into_iter().map(Ok::<StreamEvent, CoreError>),
            )));
        }

        self.live_tail_calls.fetch_add(1, Ordering::SeqCst);
        self.live
            .chat_stream(&ChatRequest {
                model: self.live_model.clone(),
                messages: vec![
                    Message::system("You are a deterministic integration-test endpoint."),
                    Message::user(format!(
                        "Return exactly this ASCII string and nothing else: {NATIVE_P3_FINAL_TEXT}"
                    )),
                ],
                tools: Vec::new(),
                tool_choice: ToolChoice::None,
                temperature: Some(0.0),
                max_tokens: Some(32),
            })
            .await
    }
}
