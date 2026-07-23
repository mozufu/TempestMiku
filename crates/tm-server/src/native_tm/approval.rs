use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tm_host::{ApprovalDecision as HostApprovalDecision, ApprovalPolicy, HostError};
use uuid::Uuid;

use crate::{
    ApprovalBroker, ApprovalOption, ApprovalPrompt, ApprovalStatus, CodingEventSink,
    DetailedApprovalOutcome, Result, ServerError,
};

const NATIVE_TM_BACKEND: &str = "native-tm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeApprovalMode {
    Deny,
    Manual,
}

impl NativeApprovalMode {
    pub fn parse(mode: &str) -> Result<Self> {
        match mode {
            "" | "deny" => Ok(Self::Deny),
            "manual" => Ok(Self::Manual),
            other => Err(ServerError::InvalidRequest(format!(
                "unsupported approval mode {other}"
            ))),
        }
    }
}
pub struct HttpApprovalPolicy {
    broker: Arc<ApprovalBroker>,
    session_id: Uuid,
    sink: Arc<dyn CodingEventSink>,
    actor_id: Option<String>,
}

impl HttpApprovalPolicy {
    pub fn new(
        broker: Arc<ApprovalBroker>,
        session_id: Uuid,
        sink: Arc<dyn CodingEventSink>,
    ) -> Self {
        Self {
            broker,
            session_id,
            sink,
            actor_id: None,
        }
    }

    pub fn with_actor_id(mut self, actor_id: Option<impl Into<String>>) -> Self {
        self.actor_id = actor_id.map(Into::into);
        self
    }
}

#[async_trait]
impl ApprovalPolicy for HttpApprovalPolicy {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<HostApprovalDecision> {
        let action = action.to_string();
        let detailed = self
            .broker
            .request_permission_detailed_for_backend(
                self.session_id,
                NATIVE_TM_BACKEND,
                approval_prompt(&action, self.actor_id.as_deref()),
                timeout,
                Arc::clone(&self.sink),
            )
            .await
            .map_err(|err| HostError::HostCall(err.to_string()))?;
        host_decision(&action, detailed)
    }
}

