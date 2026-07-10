# 11. OpenAI-compatible integration

### 11.1 Request

```jsonc
POST {base_url}/v1/chat/completions
{
  "model": "…",
  "messages": [ /* system, user, assistant(tool_calls), tool(result), … */ ],
  "tools": [ /* the single execute() function from §5.2 */ ],
  "tool_choice": "auto",
  "parallel_tool_calls": false,
  "stream": true
}
```

### 11.2 Loop mapping

- Assistant turn with `tool_calls[0].function.name == "execute"` → parse `arguments.code`, run,
  append a `role:"tool"` message keyed by `tool_call_id`.
- Assistant turn with content and no tool call → final answer; stop.
- More than one tool call, any name other than `execute`, missing id, malformed arguments, empty
  code, or a truncated tool-call completion is a protocol error and executes nothing (§5.2).

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
