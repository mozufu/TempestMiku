use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output, Stdio},
    thread,
};

use serde_json::json;
use tm_artifacts::ArtifactStore;

#[test]
fn cli_tm_host_ops_run_on_current_thread_runtime() {
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
    let event_log = repo.path().join("events.jsonl");
    let output = run_tm_with_event_log(
        &config,
        "cli-runtime-test",
        "3",
        &server.base_url(),
        "Use execute to read linked://repo/Cargo.toml, then answer done.",
        Some(&event_log),
        false,
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
    let events = read_events(&event_log);
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "tool_call",
            "turn_end",
            "cell_start",
            "cell_result",
            "text_delta",
            "turn_end",
            "final",
        ]
    );
    assert!(events[2]["code"].as_str().unwrap().contains("@fs.read"));
    assert_eq!(events.last().unwrap()["text"], "done");
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
    assert!(transcript.content.contains("\"testExit\":0"));
    assert!(transcript.content.contains("\"changed\":true"));
}

fn run_tm(
    config: &std::path::Path,
    session_id: &str,
    max_turns: &str,
    base_url: &str,
    prompt: &str,
) -> Output {
    run_tm_with_event_log(config, session_id, max_turns, base_url, prompt, None, false)
}

fn run_tm_with_event_log(
    config: &std::path::Path,
    session_id: &str,
    max_turns: &str,
    base_url: &str,
    prompt: &str,
    event_log: Option<&std::path::Path>,
    turn_budget_ok: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tm"));
    command
        .arg("--config")
        .arg(config)
        .arg("--session-id")
        .arg(session_id)
        .arg("--max-turns")
        .arg(max_turns);
    if let Some(event_log) = event_log {
        command.arg("--event-log").arg(event_log);
    }
    if turn_budget_ok {
        command.arg("--turn-budget-ok");
    }
    let mut child = command
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

fn read_events(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn cli_event_log_classifies_allowed_turn_budget_exhaustion() {
    let (output, events) = run_turn_budget_exhaustion(true);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "tool_call",
            "turn_end",
            "cell_start",
            "cell_result",
            "turn_budget_exhausted"
        ]
    );
    assert_eq!(events.last().unwrap()["maxTurns"], 1);
    assert!(!events.iter().any(|event| event["type"] == "final"));
}

#[test]
fn cli_turn_budget_exhaustion_remains_an_error_without_opt_in() {
    let (output, events) = run_turn_budget_exhaustion(false);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("turn budget exhausted after 1 turns")
    );
    assert_eq!(events.last().unwrap()["type"], "turn_budget_exhausted");
    assert_eq!(events.last().unwrap()["maxTurns"], 1);
}

fn run_turn_budget_exhaustion(turn_budget_ok: bool) -> (Output, Vec<serde_json::Value>) {
    let repo = tempfile::tempdir().unwrap();
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
    let event_log = repo.path().join("exhausted-events.jsonl");
    let server = StubOpenAiServer::start_exhaustion();
    let output = run_tm_with_event_log(
        &config,
        "cli-exhaustion-test",
        "1",
        &server.base_url(),
        "Use execute once.",
        Some(&event_log),
        turn_budget_ok,
    );
    server.join();
    let events = read_events(&event_log);
    (output, events)
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

            let code = r#"let doc = @fs.read {path: "repo:Cargo.toml"};
{ok: contains "[workspace]" doc.content} |> display {kind: "json"}"#;
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

            let code = r##"
let before = @fs.read {path: "repo:src/lib.rs"};
let hits = @code.search {
  pattern: "\"old\"",
  paths: ["repo:src/lib.rs"],
  regex: false
};
let hit = match hits { | first :: _ -> first | [] -> null };
let edit = @fs.patch {
  path: hit.path,
  tag: hit.tag,
  hunks: [{
    op: "replace",
    startLine: hit.line,
    endLine: hit.line,
    expectedLines: [hit.text],
    lines: ["    \"native\""]
  }]
};
let after = @fs.read {path: "repo:src/lib.rs"};
let tests = @proc.run {cmd: "cargo", args: ["test"],
  cwd: "repo:",
  outputBytes: 20000
};
let denied = handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error {
  | ApprovalTimeoutError {message, ...} -> {name: "ApprovalTimeoutError", message: message}
  | other -> rethrow other
};
let summary = {
  beforeHadOld: contains "\"old\"" before.content,
  changed: contains "\"native\"" after.content,
  editChanged: edit.changed,
  testExit: tests.exitCode,
  testStdoutHasOk: contains "test result: ok" tests.stdout,
  denied: denied
};
let artifact = @artifacts.put {data: "#{summary}",
  title: "cli-cutover-transcript",
  mime: "application/json"
};
{
  beforeHadOld: summary.beforeHadOld,
  changed: summary.changed,
  editChanged: summary.editChanged,
  testExit: summary.testExit,
  testStdoutHasOk: summary.testStdoutHasOk,
  denied: summary.denied,
  artifact: artifact.uri
} |> display {kind: "json"}
"##;
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
            assert!(second_body.contains("fs.patch"));
            let tool_content = tool_message_content(&second_body);
            assert!(tool_content.contains("artifact://0"), "{tool_content}");
            assert!(
                tool_content.contains("ApprovalTimeoutError"),
                "{tool_content}"
            );
            assert!(tool_content.contains("\"testExit\": 0"), "{tool_content}");
            assert!(
                tool_content.contains("\"testStdoutHasOk\": true"),
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

    fn start_exhaustion() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let body = read_request_from(&mut stream);
            assert!(body.contains("Use execute once."));
            let args = json!({ "code": "1" }).to_string();
            let chunk = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_cli_exhaustion",
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
            respond_sse(&mut stream, &[chunk]);
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
