# 11. OpenAI-compatible integration

### 11.1 Request

```jsonc
POST {base_url}/v1/chat/completions
{
  "model": "…",
  "messages": [ /* system, user, assistant(tool_calls), tool(result), … */ ],
  "tools": [ /* the single execute() function from §5.2 */ ],
  "tool_choice": "auto",
  "stream": true
}
```

### 11.2 Loop mapping

- Assistant turn with `tool_calls[0].function.name == "execute"` → parse `arguments.code`, run,
  append a `role:"tool"` message keyed by `tool_call_id`.
- Assistant turn with content and no tool call → final answer; stop.
- Multiple tool calls in one turn: supported, but the default system prompt nudges one `execute`
  per turn (state persists, so there's no need to batch).

### 11.3 Compatibility notes

- **No-tools endpoints:** use the `FencedBlock` protocol (§5.3).
- **Streaming (day 1, default path):** assistant text streams to the UI token-by-token and
  `execute` arguments are accumulated from deltas before the cell runs (§5.5). A running cell's
  stdout may be mirrored to the UI but is folded into one tool result for the model.
- **Token accounting:** record prompt/completion tokens per turn; artifact spill keeps prompt
  growth flat regardless of data volume.
