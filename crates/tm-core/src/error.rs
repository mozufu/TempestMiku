use thiserror::Error;

/// Errors surfaced by the core loop and the traits it drives.
///
/// Sandbox *evaluation* failures (a cell throwing) are data on [`crate::EvalOutput`],
/// not variants here — those flow back to the model. These variants are host-side
/// failures the loop cannot paper over.
#[derive(Debug, Error)]
pub enum Error {
    /// The LLM transport failed (network, decode, bad status).
    #[error("llm transport error: {0}")]
    Llm(String),

    /// The sandbox host failed to open a session or run a cell.
    #[error("sandbox error: {0}")]
    Sandbox(String),

    /// The model violated the wire protocol (e.g. malformed tool call).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The loop ran the configured number of turns without a final answer.
    #[error("turn budget exhausted after {0} turns")]
    TurnBudget(usize),

    /// JSON (de)serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Crate result type. The error parameter defaults to [`Error`] but stays overridable.
pub type Result<T, E = Error> = std::result::Result<T, E>;
