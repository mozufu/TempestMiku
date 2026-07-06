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
globalThis.print = (...items) => {
  globalThis.__tm_stdout.push(items.map((item) =>
    typeof item === "string" ? item : JSON.stringify(item)
  ).join(" "));
};
globalThis.display = (value, opts = undefined) => {
  globalThis.__tm_displays.push({ value, opts });
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
globalThis.agents = {
  run: async (role, task, opts = undefined) =>
    __tm_host_call("agents.run", { role: String(role), task: String(task), ...(opts != null ? { opts } : {}) }),
  spawn: async (role, task) =>
    __tm_host_call("agents.spawn", { role: String(role), task: String(task) }),
  parallel: async (tasks) =>
    __tm_host_call("agents.parallel", { tasks }),
  msg: async (handle, text, opts = undefined) =>
    __tm_host_call("agents.msg", { handle, text: String(text), ...(opts != null ? { opts } : {}) }),
  send: async (to, text, opts = undefined) =>
    __tm_host_call("agents.send", { to: __tm_actor_id(to), text: String(text), ...(opts != null ? { opts } : {}) }),
  broadcast: async (text) =>
    __tm_host_call("agents.broadcast", { text: String(text) }),
  wait: async (from = undefined, timeoutMs = undefined) =>
    __tm_host_call("agents.wait", __tm_wait_args(from, timeoutMs)),
  inbox: async () =>
    __tm_host_call("agents.inbox", {}),
  list: async () =>
    __tm_host_call("agents.list", {}),
};
"#;
