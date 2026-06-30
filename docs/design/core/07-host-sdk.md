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
- `resources.read/preview/list(...)` — uniform, scheme-dispatched resource resolver (§9.2).
- `artifacts.put/get/slice/list(...)` — session artifact store; large outputs return `artifact://`
  handles.
- `fs.read/write/ls/find(...)` — workspace / linked-folder filesystem access through grants.
- `code.search/edit(...)` — regex search plus JSON-hunk surgical edits in the first pass.
- `proc.run(cmd, args, opts?)` — allowlisted argv-vector process execution; never a shell string.
- `http.get(url)` — current M1 deterministic allowlist helper; production network egress policy is
  still deferred.

Reserved first-pass globals:

- `secrets`, `memory`, `skills`, and `agents` are explicitly set to `undefined`. This makes feature
  checks safe while keeping secrets, memory, skills, and sub-agents closed until their backing crates
  and policies exist. If a future namespace exists but a method is incomplete, that method throws
  `NotImplementedError`.

Never exposed:

- raw `Deno.*`, raw `fetch`, raw host filesystem/process/network APIs, raw shell strings, environment
  variables, npm/package installation, browser globals, or Node built-ins such as `node:fs` and
  `node:child_process`.

### 7.1 Authoritative TypeScript surface

```ts
/**
 * TempestMiku JS/TS runtime prelude.
 *
 * No ambient filesystem, process, network, secret, shell, or host access.
 * Every external effect goes through capability-checked SDK namespaces.
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
  var agents: undefined;
}

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonObject | JsonArray;
interface JsonObject { [key: string]: JsonValue; }
interface JsonArray extends Array<JsonValue> {}

type MimeType = string;
type CapabilityName = string;
type ArtifactUri = `artifact://${string}`;

type ResourceUri =
  | `artifact://${string}`
  | `agent://${string}`
  | `history://${string}`
  | `memory://${string}`
  | `skill://${string}`
  | `drive://${string}`
  | `cron://${string}`
  | `workspace://session/${string}`
  | `linked://${string}/${string}`
  | `project://${string}/${string}`;

type SdkPath =
  | `workspace:${string}`
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
  retryable?: boolean;
  details?: JsonValue;
}

type DisplayValue =
  | string
  | number
  | boolean
  | null
  | JsonValue
  | Uint8Array
  | ArrayBuffer
  | DisplayMarkdown
  | DisplayTable
  | DisplayImage
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

interface DisplayImage {
  kind: "image";
  data: Uint8Array | ArrayBuffer | ArtifactUri;
  mime: "image/png" | "image/jpeg" | "image/webp" | "image/gif" | string;
  alt?: string;
  title?: string;
}

interface ToolsNamespace {
  search(query: string, opts?: ToolSearchOptions): Promise<ToolSummary[]>;
  docs(name: CapabilityName): Promise<ToolDocs>;
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
  sensitive?: boolean;
  granted: boolean;
}

interface ToolDocs {
  name: CapabilityName;
  namespace: string;
  summary: string;
  description?: string;
  signature?: string;
  argsSchema: JsonObject;
  resultSchema?: JsonObject;
  examples?: ToolExample[];
  errors?: ToolErrorDoc[];
  grants?: GrantDoc[];
  sensitive?: boolean;
  approval?: "none" | "on-write" | "on-external" | "always" | "policy";
  since?: string;
  stability?: "stable" | "experimental" | "reserved" | "deprecated";
}

interface ToolExample {
  title?: string;
  code: string;
  notes?: string;
}

interface ToolErrorDoc {
  name: HostError["name"];
  when: string;
  retryable?: boolean;
}

interface GrantDoc {
  kind: "workspace" | "linked-folder" | "network" | "process" | "secret" | "memory" | "artifact";
  description: string;
}

interface ResourcesNamespace {
  read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>;
  preview(uri: ResourceUri): Promise<ResourcePreview>;
  list(uri?: ResourceUri): Promise<ResourceEntry[]>;
}

interface ResourceContent {
  uri: ResourceUri;
  kind: ResourceKind;
  mime: MimeType;
  title?: string;
  sizeBytes?: number;
  selector?: ResourceSelector;
  hasMore?: boolean;
  content?: string;
  bytes?: Uint8Array;
  preview?: string;
  artifact?: ArtifactRef;
}

interface ResourcePreview {
  uri: ResourceUri;
  kind: ResourceKind;
  mime: MimeType;
  title?: string;
  sizeBytes?: number;
  preview?: string;
  hasMore?: boolean;
}

interface ResourceEntry {
  uri: ResourceUri;
  name: string;
  kind: ResourceKind | "directory";
  mime?: MimeType;
  sizeBytes?: number;
  modifiedAt?: string;
  preview?: string;
}

type ResourceKind =
  | "text"
  | "markdown"
  | "json"
  | "table"
  | "image"
  | "binary"
  | "directory"
  | "log";

interface ArtifactsNamespace {
  put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef;
  get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent>;
  slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent>;
  list(): ArtifactInfo[];
}

type ArtifactInput = string | Uint8Array | ArrayBuffer | JsonValue;

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
  preview?: string;
}

interface ArtifactInfo {
  uri: ArtifactUri;
  id: string;
  kind: ResourceKind;
  mime: MimeType;
  title?: string;
  sizeBytes: number;
  createdAt: string;
  preview?: string;
}

interface FsNamespace {
  read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>;
  write(path: SdkPath, data: string | Uint8Array | ArrayBuffer, opts?: FsWriteOptions): Promise<FsWriteResult>;
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
  uri?: ResourceUri;
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
  uri?: ResourceUri;
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
  uri?: ResourceUri;
  line: number;
  column?: number;
  text: string;
  before?: string[];
  after?: string[];
  tag?: string;
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
  diff?: string;
  newTag?: string;
  diagnostics?: Diagnostic[];
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
  /** Reserved in P0; non-empty overrides are rejected. */
  env?: Record<string, string>;
  /** Reserved in P0; stdin is rejected. */
  stdin?: string | Uint8Array;
  outputBytes?: number;
}

interface ProcOutput {
  cmd: string;
  args: string[];
  cwd?: SdkPath;
  exitCode: number;
  signal?: string;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  durationMs: number;
  truncated: boolean;
  artifact?: ArtifactRef;
}

interface HttpNamespace {
  get(url: string): Promise<string>;
}
```

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
