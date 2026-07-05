# 7. The host SDK / standard library

The JS/TS runtime exposes one fixed prelude plus capability-gated host namespaces. The model still
gets only one chat-native tool, `execute(code)` (§5.2); SDK growth happens inside the sandbox and
through progressive disclosure, not by adding chat-native tools.

First-pass globals:

- `print(...items)` — append to capped stdout.
- `display(value, opts?)` — **synchronous** intended output; buffered until cell finish and preferred
  by result shaping (§5.4).
- `tools.search(query)` / `tools.docs(name)` / `tools.call(name, args)` — progressive disclosure over
  the host capability catalog. `tools.docs("fs.read")` is the model's SDK-definition lookup: it
  returns signature, machine-readable schemas, examples, errors, grants, and approval policy for a
  capability without adding another chat-native tool. New capabilities register behind `tools.call`;
  the op layer stays small.
- `resources.read/preview/list(...)` — uniform, scheme-dispatched resource resolver (§9.2),
  including the live `artifact://`, `workspace://session`, `linked://`, `project://`, `memory://`,
  `agent://`, and `history://` handlers when their grants are present. `skill://...` labels are
  prompt-composition provenance only in the current runtime; `drive://` and `cron://` are reserved
  URI shapes; reads for unregistered schemes must fail closed until the owning milestone registers a
  handler and grants.
- `artifacts.put/get/slice/list(...)` — session artifact store; large outputs return `artifact://`
  handles.
- `fs.read/write/ls/find(...)` — workspace / linked-folder filesystem access through grants.
- `code.search/edit(...)` — regex search plus JSON-hunk surgical edits in the first pass.
- `proc.run(cmd, args, opts?)` — allowlisted argv-vector process execution; never a shell string.
- `http.get(url)` — current M1/P0 default-deny deterministic allowlist helper; it is not ambient
  network egress, not `fetch()`, and not a production egress policy.
- `agents.run/spawn/parallel/msg/send/wait/inbox/list(...)` — capability-gated sub-agent
  orchestration, defined only in sessions holding an `agents.*` grant. Ungranted sessions keep
  `agents` as `undefined`; the remaining P3-plus/full surface still owns `pipeline`, `broadcast`,
  and active supervision.

Reserved first-pass globals:

- `secrets`, `memory`, and `skills` are explicitly set to `undefined`. This makes feature checks
  safe while keeping secrets, `memory.*`, and skills closed until their backing crates and policies
  exist. This does **not** close the P2 `memory://` resource route; memory reads go through
  `resources.read(...)` and the `resources.read:memory` grant. `agents` starts as `undefined` too,
  then the P3 sandbox prelude replaces it with `AgentsNamespace` only when the session has an
  `agents.*` grant. If a namespace exists but a method is incomplete, that method throws
  `NotImplementedError`. Likewise, `skill://...` is not a readable `ResourceUri` today even though
  composed prompts may use it as a section label for injected skill markdown.

Never exposed:

- raw `Deno.*`, raw `fetch`, raw host filesystem/process/network APIs, raw shell strings, environment
  variables, npm/package installation, browser globals, or Node built-ins such as `node:fs` and
  `node:child_process`.

### 7.1 Authoritative TypeScript surface

The checked-in SDK type artifact lives at `docs/sdk/tm-runtime.d.ts`. It is the source file clients
and tests should reference; the excerpt below is an abbreviated design-doc view, not a second source
of truth.

