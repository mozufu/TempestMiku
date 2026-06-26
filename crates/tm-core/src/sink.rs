/// Observer for streaming loop events. The loop calls these as things happen, so a UI can
/// render assistant tokens the instant they arrive (day-1 streaming, design §5.5).
///
/// Every method has a no-op default; implement only what you render.
pub trait EventSink: Send + Sync {
    /// A fragment of assistant text just arrived.
    fn on_text(&self, _delta: &str) {}
    /// The model began a tool call with this name.
    fn on_tool_call(&self, _name: &str) {}
    /// The sandbox is about to evaluate this code.
    fn on_cell_start(&self, _code: &str) {}
    /// The shaped cell result that will be fed back to the model.
    fn on_cell_result(&self, _shaped: &str) {}
    /// The model produced a final answer (no tool call).
    fn on_final(&self, _text: &str) {}
    /// The streamed turn finished (after the last delta, before any cell runs).
    fn on_turn_end(&self) {}
}

/// An [`EventSink`] that drops everything. Useful for tests and headless runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {}
