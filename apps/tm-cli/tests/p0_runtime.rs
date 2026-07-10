use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output, Stdio},
    thread,
};

use serde_json::json;
use tm_artifacts::ArtifactStore;

#[test]
fn cli_deno_host_ops_run_on_current_thread_runtime() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[workspace]\nmembers = []\n",
    )
    .unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let config = repo.path().join("config.json");
    std::fs::write(
        &config,
        json!({
            "linked_folders": [{
                "name": "repo",
                "path": repo.path(),
                "mode": "rw",
                "commands": [],
                "safe_args": []
            }],
            "approvals": { "mode": "deny", "timeout_ms": 1000 },
            "artifact_root": artifacts.path()
        })
        .to_string(),
    )
    .unwrap();

    let server = StubOpenAiServer::start();
    let output = run_tm(
        &config,
        "cli-runtime-test",
        "3",
        &server.base_url(),
        "Use execute to read linked://repo/Cargo.toml, then answer done.",
    );
    server.join();

    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("done"));
}

#[test]
fn cli_native_cutover_edits_runs_artifacts_and_denies_unsafe_proc() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"cutover_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("src/lib.rs"),
        "pub fn answer() -> &'static str {\n    \"old\"\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn answer_is_native() {\n        assert_eq!(answer(), \"native\");\n    }\n}\n",
    )
    .unwrap();

    let artifacts = tempfile::tempdir().unwrap();
    let config = repo.path().join("config.json");
    std::fs::write(
        &config,
        json!({
            "linked_folders": [{
                "name": "repo",
                "path": repo.path(),
                "mode": "rw",
                "commands": ["cargo"],
                "safe_args": [["cargo", "test"]]
            }],
            "approvals": { "mode": "deny", "timeout_ms": 1000 },
            "artifact_root": artifacts.path()
        })
        .to_string(),
    )
    .unwrap();

    let server = StubOpenAiServer::start_cutover();
    let output = run_tm(
        &config,
        "cli-cutover-test",
        "3",
        &server.base_url(),
        "Use execute to patch the linked repo, run cargo test, save a transcript artifact, and report native dogfood done.",
    );
    server.join();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code(),
    );
    assert_eq!(stdout.as_ref(), "native dogfood done\n");
    assert!(stderr.contains("tool call: execute"), "stderr={stderr}");
    assert!(stderr.contains("executing cell"), "stderr={stderr}");
    assert!(stderr.contains("result"), "stderr={stderr}");
    assert!(stderr.contains("artifact://0"), "stderr={stderr}");
    assert!(stderr.contains("ApprovalTimeoutError"), "stderr={stderr}");

    let edited = std::fs::read_to_string(repo.path().join("src/lib.rs")).unwrap();
    assert!(edited.contains("\"native\""), "{edited}");
    assert!(!edited.contains("\"old\""), "{edited}");

    let store = ArtifactStore::open(artifacts.path(), "cli-cutover-test").unwrap();
    let refs = store.list();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].uri, "artifact://0");
    assert_eq!(refs[0].title.as_deref(), Some("cli-cutover-transcript"));
    let transcript = store.read("artifact://0", None).unwrap();
    assert!(transcript.content.contains("ApprovalTimeoutError"));
    assert!(transcript.content.contains("\"testExit\": 0"));
    assert!(transcript.content.contains("\"changed\": true"));
}

fn run_tm(
    config: &std::path::Path,
    session_id: &str,
    max_turns: &str,
    base_url: &str,
    prompt: &str,
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tm"))
        .arg("--config")
        .arg(config)
        .arg("--session-id")
        .arg(session_id)
        .arg("--max-turns")
        .arg(max_turns)
        .env("OPENAI_BASE_URL", base_url)
        .env("OPENAI_API_KEY", "test-token")
        .env("OPENAI_MODEL", "stub-model")
        .env("OPENAI_STREAM_USAGE", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

struct StubOpenAiServer {
    addr: std::net::SocketAddr,
    handle: thread::JoinHandle<()>,
}

impl StubOpenAiServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_body = read_request_from(&mut first);
            assert!(first_body.contains("Use execute"));

            let code = r#"const doc = await fs.read("linked://repo/Cargo.toml"); display({ ok: doc.content.includes("[workspace]") });"#;
            let args = json!({ "code": code }).to_string();
            let first_chunk = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_cli_runtime",
                            "function": {
                                "name": "execute",
                                "arguments": args
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string();
            respond_sse(&mut first, &[first_chunk]);

            let (mut second, _) = listener.accept().unwrap();
            let second_body = read_request_from(&mut second);
            assert!(second_body.contains("call_cli_runtime"));
            assert!(second_body.contains("display"));
            let final_chunk = json!({
                "choices": [{
                    "delta": { "content": "done" },
                    "finish_reason": "stop"
                }]
            })
            .to_string();
            respond_sse(&mut second, &[final_chunk]);
        });
        Self { addr, handle }
    }

    fn start_cutover() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_body = read_request_from(&mut first);
            assert!(first_body.contains("patch the linked repo"));

            let code = r#"