```ts
/**
 * TempestMiku JS/TS runtime prelude.
 *
 * P0/P2 surface: no ambient filesystem, process, network, secret, shell, or
 * host access. Every external effect goes through capability-checked SDK
 * namespaces. P2 memory is exposed as memory:// resources behind
 * resources.read:memory, not as a memory.* namespace. Bundled skill
 * markdown may be labeled skill://... inside composed prompts, but that
 * label is not a resources.read/list/preview surface until the P4/P7 skill
 * lifecycle work registers a handler and grants.
 *
 * P3/P3-plus agents surface: `agents` is defined only in sessions holding the
 * required agents.* grant. In ungranted sessions it remains `undefined`.
 * P3 shipped run, spawn, parallel, and msg; the first P3-plus foundation slice
 * adds live per-actor inbox delivery through send, wait, inbox, and list.
 * Child approval routing through the live HTTP broker is now in place; pipeline,
 * broadcast, cancel, restart, and stricter protocol enforcement remain later
 * P3-plus work.
 */

export {};

declare global {
  function print(...items: unknown[]): void;
  function display(value: DisplayValue, opts?: DisplayOptions): void;

  const tools: ToolsNamespace;
  const resources: ResourcesNamespace;
  const artifacts: ArtifactsNamespace;
  const fs: FsNamespace;
  const code: CodeNamespace;
  const proc: ProcNamespace;
  const http: HttpNamespace;

  var secrets: undefined;
  var memory: undefined;
  var skills: undefined;
  const agents: AgentsNamespace | undefined;
}

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonObject | JsonArray;
interface JsonObject { [key: string]: JsonValue; }
interface JsonArray extends Array<JsonValue> {}

type MimeType = string;
type CapabilityName = string;
type ArtifactUri = `artifact://${string}`;
type SkillPromptLabel = `skill://${string}`;

type MemoryResourceUri =
  | "memory://root"
  | "memory://user-model"
  | `memory://profile/${string}/facts/${string}`
  | `memory://scopes/${string}/chunks/${string}`;

type ProjectResourceUri = `project://${string}`;

type ResourceUri =
  | `artifact://${string}`
  | `agent://${string}`
  | `history://${string}`
  | MemoryResourceUri
  | `drive://${string}`
  | `cron://${string}`
  | `workspace://session/${string}`
  | `linked://${string}/${string}`
  | ProjectResourceUri;

type SdkPath =
  | `${string}:`
  | `${string}:${string}`
  | `linked://${string}/${string}`;

type ResourceSelector = string;

interface HostError extends Error {
  name:
    | "CapabilityDeniedError"
    | "ApprovalDeniedError"
    | "ApprovalTimeoutError"
    | "NotFoundError"
    | "NotImplementedError"
    | "InvalidPathError"
    | "InvalidArgsError"
    | "QuotaExceededError"
    | "TimeoutError"
    | "OutputTruncatedError"
    | "HostCallError";
  capability?: string;
  path?: string;
  uri?: string;
  retryable: boolean;
  details: JsonValue;
}

type DisplayValue =
  | string
  | number
  | boolean
  | null
  | JsonValue
  | DisplayMarkdown
  | DisplayTable
  | ArtifactRef
  | ResourceContent;

interface DisplayOptions {
  kind?: "text" | "markdown" | "json" | "table" | "image" | "binary";
  title?: string;
  mime?: MimeType;
  filename?: string;
  artifact?: boolean;
}

interface DisplayMarkdown {
  kind: "markdown";
  markdown: string;
  title?: string;
}

interface DisplayTable {
  kind: "table";
  columns?: string[];
  rows: Array<Record<string, JsonValue> | JsonValue[]>;
  title?: string;
}

interface ToolsNamespace {
  /**
   * tools.search(query: string, opts?: ToolSearchOptions): Promise<ToolSummary[]>
   *
   * Search the runtime capability catalog without loading the whole SDK into
   * the model context. Results include host-dispatched capabilities plus
   * docs-only entries for core direct namespace methods.
   */
  search(query: string, opts?: ToolSearchOptions): Promise<ToolSummary[]>;
  /** tools.docs(name: CapabilityName): Promise<ToolDocs> */
  docs(name: CapabilityName): Promise<ToolDocs>;
  /**
   * tools.call<T = unknown>(name: CapabilityName, args?: JsonValue): Promise<T>
   *
   * Dispatch a capability-gated host call by name. Prefer the typed namespace
   * wrappers when one exists; unknown or ungranted capabilities fail closed.
   */
  call<T = unknown>(name: CapabilityName, args?: JsonValue): Promise<T>;
}

