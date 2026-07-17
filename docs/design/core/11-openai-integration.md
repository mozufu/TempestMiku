# 11. OpenAI-compatible integration

### 11.1 Request

```jsonc
POST {base_url}/v1/chat/completions
{
  "model": "…",
  "messages": [ /* system, user, assistant(tool_calls), tool(result), … */ ],
  "tools": [ /* the single execute() function from §5.2 */ ],
  "tool_choice": "auto",
  "parallel_tool_calls": true,
  "stream": true
}
```

### 11.2 Loop mapping

- Assistant turn with one to sixteen `execute` calls → validate the complete batch, then evaluate
  independent `arguments.code` cells through the session batch path, appending one `role:"tool"`
  message per call in response order.
- Assistant turn with content and no tool call → final answer; stop.
- More than sixteen calls, duplicate or missing ids, any name other than `execute`, malformed
  arguments, empty code, or a truncated tool-call completion is a protocol error and executes
  nothing (§5.2). tm-lang derives a forward binding DAG from each cell's free reads and persistent
  writes. Independent cells share the pre-batch snapshot; dependent cells execute once after
  successful producers, and final commits merge deterministically in response order. A producer
  failure blocks dependents with `BatchDependencyError`. Dependencies must point left-to-right.

### 11.3 Compatibility notes

- **No-tools endpoints:** use the `FencedBlock` protocol (§5.3).
- **Streaming (day 1, default path):** assistant text streams to the UI token-by-token and
  `execute` arguments are accumulated from deltas before the cell runs (§5.5). A running cell's
  stdout may be mirrored to the UI but is folded into one tool result for the model.
- **Token accounting:** record prompt/completion tokens per turn; artifact spill keeps prompt
  growth flat regardless of data volume.
- **Transport bounds:** defaults are 10 seconds to connect, 120 seconds idle between response chunks,
  and 300 seconds total request time. One SSE line is capped at 1 MiB, the aggregate completion
  stream at 4 MiB, and non-success response bodies at 64 KiB.
- **Protocol integrity:** malformed JSON/SSE, an oversized line/stream, or EOF before `[DONE]` is an
  error, never a silently shortened completion. Cancellation is selected against connection setup,
  stream reads, pending host waits, and cell evaluation.
- **Secret handling:** `OpenAiConfig` redacts API keys in `Debug`; transport/error text and bounded
  error bodies are redacted before they can reach logs or persisted events.
