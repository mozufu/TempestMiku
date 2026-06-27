# 2. Why code execution beats classic tool calls

### Tool Call 1.0 — the failure modes

| Problem | Why it happens |
|---|---|
| **Prompt bloat** | Every tool's full JSON schema is loaded upfront, every turn, whether used or not. Hundreds of tools = tens of thousands of tokens before the user even speaks. |
| **Result bloat** | Every intermediate result is re-tokenized into context. Fetch a 2 MB JSON to extract one field → the whole blob sits in the window. |
| **Weak composition** | Chaining N tools = N round-trips. Filtering, joining, or looping over results is done by the model *in prose*, badly and expensively. |
| **No data-side compute** | The model must reason over raw blobs token-by-token instead of running `data.filter(...)`. |
| **Privacy leak** | Sensitive intermediate values (PII, secrets, large corpora) necessarily pass through the model. |

### Tool Call 2.0 — what code execution buys

- **Progressive disclosure.** The system prompt only says *"you have a REPL and an SDK; discover
  capabilities with `tools.search()` / `tools.docs()`."* Tool definitions are read on demand.
  (Anthropic measures skill discovery at ~80 tokens each vs. loading everything upfront.)
- **Context-efficient results.** Code filters/aggregates *before* anything returns to the model.
  Only what the code explicitly `display()`s or returns reaches context; the 2 MB blob stays in
  the sandbox or becomes an `artifact://` handle.
- **Real control flow.** Loops, conditionals, retries, `Promise.all` fan-out — ordinary code,
  one tool round-trip instead of N.
- **Data never has to touch the model.** Values can flow from source A to sink B entirely inside
  the sandbox; the model orchestrates without seeing the payload.
- **Persistent state & skills.** Variables persist across cells (Jupyter-style); reusable
  functions can be saved as importable skills.

### The cost (be honest)

Running model-authored code demands a **secure sandbox, resource limits, and monitoring**. That
operational surface is the price of admission and the hardest part of this runtime — §7 and §8
are dedicated to it.