interface ToolSearchOptions {
  namespace?: string;
  limit?: number;
}

interface ToolSummary {
  name: CapabilityName;
  namespace: string;
  summary: string;
  sensitive: boolean;
  granted: boolean;
}

interface ToolDocs {
  name: CapabilityName;
  namespace: string;
  summary: string;
  description?: string;
  signature: string;
  argsSchema: JsonObject;
  resultSchema?: JsonObject;
  examples: ToolExample[];
  errors: ToolErrorDoc[];
  grants: GrantDoc[];
  sensitive: boolean;
  approval: "none" | "on-write" | "on-external" | "always" | "policy";
  since: string;
  stability: "stable" | "experimental" | "reserved" | "deprecated";
}

interface ToolExample {
  title?: string;
  code: string;
  notes?: string;
}

interface ToolErrorDoc {
  name: HostError["name"];
  when: string;
  retryable: boolean;
}

interface GrantDoc {
  kind:
    | "catalog"
    | "capability"
    | "workspace"
    | "linked-folder"
    | "network"
    | "process"
    | "secret"
    | "memory"
    | "artifact";
  description: string;
}

interface ResourcesNamespace {
  read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>;
  preview(uri: ResourceUri): Promise<ResourceContent>;
  list(uri?: ResourceUri): Promise<ResourceEntry[]>;
}

interface ResourceContent {
  uri: ResourceUri;
  kind: ResourceKind;
  mime: MimeType;
  title?: string;
  sizeBytes: number;
  selector?: ResourceSelector;
  hasMore: boolean;
  content: string;
  preview: string;
}

interface ResourceEntry {
  uri: ResourceUri;
  name: string;
  kind: ResourceKind | "directory" | "scheme";
  title?: string;
  sizeBytes?: number;
  modifiedAt?: string;
}

type ResourceKind =
  | "text"
  | "markdown"
  | "json"
  | "table"
  | "image"
  | "binary"
  | "directory"
  | "log"
  | "memory_root"
  | "memory_user_model"
  | "memory_profile_fact"
  | "memory_recall_chunk"
  | "project_view"
  | (string & {});

interface ArtifactsNamespace {
  put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef;
  get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent>;
  slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent>;
  list(): ArtifactRef[];
}

type ArtifactInput = string | JsonValue;

interface ArtifactPutOptions {
  title?: string;
  mime?: MimeType;
  kind?: ResourceKind;
  filename?: string;
}

interface ArtifactReadOptions {
  selector?: ResourceSelector;
}

interface ArtifactRef {
  uri: ArtifactUri;
  id: string;
  kind: ResourceKind;
  mime: MimeType;
  title?: string;
  sizeBytes: number;
  preview: string;
}

interface FsNamespace {
  read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>;
  write(path: SdkPath, data: string, opts?: FsWriteOptions): Promise<FsWriteResult>;
  ls(path?: SdkPath, opts?: FsListOptions): Promise<FsEntry[]>;
  find(patterns: string | string[], opts?: FsFindOptions): Promise<FsEntry[]>;
}

interface FsReadOptions {
  selector?: ResourceSelector;
  raw?: boolean;
}

interface FsWriteOptions {
  createParents?: boolean;
  overwrite?: boolean;
  mime?: MimeType;
}

interface FsWriteResult {
  path: SdkPath;
  uri: ResourceUri;
  bytesWritten: number;
  created: boolean;
  overwritten: boolean;
}

interface FsListOptions {
  recursive?: boolean;
  limit?: number;
  includeHidden?: boolean;
}

interface FsFindOptions {
  cwd?: SdkPath;
  limit?: number;
  includeHidden?: boolean;
  respectGitignore?: boolean;
}

