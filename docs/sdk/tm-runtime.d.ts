/**
 * TempestMiku JS/TS runtime prelude.
 *
 * P0/P2 surface: no ambient filesystem, process, network, secret, shell, or
 * host access. Every external effect goes through capability-checked SDK
 * namespaces. P2 memory is exposed as memory:// resources behind
 * resources.read:memory, not as a memory.* namespace. P4 adds constrained
 * memory:// dream queues/records, summaries, and skill-proposal previews;
 * bundled skill markdown may still be labeled skill://... inside composed
 * prompts, but that label is not a resources.read/list/preview surface until
 * the P7 import/reload lifecycle registers a handler and grants.
 *
 * P3/P3-plus agents surface: `agents` is defined only in sessions holding the required
 * agents.* grant. In ungranted sessions it remains `undefined`. Use
 * `tools.search('agents')` to check availability before calling. Messages
 * between actors are always plain prose — never control-payload blobs.
 * Large payloads pass by reference (artifact://, memory://), never inline.
 * P3 shipped run/spawn/parallel/msg; P3-plus adds live inbox delivery through
 * send/broadcast/wait/inbox/list plus parent-driven actor cancellation.
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
  const drive: DriveNamespace;
  const research: ResearchNamespace;

  var secrets: undefined;
  var memory: undefined;
  var skills: undefined;

  /**
   * P3 — defined in sessions holding the required agents.* grant; `undefined`
   * in all other sessions. Use `tools.search('agents')` to check availability.
   * Protocol invariants (all messaging from day one):
   * - Messages are plain prose — never control-payload blobs (`{"type":"done"}` is banned).
   * - One ask per message; lead with the answer when replying.
   * - Large payloads pass by reference (artifact://, memory://), never inline.
   * - A failed receipt means unreachable or backpressured — do not retry-loop.
   * - The agent DAG must be acyclic; a real actor never waits on itself or its
   *   own descendant. Synthetic Root may await root-level workers.
   */
  const agents: AgentsNamespace | undefined;
}

type JsonPrimitive = string | number | boolean | null;
type JsonValue = JsonPrimitive | JsonObject | JsonArray;
interface JsonObject { [key: string]: JsonValue; }
interface JsonArray extends Array<JsonValue> {}

type MimeType = string;
type CapabilityName = string;
type ArtifactUri = `artifact://${string}`;
type BlobUri = `blob:sha256:${string}`;
type SkillPromptLabel = `skill://${string}`;

type MemoryResourceUri =
  | "memory://root"
  | "memory://user-model"
  | "memory://dreams"
  | `memory://dreams/${string}`
  | `memory://profile/${string}/facts/${string}`
  | `memory://scopes/${string}/chunks/${string}`
  | `memory://summaries/${string}`
  | `memory://skill-proposals/${string}`;

type ProjectResourceUri = `project://${string}`;
type CronResourceUri =
  | "cron://"
  | "cron://root"
  | `cron://${string}`
  | `cron://${string}/runs`
  | `cron://${string}/runs/${string}`;

type ResourceUri =
  | `artifact://${string}`
  | `agent://${string}`
  | `history://${string}`
  | MemoryResourceUri
  | `drive://${string}`
  | CronResourceUri
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
    | "artifact"
    | "agents"
    | "drive";
  description: string;
}

