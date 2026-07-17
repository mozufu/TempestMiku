/// Observer for streaming loop events. The loop calls these as things happen, so a UI can
/// render assistant tokens the instant they arrive (day-1 streaming, design §5.5).
///
/// Every method has a no-op default; implement only what you render. The `try_on_*` variants
/// preserve these callbacks by default while allowing bounded transports to fail the agent turn
/// instead of silently dropping an event.
pub trait EventSink: Send + Sync {
    /// A fragment of private assistant reasoning (chain-of-thought) just arrived. Providers
    /// that stream `reasoning` / `reasoning_content` deltas produce these; they are never
    /// sent back to the model. Default drops them so existing sinks keep compiling.
    fn on_reasoning(&self, _delta: &str) {}
    fn try_on_reasoning(&self, delta: &str) -> crate::Result<()> {
        self.on_reasoning(delta);
        Ok(())
    }
    /// A fragment of assistant text just arrived.
    fn on_text(&self, _delta: &str) {}
    fn try_on_text(&self, delta: &str) -> crate::Result<()> {
        self.on_text(delta);
        Ok(())
    }
    /// The model began a tool call with this name.
    fn on_tool_call(&self, _name: &str) {}
    fn try_on_tool_call(&self, name: &str) -> crate::Result<()> {
        self.on_tool_call(name);
        Ok(())
    }
    /// The sandbox is about to evaluate this code.
    fn on_cell_start(&self, _code: &str) {}
    fn try_on_cell_start(&self, code: &str) -> crate::Result<()> {
        self.on_cell_start(code);
        Ok(())
    }
    /// The shaped cell result that will be fed back to the model.
    fn on_cell_result(&self, _shaped: &str) {}
    fn try_on_cell_result(&self, shaped: &str) -> crate::Result<()> {
        self.on_cell_result(shaped);
        Ok(())
    }
    /// A conversation has persisted history, but its live sandbox runtime had to be reopened.
    ///
    /// Product sinks can surface this distinction from a brand-new empty-history session so the
    /// user knows that prior REPL declarations and other ephemeral runtime state are unavailable.
    fn on_runtime_reset(&self) {}
    fn try_on_runtime_reset(&self) -> crate::Result<()> {
        self.on_runtime_reset();
        Ok(())
    }
    /// Backend-specific structured runtime event. Payloads must already be bounded and redacted.
    fn on_runtime_event(&self, _event_type: &str, _payload: &serde_json::Value) {}
    fn try_on_runtime_event(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> crate::Result<()> {
        self.on_runtime_event(event_type, payload);
        Ok(())
    }
    /// The model produced a final answer (no tool call).
    fn on_final(&self, _text: &str) {}
    fn try_on_final(&self, text: &str) -> crate::Result<()> {
        self.on_final(text);
        Ok(())
    }
    /// The streamed turn finished (after the last delta, before any cell runs).
    fn on_turn_end(&self) {}
    fn try_on_turn_end(&self) -> crate::Result<()> {
        self.on_turn_end();
        Ok(())
    }
}

/// An [`EventSink`] that drops everything. Useful for tests and headless runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {}