const before = await fs.read("linked://repo/src/lib.rs");
const hits = await code.search({
  pattern: '"old"',
  paths: ["repo:src/lib.rs"],
  regex: false
});
const hit = hits[0];
const edit = await code.edit({
  path: hit.path,
  tag: hit.tag,
  hunks: [{
    op: "replace",
    startLine: hit.line,
    endLine: hit.line,
    lines: ['    "native"']
  }]
});
const after = await fs.read("linked://repo/src/lib.rs");
const tests = await proc.run("cargo", ["test"], {
  cwd: "repo:",
  outputBytes: 20000
});
const denied = await proc.run("cargo", ["clean"], { cwd: "repo:" })
  .then(() => ({ name: "unexpected success" }))
  .catch((err) => ({
    name: err.name,
    retryable: err.retryable,
    action: err.details?.action ?? null
  }));
const summary = {
  beforeHadOld: before.content.includes('"old"'),
  changed: after.content.includes('"native"'),
  editChanged: edit.changed,
  testExit: tests.exitCode,
  testStdoutHasOk: tests.stdout.includes("test result: ok"),
  denied
};
const artifact = artifacts.put(JSON.stringify(summary, null, 2), {
  title: "cli-cutover-transcript",
  mime: "application/json"
});
display({ ...summary, artifact: artifact.uri });
"#;
            let args = json!({ "code": code }).to_string();
            let first_chunk = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_cli_cutover",
                            "function": {
                                "name": "execute",
                                "arguments": args
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string();
            respond_sse(&mut first, &[first_chunk]);

            let (mut second, _) = listener.accept().unwrap();
            let second_body = read_request_from(&mut second);
            assert!(second_body.contains("call_cli_cutover"));
            assert!(second_body.contains("code.edit"));
            let tool_content = tool_message_content(&second_body);
            assert!(tool_content.contains("artifact://0"), "{tool_content}");
            assert!(
                tool_content.contains("ApprovalTimeoutError"),
                "{tool_content}"
            );
            assert!(tool_content.contains("\"testExit\":0"), "{tool_content}");
            assert!(
                tool_content.contains("\"testStdoutHasOk\":true"),
                "{tool_content}"
            );
            let final_chunk = json!({
                "choices": [{
                    "delta": { "content": "native dogfood done" },
                    "finish_reason": "stop"
                }]
            })
            .to_string();
            respond_sse(&mut second, &[final_chunk]);
        });
        Self { addr, handle }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn join(self) {
        self.handle.join().unwrap();
    }
}

fn tool_message_content(request: &str) -> String {
    let value = request_json(request);
    value["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .find(|message| message["role"].as_str() == Some("tool"))
        })
        .and_then(|message| message["content"].as_str())
        .unwrap_or_else(|| panic!("missing tool message in request: {value}"))
        .to_string()
}

fn request_json(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request contains header/body separator");
    serde_json::from_str(body).unwrap_or_else(|err| panic!("request body is json: {err}: {body}"))
}

fn respond_sse(stream: &mut TcpStream, chunks: &[String]) {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
}

fn read_request_from(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buf = [0; 1024];
    loop {
        let n = stream.read(&mut buf).unwrap();
        assert_ne!(n, 0, "connection closed before headers");
        bytes.extend_from_slice(&buf[..n]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let n = stream.read(&mut buf).unwrap();
        assert_ne!(n, 0, "connection closed before body");
        bytes.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&bytes).to_string()
}