interface ResourcesNamespace {
  /**
   * resources.read(uri: ResourceUri, selector?: ResourceSelector): Promise<ResourceContent>
   *
   * Scheme-dispatched resource read. Current registered schemes include
   * artifact://, linked://, workspace://session, project://, the P2/P4
   * memory:// surface, the P3 agent:// / history:// handlers, P4 cron://
   * job/run previews, and P5 drive:// documents when configured. Each scheme has its own grant such as
   * resources.read:artifact, resources.read:linked, resources.read:memory,
   * resources.read:agent, resources.read:history, resources.read:cron,
   * or resources.read:drive;
   * missing grants and unknown schemes fail closed.
   * skill://... is prompt-composition-only for now. Reads for unregistered
   * schemes must fail closed until their owning milestones wire handlers and grants.
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
  | "drive_document"
  | "drive_binary"
  | "actor"
  | "history"
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

// ─── P5 Drive ────────────────────────────────────────────────────────────────

interface DriveNamespace {
  /** drive.put(content: DriveContent, opts?: DrivePutOptions): Promise<DrivePutResult> */
  put(content: DriveContent, opts?: DrivePutOptions): Promise<DrivePutResult>;
  /** drive.get(pathOrUri: string, opts?: { selector?: ResourceSelector }): Promise<ResourceContent> */
  get(pathOrUri: string, opts?: { selector?: ResourceSelector }): Promise<ResourceContent>;
  /** drive.ls(pathOrQuery?: string, opts?: DriveListOptions): Promise<DriveEntry[]> */
  ls(pathOrQuery?: string, opts?: DriveListOptions): Promise<DriveEntry[]>;
  /** drive.move(from: string, to: string, opts?: DriveMoveOptions): Promise<DriveEntry> */
  move(from: string, to: string, opts?: DriveMoveOptions): Promise<DriveEntry>;
  /** drive.search(query?: string, opts?: DriveSearchOptions): Promise<DriveSearchResult[]> */
  search(query?: string, opts?: DriveSearchOptions): Promise<DriveSearchResult[]>;
  /** drive.tag(path: string, tags: string[]): Promise<DriveEntry> */
  tag(path: string, tags: string[]): Promise<DriveEntry>;
  /** drive.link(hostPath: string, mode?: 'ro' | 'rw', opts?: { project?: string }): Promise<DriveLinkPlan> */
  link(hostPath: string, mode?: 'ro' | 'rw', opts?: { project?: string }): Promise<DriveLinkPlan>;
  /** drive.unlink(aliasOrUri: string): Promise<DriveUnlinkResult> */
  unlink(aliasOrUri: string): Promise<DriveUnlinkResult>;
  /** drive.organize(opts?: DriveOrganizeOptions): Promise<OrganizerProposal[]> */
  organize(opts?: DriveOrganizeOptions): Promise<OrganizerProposal[]>;
}

type DriveJsonContent = JsonPrimitive | JsonArray | ({ [key: string]: JsonValue } & { uri?: never });
type DriveContent = string | DriveJsonContent | { uri: BlobUri } | { text: string };
type DriveUri = `drive://${string}`;
type DriveEntryStatus = "active" | "archived" | "deleted";
type DriveCollisionStrategy = "keep-both" | "reject" | "overwrite";
/** "propose" is the conservative default; trusted host policy owns auto filing. */
type DriveApprovalMode = "propose" | "requireApproval";
type DriveDedupeMode = "contentHash" | "off";
type DriveAutomationTier = "conservative";

interface DriveModelExtractionOptions {
  /** Disabled by default; when enabled, emits a redacted bounded request for a configured model role. */
  enabled?: boolean;
  /** Defaults to "document_extractor". */
  role?: string;
  /** Defaults to docKind/entities/dates/amounts/summary/embedding. */
  fields?: string[];
  /** Bounded by the host; defaults to 2000 bytes. */
  maxPreviewBytes?: number;
}

interface DrivePutOptions {
  /** When true, the host policy asks before committing at the sandbox boundary. */
  auto?: boolean;
  suggestedPath?: string;
  project?: string;
  docKind?: string;
  tags?: string[];
  sourceUri?: string;
  mime?: MimeType;
  title?: string;
  /** "requireApproval" always asks; auto filing is trusted host policy, not a sandbox option. */
  approvalMode?: DriveApprovalMode;
  dedupe?: DriveDedupeMode;
  collision?: DriveCollisionStrategy;
  overwrite?: boolean;
  conventions?: DriveConventions;
  modelExtraction?: DriveModelExtractionOptions;
}

interface DriveConventions {
  /** Template tokens: {project}, {docKind}, {title}, {filename}. */
  project?: string;
  /** Template tokens: {year}, {docKind}, {title}, {filename}. */
  finance?: string;
  /** Template tokens: {date}, {docKind}, {title}, {filename}. */
  inbox?: string;
}

interface DriveListOptions {
  recursive?: boolean;
  limit?: number;
  includeArchived?: boolean;
}

