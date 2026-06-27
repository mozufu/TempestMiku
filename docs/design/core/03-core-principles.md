# 3. Core principles

1. **Code is the tool interface.** One first-class tool: `execute(code)`. Everything else is an
   SDK function the code may call.
2. **Progressive disclosure by default.** Nothing about a capability enters context until the
   code asks for it.
3. **The context window is a scarce output budget, not a scratchpad.** The sandbox is the
   scratchpad; context holds only decisions and distilled results.
4. **Capability-scoped, least-privilege sandbox.** Code can do *nothing* the host did not
   explicitly grant — no ambient network, filesystem, or secrets.
5. **Secrets by reference, never by value.** Code holds opaque handles; real secret values are
   substituted at the host boundary and never enter the sandbox heap or the model context.
6. **Everything is replayable.** Every cell, host call, and output is recorded; a session can be
   replayed deterministically.
7. **Pluggable everything.** `LlmClient`, `Sandbox`, and the host-function registry are traits.
   Swapping the REPL language or the model backend is a config change.
8. **No raw shell, no escape hatches.** There is no generic `bash`/`shell` capability. The model
   writes TS against typed, curated capabilities instead of escaping to another language; the
   REPL language's package ecosystem is a long-tail safety net, not the primary capability source.
9. **One bridge, a runtime registry.** Every extensible host call routes through a single
   dynamic-dispatch op into a capability **registry**. Adding a capability = register a handler +
   emit a stub — no new op, no rebuild of the bridge, hot-addable at runtime.
