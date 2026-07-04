/**
 * TempestMiku JS/TS runtime prelude.
 *
 * P0/P2 surface: no ambient filesystem, process, network, secret, shell, or
 * host access. Every external effect goes through capability-checked SDK
 * namespaces. P2 memory is exposed as memory:// resources behind
 * resources.read:memory, not as a memory.* namespace.
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
  | `skill://${string}`
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
  /**
   * resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>
   *
   * Scheme-dispatched resource read. Current registered schemes include
   * artifact:// and, in the server resource gateway, linked://,
   * workspace://session, project://, and the P2 memory:// surface. Each scheme
   * has its own grant such as resources.read:artifact, resources.read:linked,
   * or resources.read:memory; missing grants and unknown schemes fail closed.
   */
  read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>;
  /** resources.preview(uri: ResourceUri): Promise<ResourceContent> */
  preview(uri: ResourceUri): Promise<ResourceContent>;
  /** resources.list(uri?: ResourceUri): Promise<ResourceEntry[]> */
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
  /** artifacts.put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef */
  put(data: ArtifactInput, opts?: ArtifactPutOptions): ArtifactRef;
  /** artifacts.get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent> */
  get(ref: ArtifactUri | ArtifactRef, opts?: ArtifactReadOptions): Promise<ResourceContent>;
  /** artifacts.slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent> */
  slice(ref: ArtifactUri | ArtifactRef, selector: ResourceSelector): Promise<ResourceContent>;
  /** artifacts.list(): ArtifactRef[] */
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
  /** fs.read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent> */
  read(path: SdkPath, opts?: FsReadOptions): Promise<ResourceContent>;
  /** fs.write(path: SdkPath, data: string, opts?: FsWriteOptions): Promise<FsWriteResult> */
  write(path: SdkPath, data: string, opts?: FsWriteOptions): Promise<FsWriteResult>;
  /** fs.ls(path?: SdkPath, opts?: FsListOptions): Promise<FsEntry[]> */
  ls(path?: SdkPath, opts?: FsListOptions): Promise<FsEntry[]>;
  /** fs.find(patterns: string | string[], opts?: FsFindOptions): Promise<FsEntry[]> */
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
  /** code.search(query: CodeSearchQuery): Promise<CodeSearchResult[]> */
  search(query: CodeSearchQuery): Promise<CodeSearchResult[]>;
  /** code.edit(patch: PatchEdit, opts?: CodeEditOptions): Promise<CodeEditResult> */
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
  /** proc.run(cmd: string, args?: string[], opts?: ProcRunOptions): Promise<ProcOutput> */
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
   * http.get(url: string): Promise<string>
   *
   * Experimental M1/P0 default-deny deterministic allowlist helper. This is
   * not ambient network egress, not fetch(), and not a production egress
   * policy. Non-allowlisted URLs fail closed with CapabilityDeniedError;
   * production egress hardening remains deferred.
   */
  get(url: string): Promise<string>;
}
