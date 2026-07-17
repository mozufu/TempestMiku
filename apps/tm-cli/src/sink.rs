use std::{fs::OpenOptions, io::Write, path::PathBuf};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tm_core::{Error as CoreError, EventSink};

/// Streams assistant tokens to stdout live; cell telemetry to stderr (dimmed).
pub(super) type StdoutSink = StreamingSink<std::io::Stdout, std::io::Stderr>;

pub(super) struct StreamingSink<O, E> {
    pub(super) stdout: Mutex<O>,
    pub(super) stderr: Mutex<E>,
    pub(super) event_log: Option<Mutex<Box<dyn Write + Send>>>,
    dim: bool,
}

impl StdoutSink {
    pub(super) fn stdio(event_log: Option<&PathBuf>) -> Result<Self> {
        let mut sink = StreamingSink::new(std::io::stdout(), std::io::stderr(), true);
        if let Some(path) = event_log {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating event log parent {}", parent.display()))?;
            }
            let writer = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)
                .with_context(|| format!("opening event log {}", path.display()))?;
            sink.event_log = Some(Mutex::new(Box::new(writer)));
        }
        Ok(sink)
    }
}

impl<O, E> StreamingSink<O, E> {
    pub(super) fn new(stdout: O, stderr: E, dim: bool) -> Self {
        Self {
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
            event_log: None,
            dim,
        }
    }
}

impl<O, E> StreamingSink<O, E>
where
    O: Write + Send,
    E: Write + Send,
{
    fn write_stderr_line(&self, line: impl AsRef<str>) {
        let mut stderr = self.stderr.lock();
        if self.dim {
            let _ = writeln!(stderr, "\x1b[2m{}\x1b[0m", line.as_ref());
        } else {
            let _ = writeln!(stderr, "{}", line.as_ref());
        }
        let _ = stderr.flush();
    }

    pub(super) fn write_event(&self, event: Value) -> tm_core::Result<()> {
        let Some(writer) = &self.event_log else {
            return Ok(());
        };
        let mut writer = writer.lock();
        serde_json::to_writer(&mut *writer, &event)
            .map_err(|err| CoreError::EventSink(err.to_string()))?;
        writeln!(writer).map_err(|err| CoreError::EventSink(err.to_string()))?;
        writer
            .flush()
            .map_err(|err| CoreError::EventSink(err.to_string()))
    }

    fn write_stdout_delta(&self, delta: &str) {
        let mut stdout = self.stdout.lock();
        let _ = write!(stdout, "{delta}");
        let _ = stdout.flush();
    }

    fn write_final_newline(&self) {
        let mut stdout = self.stdout.lock();
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }
}

impl<O, E> EventSink for StreamingSink<O, E>
where
    O: Write + Send,
    E: Write + Send,
{
    fn on_reasoning(&self, delta: &str) {
        let _ = self.write_event(json!({"type": "reasoning_delta", "text": delta}));
    }

    fn try_on_reasoning(&self, delta: &str) -> tm_core::Result<()> {
        self.write_event(json!({"type": "reasoning_delta", "text": delta}))
    }

    fn on_text(&self, delta: &str) {
        self.write_stdout_delta(delta);
        let _ = self.write_event(json!({"type": "text_delta", "text": delta}));
    }

    fn try_on_text(&self, delta: &str) -> tm_core::Result<()> {
        self.write_stdout_delta(delta);
        self.write_event(json!({"type": "text_delta", "text": delta}))
    }

    fn on_tool_call(&self, name: &str) {
        self.write_stderr_line(format!("· tool call: {name}"));
        let _ = self.write_event(json!({"type": "tool_call", "name": name}));
    }

    fn try_on_tool_call(&self, name: &str) -> tm_core::Result<()> {
        self.write_stderr_line(format!("· tool call: {name}"));
        self.write_event(json!({"type": "tool_call", "name": name}))
    }

    fn on_cell_start(&self, code: &str) {
        self.write_stderr_line(format!("· executing cell ({} bytes)", code.len()));
        let _ = self.write_event(json!({"type": "cell_start", "code": code}));
    }

    fn try_on_cell_start(&self, code: &str) -> tm_core::Result<()> {
        self.write_stderr_line(format!("· executing cell ({} bytes)", code.len()));
        self.write_event(json!({"type": "cell_start", "code": code}))
    }

    fn on_cell_result(&self, shaped: &str) {
        self.write_stderr_line("· result → model:");
        for line in shaped.lines() {
            self.write_stderr_line(format!("  {line}"));
        }
        let _ = self.write_event(json!({"type": "cell_result", "result": shaped}));
    }

    fn try_on_cell_result(&self, shaped: &str) -> tm_core::Result<()> {
        self.write_stderr_line("· result → model:");
        for line in shaped.lines() {
            self.write_stderr_line(format!("  {line}"));
        }
        self.write_event(json!({"type": "cell_result", "result": shaped}))
    }

    fn on_turn_end(&self) {
        let _ = self.write_event(json!({"type": "turn_end"}));
    }

    fn try_on_turn_end(&self) -> tm_core::Result<()> {
        self.write_event(json!({"type": "turn_end"}))
    }

    fn on_final(&self, text: &str) {
        self.write_final_newline();
        let _ = self.write_event(json!({"type": "final", "text": text}));
    }

    fn try_on_final(&self, text: &str) -> tm_core::Result<()> {
        self.write_final_newline();
        self.write_event(json!({"type": "final", "text": text}))
    }
}