fn approval_prompt(action: &str, actor_id: Option<&str>) -> ApprovalPrompt {
    let mut scope = serde_json::Map::new();
    scope.insert("action".to_string(), json!(action));
    scope.insert(
        "capability".to_string(),
        json!(action.split_whitespace().next().unwrap_or(action)),
    );
    if let Some(actor_id) = actor_id {
        scope.insert("actorId".to_string(), json!(actor_id));
    }
    if let Some(summary) = summarize_action(action) {
        scope.insert("summary".to_string(), json!(summary));
    }
    ApprovalPrompt {
        action: action.to_string(),
        scope: Value::Object(scope),
        options: vec![
            ApprovalOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            ApprovalOption {
                option_id: "reject".to_string(),
                name: "Reject once".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    }
}

/// Turns the raw JSON/ad hoc `action` string that tm-host, tm-egress, tm-mcp,
/// and tm-drive build for `require_approval` into a short human sentence for
/// the approval card. `action` remains the audit source of truth; a `None`
/// here just means the client falls back to showing `action` directly rather
/// than guessing on an unrecognized shape.
fn summarize_action(action: &str) -> Option<String> {
    match serde_json::from_str::<Value>(action) {
        Ok(value) => summarize_json_action(&value),
        Err(_) => summarize_ad_hoc_action(action),
    }
}

fn summarize_json_action(value: &Value) -> Option<String> {
    let operation = value.get("operation")?.as_str()?;
    if let Some(details) = value.get("details") {
        return summarize_nested_action(operation, details);
    }
    if operation == "http.request" {
        return summarize_egress_action(value);
    }
    None
}

fn summarize_nested_action(operation: &str, details: &Value) -> Option<String> {
    match operation {
        "proc.run" => {
            let argv0 = details.get("argvPreview")?.as_array()?.first()?.as_str()?;
            let cwd = details.get("cwd")?.get("linkedPath")?.as_str()?;
            match details.get("timeoutMs").and_then(Value::as_u64) {
                Some(timeout_ms) => Some(format!(
                    "在 {cwd} 執行 {argv0}，最長 {}",
                    format_duration_ms(timeout_ms)
                )),
                None => Some(format!("在 {cwd} 執行 {argv0}")),
            }
        }
        "fs.write" => Some(format!("覆寫 {}", details.get("path")?.as_str()?)),
        "fs.remove" => Some(format!("刪除 {}", details.get("path")?.as_str()?)),
        "fs.move" => Some(format!(
            "將 {} 搬移到 {}",
            details.get("source")?.as_str()?,
            details.get("destination")?.as_str()?,
        )),
        _ if operation.starts_with("git.") => summarize_git_action(operation, details),
        _ => None,
    }
}

fn summarize_git_action(operation: &str, details: &Value) -> Option<String> {
    let cwd = details.get("cwd")?.as_str()?;
    let verb = match operation {
        "git.commit" => "提交變更",
        "git.push" => "推送變更",
        "git.pull" => "拉取變更",
        "git.clone" => "複製版本庫",
        "git.init" => "初始化版本庫",
        "git.add" => "加入變更到暫存區",
        "git.mv" => "搬移版本控制的檔案",
        "git.restore" => "還原檔案",
        "git.rm" => "移除版本控制的檔案",
        "git.bisect" => "執行版本回溯查找",
        "git.log" => "查看版本紀錄",
        "git.show" => "查看版本內容",
        "git.status" => "檢查版本狀態",
        "git.diff" => "檢查差異",
        "git.grep" => "搜尋版本內容",
        _ => return None,
    };
    Some(format!("在 {cwd} {verb}"))
}

fn summarize_egress_action(value: &Value) -> Option<String> {
    let host = value.get("host")?.as_str()?;
    let path = value.get("path").and_then(Value::as_str).unwrap_or("");
    let method = value.get("method").and_then(Value::as_str).unwrap_or("GET");
    Some(format!("傳送 {method} 請求到 {host}{path}"))
}

fn summarize_ad_hoc_action(action: &str) -> Option<String> {
    if action == "drive.organize apply" {
        return Some("套用 Drive 整理提案".to_string());
    }
    if let Some(rest) = action.strip_prefix("MCP mutation ") {
        let server_and_name = rest.split(" target=").next()?;
        return Some(format!("呼叫外部工具 {server_and_name}"));
    }
    if let Some(path) = action.strip_prefix("drive.put ") {
        return Some(format!("保存到 Drive：{path}"));
    }
    if let Some(rest) = action.strip_prefix("drive.move ") {
        return Some(match rest.split_once(" -> ") {
            Some((from, to)) => format!("在 Drive 將 {from} 搬移到 {to}"),
            None => format!("在 Drive 搬移：{rest}"),
        });
    }
    if let Some(path) = action.strip_prefix("drive.tag ") {
        return Some(format!("更新 Drive 標籤：{path}"));
    }
    if let Some(path) = action.strip_prefix("project.link ") {
        return Some(format!("連結資料夾：{path}"));
    }
    if let Some(alias) = action.strip_prefix("project.unlink ") {
        return Some(format!("解除資料夾連結：{alias}"));
    }
    None
}

fn format_duration_ms(timeout_ms: u64) -> String {
    let seconds = timeout_ms / 1000;
    if seconds == 0 {
        return format!("{timeout_ms} 毫秒");
    }
    if seconds.is_multiple_of(60) {
        format!("{} 分鐘", seconds / 60)
    } else {
        format!("{seconds} 秒")
    }
}

fn host_decision(
    action: &str,
    detailed: DetailedApprovalOutcome,
) -> tm_host::Result<HostApprovalDecision> {
    match detailed.status {
        ApprovalStatus::Approved => Ok(HostApprovalDecision::Approved),
        ApprovalStatus::Denied | ApprovalStatus::Cancelled => Ok(HostApprovalDecision::Denied),
        ApprovalStatus::TimedOut => Err(HostError::ApprovalTimeout(action.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_proc_run() {
        let action = json!({
            "operation": "proc.run",
            "details": {
                "argvPreview": ["cargo", "test"],
                "cwd": {"linkedPath": "tempestmiku/apps/tm-cli", "device": 1, "inode": 2},
                "timeoutMs": 120_000,
            },
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("在 tempestmiku/apps/tm-cli 執行 cargo，最長 2 分鐘")
        );
    }

    #[test]
    fn summarizes_proc_run_without_whole_minute_timeout() {
        let action = json!({
            "operation": "proc.run",
            "details": {
                "argvPreview": ["cargo"],
                "cwd": {"linkedPath": "tempestmiku"},
                "timeoutMs": 45_000,
            },
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("在 tempestmiku 執行 cargo，最長 45 秒")
        );
    }

    #[test]
    fn summarizes_git_commit() {
        let action = json!({
            "operation": "git.commit",
            "details": {
                "cwd": "tempestmiku",
                "fixedArgv": ["commit", "-m", "[commit message; see digest/preview]"],
                "network": false,
            },
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("在 tempestmiku 提交變更")
        );
    }

    #[test]
    fn summarizes_git_push() {
        let action = json!({
            "operation": "git.push",
            "details": {"cwd": "tempestmiku", "network": true},
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("在 tempestmiku 推送變更")
        );
    }

    #[test]
    fn summarizes_fs_write_overwrite() {
        let action = json!({
            "operation": "fs.write",
            "details": {"path": "notes.md", "effect": "overwrite", "expectedTag": "abc123"},
        })
        .to_string();
        assert_eq!(summarize_action(&action).as_deref(), Some("覆寫 notes.md"));
    }

    #[test]
    fn summarizes_fs_remove() {
        let action = json!({
            "operation": "fs.remove",
            "details": {"path": "notes.md", "effect": "remove", "expectedTag": "abc123"},
        })
        .to_string();
        assert_eq!(summarize_action(&action).as_deref(), Some("刪除 notes.md"));
    }

    #[test]
    fn summarizes_fs_move() {
        let action = json!({
            "operation": "fs.move",
            "details": {
                "source": "draft.md",
                "destination": "final.md",
                "effect": "overwrite_destination",
            },
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("將 draft.md 搬移到 final.md")
        );
    }

    #[test]
    fn summarizes_egress_http_request() {
        let action = json!({
            "operation": "http.request",
            "method": "POST",
            "scheme": "https",
            "host": "api.example.com",
            "port": 443,
            "path": "/v1/messages",
        })
        .to_string();
        assert_eq!(
            summarize_action(&action).as_deref(),
            Some("傳送 POST 請求到 api.example.com/v1/messages")
        );
    }

    #[test]
    fn summarizes_mcp_mutation() {
        let action = "MCP mutation docs/publish target=sha256:abc input={\"a\": 1}";
        assert_eq!(
            summarize_action(action).as_deref(),
            Some("呼叫外部工具 docs/publish")
        );
    }

    #[test]
    fn summarizes_drive_put() {
        assert_eq!(
            summarize_action("drive.put reports/q1.md").as_deref(),
            Some("保存到 Drive：reports/q1.md")
        );
    }

    #[test]
    fn summarizes_drive_move() {
        assert_eq!(
            summarize_action("drive.move draft.md -> final.md").as_deref(),
            Some("在 Drive 將 draft.md 搬移到 final.md")
        );
    }

    #[test]
    fn summarizes_project_link() {
        assert_eq!(
            summarize_action("project.link /home/brian/tempestmiku").as_deref(),
            Some("連結資料夾：/home/brian/tempestmiku")
        );
    }

    #[test]
    fn summarizes_drive_organize_apply() {
        assert_eq!(
            summarize_action("drive.organize apply").as_deref(),
            Some("套用 Drive 整理提案")
        );
    }

    #[test]
    fn unrecognized_shapes_stay_none() {
        assert_eq!(summarize_action("Switch to Serious Engineer mode"), None);
        assert_eq!(
            summarize_action(&json!({"operation": "skill.install"}).to_string()),
            None
        );
    }

    #[test]
    fn bounded_digest_fallback_stays_none() {
        // approval_action() falls back to a digest-only shape when the
        // encoded JSON exceeds MAX_APPROVAL_ACTION_BYTES; the summarizer must
        // not panic or fabricate a sentence from missing fields.
        let action = json!({
            "operation": "proc.run",
            "details": {"bounded": true, "sha256": "deadbeef"},
        })
        .to_string();
        assert_eq!(summarize_action(&action), None);
    }

    #[test]
    fn approval_prompt_carries_summary_when_recognized() {
        let action = json!({
            "operation": "fs.remove",
            "details": {"path": "notes.md", "effect": "remove", "expectedTag": "abc"},
        })
        .to_string();
        let prompt = approval_prompt(&action, None);
        assert_eq!(prompt.scope["summary"], json!("刪除 notes.md"));
    }

    #[test]
    fn approval_prompt_omits_summary_when_unrecognized() {
        let prompt = approval_prompt("skill.install release-workflow", None);
        assert!(prompt.scope.get("summary").is_none());
    }
}
