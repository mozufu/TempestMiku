# TempestMiku — Code-Execution Agent Runtime

> Status: design draft v0.1
> Scope: the core runtime. UI, deployment, and multi-tenant concerns are out of scope here.

Design doc, split by section. Read in order or jump to a section:

1. [TL;DR — the bet](core/01-tldr-the-bet.md)
2. [Why code execution beats classic tool calls](core/02-why-code-execution.md)
3. [Core principles](core/03-core-principles.md)
4. [Architecture](core/04-architecture.md)
5. [The agent loop](core/05-agent-loop.md)
6. [The REPL / sandbox](core/06-repl-sandbox.md)
7. [The host SDK / standard library](core/07-host-sdk.md)
8. [Security model](core/08-security-model.md)
9. [Context, artifacts & resource resolution](core/09-context-artifacts.md)
10. [Rust implementation](core/10-rust-implementation.md)
11. [OpenAI-compatible integration](core/11-openai-integration.md)
12. [Observability & reproducibility](core/12-observability.md)
13. [End-to-end example](core/13-end-to-end-example.md)
14. [Roadmap](../../ROADMAP.md#core-runtime-milestones-14)
15. [Open questions / risks](core/15-open-questions.md)
16. [References](core/16-references.md)

## Product layer (the companion on top)

> Scope: TempestMiku as a product — the **Tempest Miku** companion (persona, memory, storage,
> sub-agents, clients). Extends the core; does **not** change the bet (§01). A **rewrite** of the
> current `hermes-agent` deployment (§29); canonical spec: `SOUL.md` + skills + `honcho.json`.

20. [Product overview](product/20-product-overview.md)
21. [Persona & modes & routing](product/21-persona-and-modes.md)
22. [Memory — self-built recall + "dreaming"](product/22-memory-dreaming.md)
23. [Sub-agents & orchestration](product/23-agents-orchestration.md)
24. [Smart file storage (drive)](product/24-drive-storage.md)
25. [Coding-agent SDK & artifacts](product/25-coding-agent-sdk.md)
26. [Self-evolution](product/26-self-evolution.md)
27. [Server, scheduler & clients](product/27-server-and-clients.md)
28. [Product roadmap](../../ROADMAP.md#product-roadmap-28)
29. [Parity baseline — the current deployment](product/29-parity-baseline.md)

## Experimental: the `tm` language

> Status: **fun / experimental**, not a committed milestone. A design exploration of what an
> agent-first language for `execute(code)` would look like — a persistent effectful REPL with
> a structured execution trace and declarative presentation for the client UI.
> Does not change §6.1 (TS on `deno_core` is the shipping sandbox language); `tm` would be a
> third `Sandbox` backend that gives capability gating, approval suspension, provenance, replay,
> and UI projection one observable execution model. Its source syntax is now fixed in the
> experimental [tm language reference](tm/07-language-reference.md). See
> [`tm/README.md`](tm/README.md).