interface DriveMoveOptions {
  collision?: DriveCollisionStrategy;
  overwrite?: boolean;
}

interface DriveSearchOptions {
  project?: string;
  docKind?: string;
  tags?: string[];
  limit?: number;
  includeArchived?: boolean;
  since?: string;
  until?: string;
  returnSnippets?: boolean;
}

interface DriveOrganizeOptions {
  /** Apply all pending proposals after approval; omitted means propose or config-gated auto-apply only. */
  apply?: boolean;
  /** Conservative by default; only configured higher-tier rules may auto-apply low-risk proposals. */
  config?: DriveOrganizerConfig;
}

interface DriveOrganizerConfig {
  /** The sandbox host API accepts conservative proposal generation only. */
  tier?: DriveAutomationTier;
  /** Auto-apply rules are trusted server/background policy only. */
  autoApply?: never;
}

interface DriveOrganizerAutoApplyRule {
  /** Explicit actions are required; empty means no proposal matches. */
  actions?: OrganizerActionKind[];
  docKinds?: string[];
  projects?: string[];
  /** Defaults to 0.8. */
  minConfidence?: number;
}

interface DriveAmount {
  raw: string;
  value?: number;
  currency?: string;
}

interface DriveEvidence {
  snippet: string;
  selector?: ResourceSelector;
}

interface DriveAttribute {
  key: string;
  value: string;
  confidence: number;
  evidence?: DriveEvidence;
  extractor: string;
  sourceUri?: ResourceUri;
  sessionId?: string;
  eventSeq?: number;
  contentHash?: string;
}

interface DriveProvenance {
  sourceUri?: string;
  sessionId?: string;
  eventSeq?: number;
  actorId?: string;
  sourceRunId?: string;
  contentHash: string;
  extractor: string;
  createdAt: string;
}

interface DriveEntry {
  id: string;
  path: string;
  uri: DriveUri;
  blobUri: string;
  contentHash: string;
  mime: MimeType;
  sizeBytes: number;
  title?: string;
  docKind?: string;
  project?: string;
  entities: string[];
  dates: string[];
  amounts: DriveAmount[];
  tags: string[];
  embedding?: string;
  sourceUri?: string;
  provenance: DriveProvenance[];
  createdAt: string;
  updatedAt: string;
  status: DriveEntryStatus;
  attributes: DriveAttribute[];
  summary?: string;
}

interface DrivePutResult {
  entry: DriveEntry;
  uri: DriveUri;
  proposedPath: string;
  filed: boolean;
  proposal?: OrganizerProposal;
}

interface DriveSearchResult {
  uri: DriveUri;
  path: string;
  title?: string;
  docKind?: string;
  project?: string;
  tags: string[];
  contentHash: string;
  score: number;
  snippet?: string;
  selector?: ResourceSelector;
}

interface DriveLinkPlan {
  alias: string;
  canonicalRoot: string;
  mode: "ro" | "rw";
  linkedUri: `linked://${string}/`;
  memoryScope: string;
  project: string;
  requiresApproval: boolean;
}

interface DriveUnlinkResult {
  alias: string;
  canonicalRoot: string;
  linkedUri: `linked://${string}/`;
  memoryScope: string;
  revokedAt: string;
}

type OrganizerActionKind = "move" | "tag" | "dedupe" | "archive" | "set_doc_kind" | "set_project";
type ProposalStatus = "pending" | "approved" | "denied" | "applied" | "stale" | "failed";
type PolicyDecision = "auto_apply" | "approval_required" | "denied" | "noop";

interface OrganizerProposal {
  id: string;
  action: OrganizerActionKind;
  entryId: string;
  sourcePath: string;
  proposedPath?: string;
  proposedTags: string[];
  proposedDocKind?: string;
  proposedProject?: string;
  evidence: DriveEvidence[];
  confidence: number;
  policyDecision: PolicyDecision;
  approvalId?: string;
  status: ProposalStatus;
  sourceRunId: string;
  replayMetadata: JsonObject;
  createdAt: string;
  updatedAt: string;
}

// ─── P5 Local Research Helpers ──────────────────────────────────────────────

