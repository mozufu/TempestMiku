use super::*;

const NATIVE_P3_BROADCAST_TEXT: &str = "native P3 plus broadcast token";
pub(super) const NATIVE_P3_FINAL_TEXT: &str = "native P3 plus coordination complete";

pub(super) async fn assert_native_child_resources(
    client: &MikuClient,
    session_id: &str,
    linked: &E2eEvent,
) {
    let actor_id = linked.data["actor_id"]
        .as_str()
        .expect("actor_resources_linked actor_id");
    let history_uri = linked.data["history_uri"]
        .as_str()
        .expect("actor_resources_linked history_uri");
    let artifact_uri = linked.data["artifact_uri"]
        .as_str()
        .expect("actor_resources_linked artifact_uri");
    assert_eq!(history_uri, format!("history://{actor_id}"));
    assert!(artifact_uri.starts_with("artifact://"));

    let artifact = client
        .resolve_resource(session_id, artifact_uri)
        .await
        .unwrap();
    let artifact_content = artifact["content"].as_str().unwrap();
    assert!(
        artifact_content.contains(NATIVE_P3_BROADCAST_TEXT),
        "artifact {artifact_uri} did not contain broadcast token: {artifact_content}"
    );

    let history = client
        .resolve_resource(session_id, history_uri)
        .await
        .unwrap();
    let history_content = history["content"].as_str().unwrap();
    assert!(history_content.contains("[tool_call] execute"));
    assert!(history_content.contains("[cell_start] [redacted]"));
    assert!(history_content.contains("[cell_result]"));
    assert!(!history_content.contains("@agents.wait"));

    let agent_uri = format!("agent://{actor_id}");
    let agent = client
        .resolve_resource(session_id, &agent_uri)
        .await
        .unwrap();
    let record: serde_json::Value =
        serde_json::from_str(agent["content"].as_str().unwrap()).unwrap();
    assert_eq!(record["status"], json!("terminated"));
    assert_eq!(record["cancelled"], json!(false));
    assert_eq!(record["artifact_uri"], json!(artifact_uri));
    assert_eq!(record["history_uri"], json!(history_uri));
}

pub(super) fn drive_smoke_code() -> String {
    r##"
let filed = @drive.put {content: "# Approval Drop\nManual approval gates drive writes.\nResearch smoke citation body.", options: {
  auto: true,
  suggestedPath: "inbox/approval-drop.md",
  project: "tempestmiku",
  docKind: "note",
  sourceUri: "drop://browser/approval-drop.md",
  eventSeq: 101
}};
let hits = @drive.search {query: "approval", project: "tempestmiku", returnSnippets: true, limit: 1};
match hits {
  | first :: _ -> do {
      let read = @drive.get {uri: first.uri, selector: "1-20"};
      {
        filedUri: filed.uri,
        sourceUri: filed.entry.sourceUri,
        searchHits: length hits,
        sourceKind: "drive",
        citationUri: first.uri,
        answerHasDriveUri: contains "drive://" first.uri,
        resolvedHasBody: contains "Research smoke citation body" read.content
      } |> display {kind: "json"}
    }
  | [] -> {
        filedUri: filed.uri,
        sourceUri: filed.entry.sourceUri,
        searchHits: 0,
        sourceKind: "missing",
        citationUri: "missing",
        answerHasDriveUri: false,
        resolvedHasBody: false
      } |> display {kind: "json"}
}
"##
    .to_string()
}

pub(super) fn test_linked_project(root: &std::path::Path) -> LinkedFolders {
    LinkedFolders::from_configs(vec![LinkedFolderConfig {
        name: "tempestmiku".to_string(),
        path: root.to_path_buf(),
        mode: FsMode::Ro,
        commands: Vec::new(),
        safe_args: Vec::new(),
    }])
    .expect("active TempestMiku test project")
}

pub(super) fn native_parent_coordination_code() -> String {
    format!(
        r#"
let alpha = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report.", opts: {{capabilities: ["agents.*"]}}}};
let beta = @agents.spawn {{role: "worker", task: "Wait for the parent broadcast, write a short artifact, and send Root a report.", opts: {{capabilities: ["agents.*"]}}}};
let readyA = @agents.wait {{from: alpha, timeoutMs: 15000}};
let readyB = @agents.wait {{from: beta, timeoutMs: 15000}};
let receipts = @agents.broadcast {{text: "{broadcast}"}};
let first = @agents.wait {{from: alpha, timeoutMs: 15000}};
let second = @agents.wait {{from: beta, timeoutMs: 15000}};
let roster = @agents.list {{}};
{{
  receipts,
  ready: [readyA.text, readyB.text],
  reports: [first.text, second.text],
  roster
}} |> display {{kind: "json"}}
"#,
        broadcast = NATIVE_P3_BROADCAST_TEXT
    )
}

pub(super) fn native_child_coordination_code() -> String {
    r#"
let ready = @agents.send {to: "Root", text: "child ready for native broadcast"};
let msg = @agents.wait {from: "Root", timeoutMs: 15000};
let text = msg.text;
let artifact = @artifacts.put {data: "native child saw: #{text}", title: "native p3 child"};
let report = @agents.send {to: "Root", text: "child report #{artifact.uri}: #{text}"};
{text, artifact: artifact.uri} |> display {kind: "json"}
"#
    .to_string()
}

pub(super) fn execute_script(id: &str, code: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCall {
            index: 0,
            id: Some(id.to_string()),
            name: Some("execute".to_string()),
            arguments: Some(json!({ "code": code }).to_string()),
        },
        StreamEvent::Finish {
            reason: Some("tool_calls".to_string()),
        },
    ]
}

pub(super) fn text_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::Text(text.to_string()),
        StreamEvent::Finish {
            reason: Some("stop".to_string()),
        },
    ]
}