interface FsEntry {
  path: SdkPath;
  uri: ResourceUri;
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  sizeBytes?: number;
  modifiedAt?: string;
}

interface CodeNamespace {
  search(query: CodeSearchQuery): Promise<CodeSearchResult[]>;
  edit(patch: PatchEdit, opts?: CodeEditOptions): Promise<CodeEditResult>;
}

interface CodeSearchQuery {
  pattern: string;
  paths: SdkPath[];
  caseSensitive?: boolean;
  regex?: boolean;
  contextLines?: number;
  limit?: number;
}

interface CodeSearchResult {
  path: SdkPath;
  uri: ResourceUri;
  line: number;
  column: number;
  text: string;
  before: string[];
  after: string[];
  tag: string;
}

interface PatchEdit {
  path: SdkPath;
  tag?: string;
  hunks: PatchHunk[];
}

type PatchHunk =
  | ReplaceLinesHunk
  | InsertHunk
  | DeleteLinesHunk
  | MoveFileHunk
  | RemoveFileHunk;

interface ReplaceLinesHunk {
  op: "replace";
  startLine: number;
  endLine: number;
  lines: string[];
}

interface InsertHunk {
  op: "insert";
  at: "head" | "tail" | "before" | "after";
  line?: number;
  lines: string[];
}

interface DeleteLinesHunk {
  op: "delete";
  startLine: number;
  endLine: number;
}

interface MoveFileHunk {
  op: "move";
  dest: SdkPath;
}

interface RemoveFileHunk {
  op: "remove";
}

interface CodeEditOptions {
  format?: boolean;
}

interface CodeEditResult {
  path: SdkPath;
  changed: boolean;
  diff: string;
  newTag?: string;
  diagnostics: Diagnostic[];
}

interface Diagnostic {
  path: SdkPath;
  line: number;
  column?: number;
  severity: "error" | "warning" | "info" | "hint";
  message: string;
  source?: string;
}

interface ProcNamespace {
  run(cmd: string, args?: string[], opts?: ProcRunOptions): Promise<ProcOutput>;
}

interface ProcRunOptions {
  cwd?: SdkPath;
  timeoutMs?: number;
  /** Reserved in P0; non-empty env overrides are rejected. */
  env?: Record<string, string>;
  /** Reserved in P0; non-empty stdin is rejected. */
  stdin?: string;
  outputBytes?: number;
}

interface ProcOutput {
  cmd: string;
  args: string[];
  cwd: SdkPath;
  exitCode: number;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  durationMs: number;
  truncated: boolean;
  artifact?: ArtifactRef;
}

interface HttpNamespace {
  /**
   * Experimental M1/P0 default-deny deterministic allowlist helper. This is
   * not ambient network egress, not fetch(), and not a production egress
   * policy. Non-allowlisted URLs fail closed with CapabilityDeniedError;
   * production egress hardening remains deferred.
   */
  get(url: string): Promise<string>;
}
```

Parity is enforced in the sandbox tests: the runtime-exposed direct namespace methods (`tools.*`,
`resources.*`, `artifacts.*`, `fs.*`, `code.*`, `proc.run`, and `http.get`) are enumerated from the
installed prelude, each `tools.docs(name).signature` must appear in `docs/sdk/tm-runtime.d.ts`, and
each docs entry must carry schemas, examples, fail-closed errors, grants, approval, since, and
stability metadata. Until generation exists, update the checked-in `.d.ts` snapshot and catalog docs
together.

### 7.2 Progressive disclosure flow

```mermaid
sequenceDiagram
    participant M as Model
    participant Cx as Code (sandbox)
    participant H as Host registry
    M->>Cx: execute(tools.search("edit rust file"))
    Cx->>H: op_tools_search
    H-->>Cx: [{name:"code.edit", summary:"JSON-hunk patch edit", granted:true}]
    Cx-->>M: 1 match (summary only)
    M->>Cx: execute(tools.docs("code.edit"))
    Cx->>H: op_tools_docs
    H-->>Cx: full signature + examples
    Cx-->>M: docs loaded on demand
    M->>Cx: execute(await code.edit({...}); display("patched"))
