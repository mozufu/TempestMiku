use super::support::*;
use super::*;

#[test]
fn default_prompt_teaches_tm_repl_discovery_contract() {
    assert!(DEFAULT_SYSTEM_PROMPT.contains("@tools.search"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("help @capability"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("agents.parallel"));
    assert!(DEFAULT_SYSTEM_PROMPT.contains("do not spend turns rediscovering a capability"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("not JavaScript"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("tm-conformance-v2"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("at most 16 `execute` calls"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("ordered left-to-right"));
    assert!(TM_RUNTIME_BOOT_CONTRACT.contains("custom persona, mode, and actor prompts"));
    assert!(!TM_RUNTIME_BOOT_CONTRACT.contains("fs.patch"));
    assert!(!TM_RUNTIME_BOOT_CONTRACT.contains("stale tag"));

    let description = ToolSpec::execute().function.description;
    assert!(description.contains("tm-conformance-v2"));
    assert!(description.contains("@capability"));
    assert!(description.contains("@tools.search"));
}

#[tokio::test]
async fn native_requests_advertise_exactly_execute() {
    struct InspectingLlm(Arc<Mutex<Vec<ToolSpec>>>);

    #[async_trait]
    impl LlmClient for InspectingLlm {
        async fn chat_stream(
            &self,
            request: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            *self.0.lock() = request.tools.clone();
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::Text("done".to_string())),
                Ok(StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                }),
            ])))
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent::new(
        Arc::new(InspectingLlm(Arc::clone(&seen))),
        Arc::new(EchoSandbox),
        config(Protocol::NativeTool),
    );
    agent.run("hello", &CaptureSink::default()).await.unwrap();

    let tools = seen.lock();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function.name, "execute");
}

#[tokio::test]
async fn tm_requests_append_language_contract_and_advertise_tm_execute() {
    struct InspectingLlm(Arc<Mutex<Option<(String, String)>>>);

    #[async_trait]
    impl LlmClient for InspectingLlm {
        async fn chat_stream(
            &self,
            request: &ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
            *self.0.lock() = Some((
                request.messages[0].content.clone(),
                request.tools[0].function.description.clone(),
            ));
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::Text("done".to_string())),
                Ok(StreamEvent::Finish {
                    reason: Some("stop".to_string()),
                }),
            ])))
        }
    }

    let seen = Arc::new(Mutex::new(None));
    let mut cfg = config(Protocol::NativeTool);
    cfg.system_prompt = "custom persona prompt".into();
    let agent = Agent::new(
        Arc::new(InspectingLlm(Arc::clone(&seen))),
        Arc::new(EchoSandbox),
        cfg,
    );
    agent.run("hello", &CaptureSink::default()).await.unwrap();

    let seen = seen.lock();
    let (prompt, description) = seen.as_ref().unwrap();
    assert!(prompt.starts_with("## Immutable tm runtime contract"));
    assert!(prompt.contains("custom persona prompt"));
    assert!(prompt.contains("tm-conformance-v2"));
    assert!(prompt.contains("separate top-level forms"));
    assert!(!prompt.contains("fs.patch"));
    assert!(!description.contains("JavaScript/TypeScript"));
    assert!(description.contains("tm-conformance-v2"));
}
