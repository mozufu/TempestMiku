pub(crate) const SDK_PRELUDE: &str = r#"
const __tm_ops = globalThis.Deno?.core?.ops;
if (!__tm_ops) throw new Error("HostCallError: Deno core ops unavailable");
try {
  Object.defineProperty(globalThis, "Deno", { value: undefined, writable: true, configurable: true });
} catch (_) {
  try { globalThis.Deno = undefined; } catch (_) {}
}
globalThis.fetch = undefined;
globalThis.__tm_stdout = [];
globalThis.__tm_displays = [];
globalThis.__tm_output_limit = 8192;
globalThis.__tm_output_size = 0;
globalThis.__tm_output_truncated = false;
globalThis.__tm_display_limit = 64;
globalThis.__tm_display_count = 0;
const __tm_utf8_size = (text) => {
  let bytes = 0;
  for (const char of String(text)) {
    const point = char.codePointAt(0);
    bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
  }
  return bytes;
};
const __tm_utf8_prefix = (text, maxBytes) => {
  const value = String(text);
  let bytes = 0;
  let units = 0;
  for (const char of value) {
    const point = char.codePointAt(0);
    const size = point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    if (bytes + size > maxBytes) break;
    bytes += size;
    units += char.length;
  }
  return { text: value.slice(0, units), bytes };
};
const __tm_render = (value) => {
  if (typeof value === "string") return value;
  try {
    const rendered = JSON.stringify(value);
    return rendered === undefined ? String(value) : rendered;
  } catch (_) {
    return String(value);
  }
};
const __tm_capture = (text) => {
  const rendered = String(text);
  const remaining = Math.max(0, globalThis.__tm_output_limit - globalThis.__tm_output_size);
  if (remaining === 0) {
    globalThis.__tm_output_truncated = true;
    return null;
  }
  const clipped = __tm_utf8_prefix(rendered, remaining);
  globalThis.__tm_output_size += clipped.bytes;
  if (clipped.bytes < __tm_utf8_size(rendered)) globalThis.__tm_output_truncated = true;
  return clipped.text;
};
const __tm_capture_or_spill = (text, title) => {
  const rendered = String(text);
  const captured = __tm_capture(rendered);
  if (captured != null && __tm_utf8_size(captured) === __tm_utf8_size(rendered)) return captured;
  try {
    const artifact = __tm_ops.op_tm_artifact_put(rendered, { title, mime: "text/plain" });
    return `${captured ?? ""}\n… output truncated; full output at ${artifact.uri}`.trim();
  } catch (_) {
    throw new Error("ResourceLimitError: output exceeded retention limit and artifact spill failed");
  }
};
globalThis.print = (...items) => {
  const captured = __tm_capture_or_spill(items.map(__tm_render).join(" "), "cell print");
  if (captured != null) globalThis.__tm_stdout.push(captured);
};
globalThis.display = (value, opts = undefined) => {
  globalThis.__tm_display_count += 1;
  if (globalThis.__tm_display_count > globalThis.__tm_display_limit) {
    throw new Error(`ResourceLimitError: cell exceeded ${globalThis.__tm_display_limit} displays`);
  }
  const rendered = __tm_render(value);
  if (opts && typeof opts === "object" && opts.artifact === true) {
    const artifact = __tm_ops.op_tm_artifact_put(rendered, opts);
    globalThis.__tm_displays.push({ artifact, opts });
    return;
  }
  const captured = __tm_capture_or_spill(rendered, opts?.title ?? "display");
  if (captured != null) globalThis.__tm_displays.push({ rendered: captured, opts });
};
const __tm_uri = (ref) => typeof ref === "string" ? ref : ref.uri;
const __tm_selector = (opts) => {
  const selector = opts && typeof opts === "object" ? opts.selector : undefined;
  return selector == null ? "" : String(selector);
};
const __tm_arg_selector = (selector) => selector == null ? "" : String(selector);
const __tm_sdk_shape = (value) => {
  if (!value || typeof value !== "object") return value;
  const shaped = { ...value };
  if (Object.prototype.hasOwnProperty.call(shaped, "size_bytes")) {
    shaped.sizeBytes = shaped.size_bytes;
    delete shaped.size_bytes;
  }
  if (Object.prototype.hasOwnProperty.call(shaped, "has_more")) {
    shaped.hasMore = shaped.has_more;
    delete shaped.has_more;
  }
  return shaped;
};
const __tm_sdk_error = (payload) => {
  const info = payload && typeof payload === "object" ? payload : {};
  const err = new Error(String(info.message ?? "host call failed"));
  err.name = String(info.name ?? "HostCallError");
  if (info.capability != null) err.capability = String(info.capability);
  if (info.path != null) err.path = String(info.path);
  if (info.uri != null) err.uri = String(info.uri);
  err.retryable = Boolean(info.retryable);
  err.details = info.details ?? null;
  return err;
};
const __tm_unwrap = (result) => {
  if (result && typeof result === "object" && result.ok === false) {
    throw __tm_sdk_error(result.error);
  }
  if (result && typeof result === "object" && result.ok === true) {
    return result.value;
  }
  return result;
};
const __tm_host_call = async (name, args) => __tm_unwrap(await __tm_ops.op_tm_host_call(name, args));
const __tm_resource_read = async (uri, selector) => __tm_unwrap(await __tm_ops.op_tm_resource_read(uri, selector));
const __tm_resource_preview = async (uri) => __tm_unwrap(await __tm_ops.op_tm_resource_preview(uri));
const __tm_resource_list = async (uri) => __tm_unwrap(await __tm_ops.op_tm_resource_list(uri));
globalThis.artifacts = {
  put: (data, opts = undefined) => __tm_sdk_shape(__tm_ops.op_tm_artifact_put(data, opts ?? null)),
  get: async (ref, opts = undefined) => __tm_sdk_shape(await __tm_resource_read(__tm_uri(ref), __tm_selector(opts))),
  slice: async (ref, selector) => artifacts.get(ref, { selector }),
  list: () => __tm_ops.op_tm_artifact_list().map(__tm_sdk_shape)
};
globalThis.resources = {
  read: async (uri, selector = undefined) => __tm_sdk_shape(await __tm_resource_read(String(uri), __tm_arg_selector(selector))),
  preview: async (uri) => __tm_sdk_shape(await __tm_resource_preview(String(uri))),
  list: async (uri = undefined) => (await __tm_resource_list(uri == null ? "" : String(uri))).map(__tm_sdk_shape)
};
globalThis.tools = {
  search: async (query, opts = undefined) => __tm_ops.op_tm_tools_search(String(query), opts ?? null),
  docs: async (name) => __tm_unwrap(__tm_ops.op_tm_tools_docs(String(name))),
  call: async (name, args = {}) => __tm_host_call(String(name), args ?? null)
};
globalThis.fs = {
  read: async (path, opts = undefined) => __tm_sdk_shape(await tools.call("fs.read", { path: String(path), ...(opts ?? {}) })),
  write: async (path, data, opts = undefined) => __tm_sdk_shape(await tools.call("fs.write", { path: String(path), data, ...(opts ?? {}) })),
  ls: async (path = undefined, opts = undefined) => await tools.call("fs.ls", { ...(path == null ? {} : { path: String(path) }), ...(opts ?? {}) }),
  find: async (patterns, opts = undefined) => await tools.call("fs.find", { patterns, ...(opts ?? {}) })
};
globalThis.code = {
  search: async (query) => await tools.call("code.search", query),
  edit: async (patch, opts = undefined) => await tools.call("code.edit", { ...patch, ...(opts ?? {}) })
};
globalThis.proc = {
  run: async (cmd, args = [], opts = undefined) => __tm_sdk_shape(await tools.call("proc.run", { cmd: String(cmd), args, ...(opts ?? {}) }))
};
globalThis.http = {
  get: async (url) => tools.call("http.get", { url: String(url) })
};
globalThis.modes = {
  suggest: async (targetMode, reason = undefined) =>
    await tools.call("modes.suggest", {
      targetMode: String(targetMode),
      ...(reason == null ? {} : { reason: String(reason) })
    })
};
globalThis.drive = {
  put: async (content, opts = undefined) =>
    __tm_sdk_shape(await tools.call("drive.put", { content, options: opts ?? {} })),
  get: async (pathOrUri, opts = undefined) =>
    __tm_sdk_shape(await tools.call(
      "drive.get",
      String(pathOrUri).startsWith("drive://")
        ? { uri: String(pathOrUri), ...(opts ?? {}) }
        : { path: String(pathOrUri), ...(opts ?? {}) }
    )),
  ls: async (pathOrQuery = undefined, opts = undefined) =>
    await tools.call("drive.ls", {
      ...(pathOrQuery == null ? {} : { path: String(pathOrQuery) }),
      ...(opts ?? {})
    }),
  move: async (from, to, opts = undefined) =>
    __tm_sdk_shape(await tools.call("drive.move", { from: String(from), to: String(to), ...(opts ?? {}) })),
  search: async (query = undefined, opts = undefined) =>
    await tools.call("drive.search", {
      ...(query == null ? {} : { query: String(query) }),
      ...(opts ?? {})
    }),
  tag: async (path, tags) =>
    __tm_sdk_shape(await tools.call("drive.tag", { path: String(path), tags: Array.from(tags ?? []) })),
  link: async (hostPath, mode = "ro", opts = undefined) =>
    __tm_sdk_shape(await tools.call("drive.link", { hostPath: String(hostPath), mode: String(mode), ...(opts ?? {}) })),
  unlink: async (aliasOrUri) =>
    __tm_sdk_shape(await tools.call("drive.unlink", { alias: String(aliasOrUri) })),
  organize: async (opts = undefined) =>
    await tools.call("drive.organize", opts ?? {})
};
const __tm_cap_text = (value, maxBytes) => {
  const text = String(value ?? "");
  return text.length <= maxBytes ? text : text.slice(0, maxBytes) + "...";
};
const __tm_bound_number = (value, fallback, min, max) => {
  const n = Number(value ?? fallback);
  const finite = Number.isFinite(n) ? n : fallback;
  return Math.max(min, Math.min(max, Math.floor(finite)));
};
const __tm_first_lines = (text, maxLines = 3) =>
  String(text ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(0, maxLines)
    .join(" ");
const __tm_error_text = (error) => {
  if (!error) return "unknown worker failure";
  if (typeof error === "string") return error;
  if (typeof error.message === "string") return error.message;
  try { return JSON.stringify(error); } catch (_) { return String(error); }
};
const __tm_research_failure_kind = (value) => {
  const text = __tm_error_text(value).toLowerCase();
  const status = value && typeof value === "object" && value.status != null
    ? String(value.status).toLowerCase()
    : "";
  if (status.includes("cancel") || text.includes("cancel")) return "cancelled";
  if (status.includes("timeout") || text.includes("timeout") || text.includes("timed out")) return "timeout";
  return "failed";
};
const __tm_research_actor_id = (value) => {
  if (value && typeof value === "object") {
    const actor = value.actorId ?? value.actor_id;
    if (actor != null) return String(actor);
  }
  const match = __tm_error_text(value).match(/\bactor\s+([A-Za-z][A-Za-z0-9_-]{0,63})\b/);
  return match ? match[1] : null;
};
const __tm_research_failure = (phase, value, index = null, doc = null) => ({
  phase,
  index,
  uri: doc?.uri ?? null,
  selector: doc?.selector ?? null,
  contentHash: doc?.contentHash ?? null,
  actorId: __tm_research_actor_id(value),
  kind: __tm_research_failure_kind(value),
  reason: __tm_cap_text(value && typeof value === "object" && value.reason != null ? value.reason : __tm_error_text(value), 300)
});
const __tm_drive_research = async (query = "", opts = undefined) => {
  const options = opts && typeof opts === "object" ? opts : {};
  const maxDocs = __tm_bound_number(options.maxDocs ?? options.limit, 5, 1, 10);
  const maxSnippets = __tm_bound_number(options.maxSnippets, maxDocs, 1, maxDocs);
  const maxBytesPerDoc = __tm_bound_number(options.maxBytesPerDoc, 2000, 1, 8000);
  const maxDigestBytes = __tm_bound_number(options.maxDigestBytes, 600, 32, 2000);
  const maxWorkers = __tm_bound_number(options.maxWorkers, maxDocs, 0, maxDocs);
  const requestedWorkerTimeoutMs = __tm_bound_number(options.workerTimeoutMs, 30000, 100, 120000);
  const totalTimeoutMs = __tm_bound_number(
    options.totalTimeoutMs,
    Math.max(requestedWorkerTimeoutMs, requestedWorkerTimeoutMs * Math.max(1, maxWorkers || maxDocs)),
    100,
    300000
  );
  const workerTimeoutMs = Math.min(requestedWorkerTimeoutMs, totalTimeoutMs);
  const startedAt = Date.now();
  const withinBudget = () => Date.now() - startedAt < totalTimeoutMs;
  const selector = options.selector == null ? undefined : String(options.selector);
  const hits = await drive.search(query == null ? undefined : String(query), {
    ...(options.project == null ? {} : { project: String(options.project) }),
    ...(options.docKind == null ? {} : { docKind: String(options.docKind) }),
    ...(Array.isArray(options.tags) ? { tags: options.tags.map(String) } : {}),
    limit: maxDocs,
    returnSnippets: true
  });
  const docs = [];
  for (const hit of hits.slice(0, Math.min(maxDocs, maxSnippets))) {
    if (!withinBudget()) break;
    const docSelector = selector ?? hit.selector ?? "1-20";
    const read = await resources.read(hit.uri, docSelector);
    const content = __tm_cap_text(read.content, maxBytesPerDoc);
    docs.push({
      uri: hit.uri,
      sourceKind: "drive",
      selector: docSelector,
      contentHash: hit.contentHash,
      title: hit.title ?? hit.path ?? hit.uri,
      snippet: hit.snippet ?? __tm_first_lines(content),
      sizeBytes: read.sizeBytes ?? 0,
      content
    });
  }
  const localDigests = docs.map((doc) => ({
    uri: doc.uri,
    selector: doc.selector,
    contentHash: doc.contentHash,
    summary: __tm_cap_text(__tm_first_lines(doc.content) || doc.snippet || doc.title, maxDigestBytes),
    citations: [{ uri: doc.uri, sourceKind: "drive", selector: doc.selector, contentHash: doc.contentHash }]
  }));
  const useAgents = options.useAgents !== false
    && maxWorkers > 0
    && globalThis.agents
    && typeof globalThis.agents.parallel === "function"
    && docs.length > 0
    && withinBudget();
  const agentDocs = docs.slice(0, maxWorkers);
  const workerFailures = [];
  let workerResults = localDigests;
  let agentDocsCompleted = 0;
  if (useAgents) {
    try {
      const rawWorkerResults = await globalThis.agents.parallel(agentDocs.map((doc) => ({
        role: String(options.role ?? "researcher"),
        timeoutMs: workerTimeoutMs,
        budget: { wallMs: workerTimeoutMs },
        task: [
          "Summarize this local drive document for the parent research workspace.",
          `Cite only ${doc.uri} selector ${doc.selector} hash ${doc.contentHash}.`,
          `Return a bounded digest within ${maxDigestBytes} bytes and ${workerTimeoutMs}ms; do not request network access.`,
          "",
          doc.content
        ].join("\n")
      })));
      workerResults = docs.map((doc, index) => {
        const worker = index < agentDocs.length ? rawWorkerResults[index] : localDigests[index];
        const status = worker && typeof worker === "object" && worker.status != null
          ? String(worker.status).toLowerCase()
          : "completed";
        if (status === "failed" || status === "cancelled" || status === "canceled" || status === "timeout") {
          workerFailures.push(__tm_research_failure("worker", worker, index, doc));
          return localDigests[index];
        }
        if (index < agentDocs.length && worker != null) agentDocsCompleted += 1;
        return worker ?? localDigests[index];
      });
    } catch (error) {
      workerFailures.push(__tm_research_failure("agents.parallel", error, null, null));
      workerResults = localDigests;
    }
  }
  const digests = docs.map((doc, index) => {
    const worker = workerResults[index] ?? localDigests[index];
    const summary = __tm_cap_text(worker.summary ?? worker.text ?? localDigests[index].summary, maxDigestBytes);
    return {
      uri: doc.uri,
      selector: doc.selector,
      contentHash: doc.contentHash,
      summary,
      actorId: worker.actorId ?? worker.actor_id ?? null,
      artifactUri: worker.artifactUri ?? worker.artifact_uri ?? null,
      historyUri: worker.historyUri ?? worker.history_uri ?? null,
      citations: [{ uri: doc.uri, sourceKind: "drive", selector: doc.selector, contentHash: doc.contentHash }]
    };
  });
  return {
    query: String(query ?? ""),
    corpus: docs.map(({ content, ...doc }) => doc),
    digests,
    citations: digests.flatMap((digest) => digest.citations),
    workerFailures,
    answer: digests.map((digest) => `[${digest.uri}#${digest.selector}] ${digest.summary}`).join("\n"),
    budget: {
      maxDocs,
      maxSnippets,
      maxBytesPerDoc,
      maxDigestBytes,
      maxWorkers,
      workerTimeoutMs,
      totalTimeoutMs,
      selectedDocs: docs.length,
      agentDocs: useAgents ? agentDocs.length : 0,
      agentDocsCompleted,
      workerFailures: workerFailures.length
    }
  };
};
globalThis.research = {
  drive: __tm_drive_research
};
globalThis.secrets = undefined;
globalThis.memory = undefined;
globalThis.skills = undefined;
globalThis.agents = undefined;
"#;

/// Injected after [`SDK_PRELUDE`] when the session holds at least one `agents.*` grant.
///
/// Replaces the `undefined` placeholder with a real namespace that forwards to HostFns.
/// Grant enforcement is still server-side; this only surfaces the API surface to LLM code.
pub(crate) const AGENTS_PRELUDE: &str = r#"
const __tm_actor_id = (ref) => typeof ref === "string" ? ref : ref?.id;
const __tm_wait_args = (from = undefined, timeoutMs = undefined) => {
  if (from && typeof from === "object" && !Object.prototype.hasOwnProperty.call(from, "id")) {
    return from;
  }
  return {
    ...(from == null ? {} : { from: __tm_actor_id(from) }),
    ...(timeoutMs == null ? {} : { timeoutMs: Number(timeoutMs) }),
  };
};
const __tm_pipeline_stage = async (items, stage, stageIndex) => {
  if (!stage || typeof stage !== "object" || stage.role == null) {
    throw new TypeError("agents.pipeline stage.role is required");
  }
  const role = String(stage.role);
  if (typeof stage.task === "function") {
    const tasks = [];
    for (let index = 0; index < items.length; index++) {
      tasks.push(String(await stage.task(items[index], index, stageIndex)));
    }
    return { role, tasks };
  }
  if (Array.isArray(stage.tasks)) {
    if (stage.tasks.length !== items.length) {
      throw new TypeError("agents.pipeline stage.tasks length must match current item count");
    }
    return { role, tasks: stage.tasks.map((task) => String(task)) };
  }
  if (stage.task != null) return { role, task: String(stage.task) };
  throw new TypeError("agents.pipeline stage.task or stage.tasks is required");
};
globalThis.agents = {
  run: async (role, task, opts = undefined) =>
    __tm_host_call("agents.run", { role: String(role), task: String(task), ...(opts != null ? { opts } : {}) }),
  spawn: async (role, task, opts = undefined) =>
    __tm_host_call("agents.spawn", { role: String(role), task: String(task), ...(opts != null ? { opts } : {}) }),
  parallel: async (tasks) =>
    __tm_host_call("agents.parallel", { tasks }),
  pipeline: async (items, ...stages) => {
    let current = Array.from(items ?? []);
    const waves = [];
    for (let stageIndex = 0; stageIndex < stages.length; stageIndex++) {
      const stage = await __tm_pipeline_stage(current, stages[stageIndex], stageIndex);
      const result = await __tm_host_call("agents.pipeline", { items: current, stages: [stage] });
      current = result[0] ?? [];
      waves.push(current);
    }
    if (stages.length === 0) {
      await __tm_host_call("agents.pipeline", { items: current, stages: [] });
    }
    return waves;
  },
  msg: async (handle, text, opts = undefined) =>
    __tm_host_call("agents.msg", { handle, text: String(text), ...(opts != null ? { opts } : {}) }),
  send: async (to, text, opts = undefined) =>
    __tm_host_call("agents.send", { to: __tm_actor_id(to), text: String(text), ...(opts != null ? { opts } : {}) }),
  broadcast: async (text) =>
    __tm_host_call("agents.broadcast", { text: String(text) }),
  cancel: async (target) =>
    __tm_host_call("agents.cancel", { target: __tm_actor_id(target) }),
  wait: async (from = undefined, timeoutMs = undefined) =>
    __tm_host_call("agents.wait", __tm_wait_args(from, timeoutMs)),
  inbox: async () =>
    __tm_host_call("agents.inbox", {}),
  list: async () =>
    __tm_host_call("agents.list", {}),
};
"#;