```

The catalog never sits in the system prompt; tokens are spent only on what the run actually touches.
It includes host-dispatched capabilities plus docs-only entries for core `tools.*`, `resources.*`,
and `artifacts.*` primitives; those core entries document the direct namespace methods and do not
imply `tools.call(...)` routing.

### 7.3 Semantics

- `display(...)` is synchronous. It records an intended output item and returns immediately; the host
  buffers display items and shapes/spills them after the cell completes.
- `fs.read(...)` and `resources.read(...)` always return `ResourceContent`, never a naked string.
- `code.edit(...)` accepts JSON hunks. Human-facing patch grammars may exist outside the runtime, but
  the TS SDK does not require string patch construction.
- `proc.run(...)` accepts `cmd` + `args` only. `proc.run("cargo test")` is invalid; use
  `proc.run("cargo", ["test"], { cwd: "tempestmiku:" })`.
- Missing grants fail closed with `CapabilityDeniedError`. Unknown capabilities fail closed through
  `tools.call`. Future namespaces that exist but have incomplete methods throw `NotImplementedError`.
- P2 memory reads use `resources.read("memory://...")` through `resources.read:memory`; the global
  `memory` namespace remains `undefined` until a later `memory.*` API is explicitly shipped.
- P2 skill markdown is prompt-composed under `skill://...` labels only. `skill://...` is not a
  resource URI yet; `resources.read/preview/list("skill://...")` fails closed as an unknown scheme
  until P4/P7 ships the skill resource lifecycle.

### 7.4 Deferred namespace placement

The runtime keeps future namespaces closed until their product milestone owns the storage, resource,
approval, and audit boundaries. The root roadmap is canonical (§28), but the SDK placement is:

| Namespace / surface | Target milestone | SDK rule |
|---|---|---|
| `memory.*` | P2/P4 split | P2 exposes memory reads as `memory://` resources through `resources.read:memory`; the `memory` global remains `undefined`. A future explicit `memory.*` namespace may expose minimum profile/user recall and state-capture calls, while P4 owns full scoped memory, pgvector/FTS, and dream-queue writes. |
| `agents.*` | P3/P3+ | P3 exposes `agents.run`, `agents.spawn`, `agents.parallel`, and `agents.msg` with `tm-agents`, actor lifecycle, mailbox/roster, supervision defaults, and `agent://` resource handling. The first P3-plus foundation slice adds live bounded-inbox `agents.send`, `agents.wait`, `agents.inbox`, and `agents.list`, plus child approval routing through the live HTTP broker; `pipeline`, `broadcast`, active restart/cancel, and stricter protocol enforcement remain P3-plus. |
| `skills.*` / `skill://` reads | P4/P7 split | P2 may compose bundled skill markdown under `skill://...` prompt labels only. `skills` remains `undefined`, and `resources.read/preview/list("skill://...")` must fail closed until P4/P7 defines approval-gated proposals plus safe import/version/reload semantics, provenance, audit/replay, and MCP import gates. |
| `drive.*` | P5 | Add with `tm-drive`, project memory scopes, virtual dirs, transducers, and drive organizer flows. |
| `http.*` hardening | P5 or P7 | Keep current `http.get` as a default-deny deterministic allowlisted helper with no open egress; add byte/request caps, redirect policy, audit logging, and production allowlists only when research or hardening needs live egress. |
| `secrets.use` | P7 | Requires opaque egress-scoped handles from a secret broker; secret values must never materialize in JS heap, artifacts, or model context. |
| `code.ast` / `code.lsp` | P1.5/P2 tech slice | Native cutover makes this possible, but add only for a concrete structured-edit user; do not block the P2 companion baseline. |
