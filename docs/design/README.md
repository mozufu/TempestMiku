# TempestMiku — section-numbered specification

> Scope: the detailed as-built specification behind [`../PRODUCT.md`](../PRODUCT.md). Core runtime,
> product layer, and the `tm` language are split by stable section number; delivery sequencing and
> dated acceptance records live under [`../history/`](../history/README.md).

Read in order or jump to a section:

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
14. [Delivery history](core/14-roadmap.md)
15. [Open questions / risks](core/15-open-questions.md)
16. [References](core/16-references.md)

## Product layer (the companion on top)

> Scope: TempestMiku as a product — the **Tempest Miku** companion (persona, memory, storage,
> sub-agents, clients). Extends the core; does **not** change the bet (§01). The source behavior is
> pinned by §29; the complete present-tense product overview is [`../PRODUCT.md`](../PRODUCT.md).

20. [Product overview](product/20-product-overview.md)
21. [Persona & modes & routing](product/21-persona-and-modes.md)
22. [Memory — self-built recall + "dreaming"](product/22-memory-dreaming.md)
23. [Sub-agents & orchestration](product/23-agents-orchestration.md)
24. [Smart file storage (drive)](product/24-drive-storage.md)
25. [Coding-agent SDK & artifacts](product/25-coding-agent-sdk.md)
26. [Self-evolution](product/26-self-evolution.md)
27. [Server, scheduler & clients](product/27-server-and-clients.md)
28. [Delivery history](product/28-product-roadmap.md)
29. [Parity baseline — the current deployment](product/29-parity-baseline.md)
30. [Projects — subject entities, and what drive means](product/30-projects.md)

## Shipping language: `tm`

> Status: **implemented and sole runtime**. tm is the persistent effectful language for
> `execute(code)`, with structured execution traces and declarative presentation for the client UI.
> Its source syntax is frozen in the
> [tm language reference](tm/07-language-reference.md). See
> [`tm/README.md`](tm/README.md).
