use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    thread,
};

use serde_json::json;

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
    let mut child = Command::new(env!("CARGO_BIN_EXE_tm"))
        .arg("--config")
        .arg(&config)
        .arg("--session-id")
        .arg("cli-runtime-test")
        .arg("--max-turns")
        .arg("3")
        .env("OPENAI_BASE_URL", server.base_url())
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
        .write_all(b"Use execute to read linked://repo/Cargo.toml, then answer done.")
        .unwrap();
    let output = child.wait_with_output().unwrap();
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

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn join(self) {
        self.handle.join().unwrap();
    }
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