interface ResearchNamespace {
  /**
   * Compose drive.search + paged resources.read + optional agents.parallel.
   * Returns bounded digests and resource refs; full document content is not
   * included in the returned corpus.
   */
  drive(query?: string, opts?: DriveResearchOptions): Promise<DriveResearchResult>;
}

interface DriveResearchOptions {
  project?: string;
  docKind?: string;
  tags?: string[];
  maxDocs?: number;
  limit?: number;
  maxSnippets?: number;
  maxBytesPerDoc?: number;
  maxDigestBytes?: number;
  maxWorkers?: number;
  workerTimeoutMs?: number;
  totalTimeoutMs?: number;
  selector?: ResourceSelector;
  useAgents?: boolean;
  role?: string;
}

interface DriveResearchCorpusRef {
  uri: DriveUri;
  sourceKind: "drive" | "external";
  selector: ResourceSelector;
  contentHash: string;
  title: string;
  snippet: string;
  sizeBytes: number;
}

interface DriveResearchCitation {
  uri: DriveUri;
  sourceKind: "drive" | "external";
  selector: ResourceSelector;
  contentHash: string;
}

interface DriveResearchDigest {
  uri: DriveUri;
  selector: ResourceSelector;
  contentHash: string;
  summary: string;
  actorId: string | null;
  artifactUri: ArtifactUri | null;
  historyUri: string | null;
  citations: DriveResearchCitation[];
}

interface DriveResearchFailure {
  phase: "agents.parallel" | "worker";
  index: number | null;
  uri: DriveUri | null;
  selector: ResourceSelector | null;
  contentHash: string | null;
  actorId: string | null;
  kind: "failed" | "cancelled" | "timeout";
  reason: string;
}

interface DriveResearchResult {
  query: string;
  corpus: DriveResearchCorpusRef[];
  digests: DriveResearchDigest[];
  citations: DriveResearchCitation[];
  workerFailures: DriveResearchFailure[];
  answer: string;
  budget: DriveResearchBudget;
}

interface DriveResearchBudget {
  maxDocs: number;
  maxSnippets: number;
  maxBytesPerDoc: number;
  maxDigestBytes: number;
  maxWorkers: number;
  workerTimeoutMs: number;
  totalTimeoutMs: number;
  selectedDocs: number;
  agentDocs: number;
  agentDocsCompleted: number;
  workerFailures: number;
}

// ─── P3 Agents ───────────────────────────────────────────────────────────────

/**
 * Capability-gated sub-agent orchestration namespace (§23, P3).
 *
 * Available only when the session holds the required agents.* grant.
 * `globalThis.agents` is `undefined` in ungranted sessions — check before calling.
 *
 * P3 surface: run, spawn, parallel, msg.
 * P3-plus foundation: live per-actor inbox delivery through send, broadcast,
 * wait, inbox, list, cancel, and pipeline.
 */
interface AgentsNamespace {
  /**
   * agents.run(role: string, task: string, opts?: AgentRunOpts): Promise<AgentDigest>
   *
   * Spawn one child actor, run it to completion, and return a bounded digest.
   * Full output spills to artifact://; read-only transcript is at history://<id>.
   * The agent DAG must be acyclic. Requires agents.run grant.
   */
  run(role: string, task: string, opts?: AgentRunOpts): Promise<AgentDigest>;

  /**
   * agents.spawn(role: string, task: string, opts?: AgentSpawnOpts): Promise<AgentHandle>
   *
   * Non-blocking spawn; returns a handle for later coordination via agents.msg.
   * Requires agents.spawn grant. The actor runs in the background and is
   * tracked through the agent:// roster.
   */
  spawn(role: string, task: string, opts?: AgentSpawnOpts): Promise<AgentHandle>;

  /**
   * agents.parallel(tasks: AgentTask[]): Promise<AgentDigest[]>
   *
   * One-wave fan-out: spawns N actors concurrently (bounded pool), awaits all,
   * and returns ordered digest results. Only digests return to the parent context.
   * Requires agents.parallel grant.
   */
  parallel(tasks: AgentTask[]): Promise<AgentDigest[]>;

