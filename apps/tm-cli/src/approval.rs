use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::{Result, bail};
use async_trait::async_trait;
use tm_host::{
    ApprovalDecision, ApprovalPolicy, DefaultDenyApprovalPolicy, HostError, P0HostConfig,
};

pub(super) fn approval_policy(config: &P0HostConfig) -> Result<Arc<dyn ApprovalPolicy>> {
    match config.approvals.mode.as_str() {
        "manual" => Ok(Arc::new(PromptApprovalPolicy)),
        "deny" | "" => Ok(Arc::new(DefaultDenyApprovalPolicy)),
        other => bail!("unsupported approval mode {other}"),
    }
}

#[derive(Debug)]
struct PromptApprovalPolicy;

#[async_trait]
impl ApprovalPolicy for PromptApprovalPolicy {
    async fn request(&self, action: &str, timeout: Duration) -> tm_host::Result<ApprovalDecision> {
        let action = action.to_string();
        let thread_action = action.clone();
        let (tx, rx) = mpsc::channel();
        let timeout_ms = timeout.as_millis();
        std::thread::spawn(move || {
            let result = read_tty_approval(&thread_action, timeout_ms);
            let _ = tx.send(result);
        });
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => Err(HostError::ApprovalTimeout(action)),
        }
    }
}

fn read_tty_approval(action: &str, timeout_ms: u128) -> tm_host::Result<ApprovalDecision> {
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    let mut writer = tty
        .try_clone()
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    write!(
        writer,
        "Approval required: {action}\nType approve within {timeout_ms}ms to continue: "
    )
    .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    writer
        .flush()
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    let mut line = String::new();
    BufReader::new(tty)
        .read_line(&mut line)
        .map_err(|_| HostError::ApprovalTimeout(action.to_string()))?;
    if line.trim() == "approve" {
        Ok(ApprovalDecision::Approved)
    } else {
        Ok(ApprovalDecision::Denied)
    }
}
