# 1. TL;DR — the bet

Most agent runtimes wire the model to capabilities through **structured tool calls**: every
tool is a JSON schema injected into the prompt, the model emits one call, the runtime runs it,
the raw result is pasted back into context, repeat. Call this **Tool Call 1.0**.

TempestMiku bets on the alternative Anthropic described in *Code execution with MCP* (Nov 2025):
the model's primary interface is a **single code-execution tool — a persistent REPL**. The model
writes code that *gathers* data (by calling host capabilities), *processes* it (filter / map /
join / reduce, with real loops and conditionals), and *decides what to surface* back into its own
context. Capabilities are presented to the code as a callable **SDK**, discovered on demand, not
as a wall of JSON schemas in the system prompt.

The whole runtime is one control loop around that idea:

```
model writes code ──▶ sandbox runs it (calls host capabilities) ──▶ only the distilled
output returns to context ──▶ model writes the next cell ──▶ … ──▶ final answer
```

Language: **Rust**. Model backend: any **OpenAI-compatible** chat-completions endpoint.

References: [Anthropic — Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp),
[Anthropic — Agent Skills / progressive disclosure](https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills),
[Simon Willison's summary](https://simonwillison.net/2025/Nov/4/code-execution-with-mcp/).
