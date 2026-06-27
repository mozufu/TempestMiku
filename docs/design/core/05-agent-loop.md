# 5. The agent loop

### 5.1 Control flow

```
seed messages = [system_prompt, user_msg]
loop:
    stream = llm.chat_stream(messages, tools=[EXECUTE_TOOL])   # streaming is the only transport
    acc = Accumulator()
    for delta in stream:                  # text + tool-call-arg fragments, as they arrive
        sink.emit(delta)                  # assistant tokens reach the UI live
        acc.push(delta)
    turn = acc.finish()                   # assembled assistant message (text + tool_calls)
    append turn.message to messages
    if turn has tool_call "execute":
        out = session.eval(code = call.code, budget)
        append tool_result(call.id, shape_result(out)) to messages
        continue
    else:
        return turn.text                  # no tool call ⇒ final answer
until turn_budget exhausted
```

The session is **persistent across iterations** — cell 2 sees the variables cell 1 defined. The
loop ends when the model stops calling `execute` (it has its answer) or a budget is hit.

### 5.2 The one tool

```json
{
  "type": "function",
  "function": {
    "name": "execute",
    "description": "Run code in your persistent REPL session. Variables persist across calls. Only what you display()/return reaches you; everything else stays in the sandbox. Discover capabilities with tools.search()/tools.docs().",
    "parameters": {
      "type": "object",
      "properties": {
        "code": { "type": "string", "description": "Source to evaluate in the session." }
      },
      "required": ["code"]
    }
  }
}
```

That is the *entire* tool surface presented to the model. Capability growth never grows this
schema — it grows the SDK the code discovers at runtime.

### 5.3 Fallback for endpoints without function calling

Some OpenAI-compatible servers don't support `tools`. Provide a `Protocol` switch:

- `NativeTool` (default): use the `execute` function-call mechanism above.
- `FencedBlock`: instruct the model to emit exactly one fenced block
  ` ```run … ``` `; the orchestrator parses the block, runs it, and injects the result as the
  next user message. Same loop, brittler parsing. Used only when native tools are unavailable.

### 5.4 Result shaping (the context-efficiency lever)

`shape_result` turns an `EvalOutput` into the compact tool message the model sees. Policy:

- `stdout` and return value: capped (e.g. 8 KB) with head+tail elision markers.
- `display()` items: the model's *intended* outputs — included (text/markdown/table inline;
  images as blocks; large data as artifact refs).
- Anything over the cap → spilled to the artifact store; the model gets
  `artifact://<id>` + a preview + size, and can re-read slices on demand.
- `error`: message + trimmed traceback.
- A one-line **host-call summary** (which capabilities ran, bytes in/out) so the model — and the
  audit log — know what happened.

The system prompt teaches the model the rule: **compute and filter in code; return only what you
need; park big data as an artifact.**

### 5.5 Streaming (day 1)

Streaming is foundational, not a later add-on. `LlmClient::chat_stream` is the single transport;
the non-streaming `chat` is just "drain the stream and assemble." Two things stream out of every
turn as the bytes arrive:

- **Assistant text** — surfaced token-by-token through an `EventSink` so a UI/CLI renders the
  answer live.
- **Tool-call arguments** — the `execute` call's `{"code": …}` arrives as JSON fragments across
  deltas; an `Accumulator` stitches the fragments (and any text) into one `AssistantTurn` before
  the code runs.

```
SSE chunk ─▶ StreamEvent ─▶ Accumulator.push()   ─▶ AssistantTurn (text + tool_calls)
                     └▶ EventSink.on_text()  (UI, live)
```

What does *not* stream to the model in v1: a *running* cell's stdout. The sandbox still returns one
shaped result per finished cell (§5.4); the cell's live output may be mirrored to the UI but is
folded into a single tool message for the model. See §15.
