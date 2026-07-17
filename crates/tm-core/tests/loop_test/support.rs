use super::*;

/// An `LlmClient` that replays a fixed script of stream events per turn and records every
/// request's message list for assertions.
pub(super) struct FakeLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    pub(super) requests: Mutex<Vec<Vec<Message>>>,
}

impl FakeLlm {
    pub(super) fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for FakeLlm {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        self.requests.lock().push(req.messages.clone());
        let script = self.scripts.lock().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(
            script.into_iter().map(Ok::<StreamEvent, Error>),
        )))
    }
}

/// A sandbox that echoes the submitted code as its result — enough to prove the loop feeds the
/// cell output back to the model.
pub(super) struct EchoSandbox;

#[async_trait]
impl Sandbox for EchoSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(EchoSession))
    }
}

struct EchoSession;

#[async_trait(?Send)]
impl Session for EchoSession {
    async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        Ok(EvalOutput {
            stdout: String::new(),
            result: Some(Value::String(code.to_string())),
            error: None,
        })
    }

    async fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

pub(super) struct CountingSession {
    pub(super) evaluations: usize,
}

#[async_trait(?Send)]
impl Session for CountingSession {
    async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        self.evaluations += 1;
        Ok(EvalOutput {
            result: Some(Value::from(self.evaluations)),
            ..EvalOutput::default()
        })
    }

    async fn reset(&mut self) -> Result<()> {
        self.evaluations = 0;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct BatchSession {
    pub(super) batches: Vec<Vec<String>>,
}

#[async_trait(?Send)]
impl Session for BatchSession {
    async fn eval(&mut self, _code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        panic!("agent loop should use eval_batch for native execute calls")
    }

    async fn eval_batch(
        &mut self,
        codes: &[String],
        _budget: CellBudget,
    ) -> Result<Vec<EvalOutput>> {
        self.batches.push(codes.to_vec());
        Ok(codes
            .iter()
            .map(|code| EvalOutput {
                result: Some(Value::String(code.clone())),
                ..EvalOutput::default()
            })
            .collect())
    }

    async fn reset(&mut self) -> Result<()> {
        self.batches.clear();
        Ok(())
    }
}

pub(super) struct StaticInbox {
    messages: Mutex<VecDeque<String>>,
}

impl StaticInbox {
    pub(super) fn new(messages: Vec<&str>) -> Self {
        Self {
            messages: Mutex::new(messages.into_iter().map(str::to_string).collect()),
        }
    }
}

#[async_trait]
impl InboxDrain for StaticInbox {
    async fn drain(&self) -> Result<Vec<String>> {
        Ok(self.messages.lock().drain(..).collect())
    }
}

pub(super) struct FlagCancellation(AtomicBool);

impl FlagCancellation {
    pub(super) fn active() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(super) fn cancelled() -> Self {
        Self(AtomicBool::new(true))
    }

    pub(super) fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl CancellationToken for FlagCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Records the streaming events the loop emits, in order.
#[derive(Default)]
pub(super) struct CaptureSink {
    events: Mutex<Vec<String>>,
}

impl CaptureSink {
    pub(super) fn events(&self) -> Vec<String> {
        self.events.lock().clone()
    }
}

impl EventSink for CaptureSink {
    fn on_text(&self, delta: &str) {
        self.events.lock().push(format!("text:{delta}"));
    }
    fn on_tool_call(&self, name: &str) {
        self.events.lock().push(format!("tool:{name}"));
    }
    fn on_cell_start(&self, _code: &str) {
        self.events.lock().push("cell_start".into());
    }
    fn on_cell_result(&self, _shaped: &str) {
        self.events.lock().push("cell_result".into());
    }
    fn on_final(&self, text: &str) {
        self.events.lock().push(format!("final:{text}"));
    }
}

pub(super) struct FailingSink(pub(super) &'static str);

impl FailingSink {
    fn at(&self, point: &str) -> Result<()> {
        if self.0 == point {
            Err(Error::EventSink(point.to_string()))
        } else {
            Ok(())
        }
    }
}

impl EventSink for FailingSink {
    fn try_on_reasoning(&self, _delta: &str) -> Result<()> {
        self.at("reasoning")
    }

    fn try_on_text(&self, _delta: &str) -> Result<()> {
        self.at("text")
    }

    fn try_on_tool_call(&self, _name: &str) -> Result<()> {
        self.at("tool_call")
    }

    fn try_on_cell_start(&self, _code: &str) -> Result<()> {
        self.at("cell_start")
    }

    fn try_on_cell_result(&self, _shaped: &str) -> Result<()> {
        self.at("cell_result")
    }

    fn try_on_final(&self, _text: &str) -> Result<()> {
        self.at("final")
    }

    fn try_on_turn_end(&self) -> Result<()> {
        self.at("turn_end")
    }
}

pub(super) fn config(protocol: Protocol) -> AgentConfig {
    AgentConfig {
        model: "fake".into(),
        protocol,
        ..AgentConfig::default()
    }
}