  /**
   * agents.pipeline(items: JsonValue[], ...stages: AgentPipelineStage[]): Promise<AgentDigest[][]>
   *
   * Run a staged map pipeline. Each stage fans out one actor per current item,
   * waits for the full wave to finish, then feeds compact digest references
   * into the next stage: actor/resource handles plus a bounded summary, never
   * the upstream transcript. Returns one ordered digest array per stage.
   * Requires agents.pipeline grant.
   */
  pipeline(items: JsonValue[], ...stages: AgentPipelineStage[]): Promise<AgentDigest[][]>;

  /**
   * agents.msg(handle: AgentHandle, text: string, opts?: MsgOpts): Promise<AgentReceipt | string | null>
   *
   * Send a plain-prose message to a spawned actor.
   *
   * Fire-and-forget (default): delivers to the actor's bounded live inbox and
   * returns a delivered/failed receipt.
   *
   * Request/reply (opts.await = true): for running actors, delivers to the live
   * inbox and waits for the actor to reply to the caller. If live delivery
   * fails, returns a failed receipt instead of waiting. For already completed
   * actors, this remains a compatibility one-shot seeded from the target actor's
   * last digest summary + the new text.
   *
   * A failed receipt means the actor is unreachable or backpressured — do not retry-loop (§23.9).
   * Request/reply from a real actor to itself or its own descendant is rejected
   * to keep the DAG acyclic. Messages must be plain prose — never
   * control-payload blobs. Pass large payloads by reference (artifact://,
   * memory://). Requires agents.msg grant.
   */
  msg(handle: AgentHandle, text: string, opts?: MsgOpts): Promise<AgentReceipt | string | null>;

  /**
   * agents.send(to: AgentHandle | string, text: string, opts?: SendOpts): Promise<AgentReceipt | AgentMessage | null>
   *
   * Deliver a plain-prose message to one live actor inbox. Fire-and-forget
   * returns a delivered/failed receipt. With opts.await = true, waits for a
   * matching reply message in the caller inbox and returns it, returns a failed
   * receipt if live delivery fails, or null on timeout.
   * Awaiting a real actor's own descendant is rejected to keep the DAG acyclic.
   * Requires agents.send grant.
   */
  send(
    to: AgentHandle | string,
    text: string,
    opts?: SendOpts,
  ): Promise<AgentReceipt | AgentMessage | null>;

  /**
   * agents.broadcast(text: string): Promise<AgentBroadcastReceipt[]>
   *
   * Deliver a plain-prose message to the caller's direct live children. Top-level
   * orchestrator code uses the synthetic Root actor and targets root-level live
   * children. Broadcast is fire-and-forget only; no replies are awaited.
   * Requires agents.broadcast grant.
   */
  broadcast(text: string): Promise<AgentBroadcastReceipt[]>;

  /**
   * agents.cancel(target: AgentHandle | string): Promise<AgentCancelReceipt>
   *
   * Request cancellation for a direct child actor. The actor record becomes
   * terminal immediately, its cancellation token is tripped, and one replayable
   * actor_cancelled session event is emitted. Only the direct parent, or the
   * synthetic top-level Root for root-level children, may cancel an actor.
   * Requires agents.cancel grant.
   */
  cancel(target: AgentHandle | string): Promise<AgentCancelReceipt>;

  /**
   * agents.wait(from?: AgentHandle | string, timeoutMs?: number): Promise<AgentMessage | null>
   *
   * Block until the current actor inbox receives a matching message. Top-level
   * orchestrator code uses the synthetic Root inbox. Returns null on timeout.
   * A real actor cannot target a wait at itself or its own descendant.
   * Requires agents.wait grant.
   */
  wait(from?: AgentHandle | string, timeoutMs?: number): Promise<AgentMessage | null>;

  /**
   * agents.inbox(): Promise<AgentMessage[]>
   *
   * Drain all pending messages from the current actor inbox without blocking.
   * Requires agents.inbox grant.
   */
  inbox(): Promise<AgentMessage[]>;

  /**
   * agents.list(): Promise<AgentRosterEntry[]>
   *
   * Return the actor roster with status, unread inbox count, last activity, and
   * resource links. Requires agents.list grant.
   */
  list(): Promise<AgentRosterEntry[]>;
}

