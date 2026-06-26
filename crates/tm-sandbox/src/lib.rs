//! Sandbox backends.
//!
//! M0 ships only [`StubSandbox`]: it implements the [`Sandbox`]/[`Session`] traits without a
//! real interpreter so the streaming message protocol can be exercised end-to-end. The real
//! `deno_core` (V8/TypeScript) backend lands in M1 behind the same traits — the loop, SDK, and
//! result-shaping never change.

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

/// A persistent session for [`StubSandbox`]. Tracks how many cells have run so callers can see
/// state surviving across `eval` calls (a property the real backend must preserve).
#[derive(Debug, Default)]
pub struct StubSession {
    cells: usize,
}

#[async_trait]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_code_and_persists_cell_count() {
        let sandbox = StubSandbox;
        let mut session = sandbox.open(SessionConfig::default()).await.unwrap();

        let out = session.eval("1 + 1", CellBudget::default()).await.unwrap();
        assert_eq!(out.result, Some(Value::String("1 + 1".into())));
        assert!(out.stdout.contains("cell #1"));

        let out2 = session.eval("2 + 2", CellBudget::default()).await.unwrap();
        assert!(out2.stdout.contains("cell #2")); // state persisted across eval calls

        session.reset().await.unwrap();
        let out3 = session.eval("3", CellBudget::default()).await.unwrap();
        assert!(out3.stdout.contains("cell #1")); // reset cleared it
    }
}
