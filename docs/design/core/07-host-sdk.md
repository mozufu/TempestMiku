# 7. The host SDK / standard library

What the code can reach (each function is capability-checked at the boundary):

- `tools.search(query)` / `tools.docs(name)` / `tools.call(name, args)` — **progressive
  disclosure** over the capability catalog (incl. imported MCP tools). Catalog lives host-side;
  only summaries return until `docs()` is called.
- `http.get/post(...)` — egress through the network allowlist, with byte/req caps.
- `fs.read/write/ls(...)` — confined to a per-session workspace root (jail).
- `artifacts.put/slice/get(...)` — large-data store; returns `artifact://` handles.
- `resources.read(uri, sel?)` — uniform, scheme-dispatched read over the resolver registry (§9.2):
  `artifact://` / `agent://` / `history://` / `memory://` / `skill://` / `drive://` / `cron://`.
- `display(value, opts)` — declare an intended output (text/markdown/json/table/image).
- `secrets.use(name)` — opaque handle; see §8.3.
- `skills.save(name, src)` / `import` — persist & reuse model-authored modules across runs
  (Anthropic "skills").

### Progressive disclosure flow

```mermaid
sequenceDiagram
    participant M as Model
    participant Cx as Code (sandbox)
    participant H as Host registry
    M->>Cx: execute(tools.search("salesforce contact"))
    Cx->>H: op_tools_search
    H-->>Cx: [{name:"sf.contacts.find", summary:"..."}]  %% names+summaries only
    Cx-->>M: 3 matches (≈60 tokens)
    M->>Cx: execute(tools.docs("sf.contacts.find"))
    Cx->>H: op_tools_docs
    H-->>Cx: full signature + examples
    Cx-->>M: docs (loaded only now)
    M->>Cx: execute(const c = await tools.call("sf.contacts.find", {...}); display(c.length))
```

The catalog never sits in the system prompt; tokens are spent only on what the run actually
touches.