/** Bounded digest returned to the parent context from a completed actor (§23.5). */
interface AgentDigest {
  /** Stable CamelCase actor ID (≤32 chars). */
  actorId: string;
  /** Plain-prose summary — the only part injected into parent context. */
  summary: string;
  /** URI of the full output artifact, when output exceeded the digest threshold. */
  artifactUri: string | null;
  /** URI of the read-only transcript for this actor. */
  historyUri: string | null;
}

/** Opaque handle returned by agents.spawn for coordination via agents.msg (§23.3). */
interface AgentHandle {
  /** Stable CamelCase actor ID matching the agent:// registry entry. */
  id: string;
}

/** Task descriptor passed to agents.parallel. */
interface AgentTask {
  /** Mode/role for the child actor. */
  role: string;
  /** Plain-prose task description. */
  task: string;
  /** Optional wall-clock timeout for this child actor in milliseconds. */
  timeoutMs?: number;
  /** Optional explicit child actor budget. */
  budget?: AgentTaskBudget;
}

interface AgentTaskBudget {
  /** Wall-clock limit for this child actor in milliseconds. */
  wallMs?: number;
  /** Maximum spawn depth allowed from this child actor. */
  maxDepth?: number;
}

/** Stage descriptor passed to agents.pipeline. */
type AgentPipelineTaskFn = (
  item: JsonValue | AgentDigest,
  index: number,
  stageIndex: number,
) => string | Promise<string>;

interface AgentPipelineStage {
  /** Mode/role for this stage's actors. */
  role: string;
  /** Task prompt applied to each current input, or a task builder function. */
  task?: string | AgentPipelineTaskFn;
  /** Per-input task prompts for the current wave. */
  tasks?: string[];
}

/** Plain-prose message delivered through a bounded actor inbox. */
interface AgentMessage {
  from: string;
  to: string;
  text: string;
  replyTo: string | null;
  sentAt: string;
}

/** Delivery receipt for fire-and-forget sends. */
interface AgentReceipt {
  status: "delivered" | "failed";
  /** Present when status is "failed". */
  reason?: "unreachable" | "backpressured";
}

/** Per-target receipt returned by agents.broadcast(). */
interface AgentBroadcastReceipt {
  actorId: string;
  status: "delivered" | "failed";
  /** Present when status is "failed". */
  reason?: "unreachable" | "backpressured";
}

/** Receipt returned by agents.cancel(). */
interface AgentCancelReceipt {
  actorId: string;
  status: "cancelled" | "already_cancelled" | "already_terminated" | "not_found";
}

/** Roster row returned by agents.list(). */
interface AgentRosterEntry {
  id: string;
  parentId: string | null;
  status: "running" | "idle" | "parked" | "terminated";
  mode: string | null;
  unread: number;
  lastActivity: string | null;
  artifactUri: string | null;
  historyUri: string | null;
}

/** Optional options for agents.run (reserved; fields added in P3.2). */
interface AgentRunOpts {
  [key: string]: unknown;
}

/** Optional options for agents.spawn. */
interface AgentSpawnOpts {
  /**
   * Group-scoped supervision for sibling spawned actors. Actors using the same
   * group share a supervisor; by default, a failing group member cancels the
   * sibling group without restart.
   */
  supervision?: AgentSupervisionOpts;
}

interface AgentSupervisionOpts {
  /** Stable caller-chosen sibling group label. */
  group?: string;
  /** Restart/cancel strategy for this supervisor group. */
  strategy?: "one_for_one" | "one_for_all" | "rest_for_one";
  /** Maximum restart attempts before escalation. Group default is 0. */
  maxRestarts?: number;
}

/** Options for agents.msg. */
interface MsgOpts {
  /** If true, block for the actor's reply (request/reply). Default: fire-and-forget. */
  await?: boolean;
  /** Milliseconds to wait for live request/reply. Default: 30000. */
  timeoutMs?: number;
}

/** Options for agents.send. */
interface SendOpts {
  /** If true, wait for a reply message to the caller inbox. */
  await?: boolean;
  /** Milliseconds to wait for a reply. Default: 30000. */
  timeoutMs?: number;
}
