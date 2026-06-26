use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::Result;

/// Configuration for a freshly opened session.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// Per-session workspace root, when the backend jails filesystem access (M1+).
    pub workspace: Option<PathBuf>,
}

/// Per-cell resource budget. Enforced by real backends; advisory for the stub.
#[derive(Debug, Clone, Copy)]
pub struct CellBudget {
    /// Wall-clock cap in milliseconds before the cell is terminated.
    pub wall_ms: u64,
    /// Cap on stdout + return bytes before truncation / artifact spill.
    pub output_bytes: usize,
}

impl Default for CellBudget {
    fn default() -> Self {
        Self {
            wall_ms: 30_000,
            output_bytes: 8 * 1024,
        }
    }
}

/// The result of evaluating one cell. A cell that *throws* sets `error` — that is data for
/// the model, not a host failure (which would be an [`crate::Error`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalOutput {
    pub stdout: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// A backend that hands out persistent code-execution [`Session`]s.
#[async_trait]
pub trait Sandbox: Send + Sync {
    async fn open(&self, cfg: SessionConfig) -> Result<Box<dyn Session>>;
}

/// A persistent REPL session. State (variables, definitions) survives across `eval` calls;
/// `reset` tears it down for a clean slate.
#[async_trait]
pub trait Session: Send {
    async fn eval(&mut self, code: &str, budget: CellBudget) -> Result<EvalOutput>;
    async fn reset(&mut self) -> Result<()>;
}
