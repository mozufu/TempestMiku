use async_trait::async_trait;
use serde_json::Value;
use tm_core::{CellBudget, EvalOutput, Result, Sandbox, Session, SessionConfig};

/// A sandbox that runs no code. Each `eval` echoes the submitted source as its result and notes
/// the cell index in stdout, which is enough to validate the `tool_call -> tool_result -> final`
/// loop without a runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubSandbox;

#[async_trait]
impl Sandbox for StubSandbox {
    async fn open(&self, _cfg: SessionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(StubSession::default()))
    }
}

/// A persistent session for [`StubSandbox`].
#[derive(Debug, Default)]
pub struct StubSession {
    cells: usize,
}

#[async_trait(?Send)]
impl Session for StubSession {
    async fn eval(&mut self, code: &str, _budget: CellBudget) -> Result<EvalOutput> {
        self.cells += 1;
        Ok(EvalOutput {
            stdout: format!(
                "[stub sandbox] no runtime yet (M1); echoing cell #{} ({} bytes)",
                self.cells,
                code.len()
            ),
            result: Some(Value::String(code.to_string())),
            error: None,
        })
    }

    async fn reset(&mut self) -> Result<()> {
        self.cells = 0;
        Ok(())
    }
}
