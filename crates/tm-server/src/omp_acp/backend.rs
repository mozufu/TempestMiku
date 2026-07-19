use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::{
    config::OmpAcpConfig,
    worker::{OmpTurnRequest, OmpWorker},
};
use crate::{
    ApprovalBroker, CodingBackend, CodingEventSink, CodingTurn, CodingTurnResult, Result,
    ServerError,
};

pub struct OmpAcpBackend {
    config: OmpAcpConfig,
    approval_broker: Arc<ApprovalBroker>,
    sessions: Mutex<BTreeMap<Uuid, mpsc::Sender<OmpTurnRequest>>>,
}

impl OmpAcpBackend {
    pub fn new(config: OmpAcpConfig, approval_broker: Arc<ApprovalBroker>) -> Result<Self> {
        config.validate()?;
        verify_omp_version(&config)?;
        Ok(Self {
            config,
            approval_broker,
            sessions: Mutex::new(BTreeMap::new()),
        })
    }

    fn sender_for_session(&self, session_id: Uuid) -> mpsc::Sender<OmpTurnRequest> {
        let mut sessions = self.sessions.lock();
        sessions
            .entry(session_id)
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(8);
                let worker = OmpWorker::new(
                    session_id,
                    self.config.clone(),
                    Arc::clone(&self.approval_broker),
                    receiver,
                );
                tokio::spawn(async move {
                    if let Err(err) = worker.run().await {
                        let err = tm_memory::redact_dream_text(&err.to_string()).text;
                        tracing::warn!(%err, %session_id, "omp acp worker stopped");
                    }
                });
                sender
            })
            .clone()
    }
}

#[async_trait]
impl CodingBackend for OmpAcpBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> Result<CodingTurnResult> {
        let session_id = turn.session_id;
        let sender = self.sender_for_session(session_id);
        let (response_tx, response_rx) = oneshot::channel();
        let request = OmpTurnRequest {
            turn,
            sink,
            response: response_tx,
        };
        if sender.send(request).await.is_err() {
            self.sessions.lock().remove(&session_id);
            return Err(ServerError::Backend(
                "omp acp worker stopped before accepting turn".to_string(),
            ));
        }
        response_rx.await.map_err(|_| {
            ServerError::Backend("omp acp worker stopped before turn completed".to_string())
        })?
    }
}

fn verify_omp_version(config: &OmpAcpConfig) -> Result<()> {
    let output = std::process::Command::new(&config.command)
        .arg("--version")
        .output()
        .map_err(|err| ServerError::Backend(format!("failed to run omp --version: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let actual = if stdout.is_empty() { stderr } else { stdout };
    if actual == config.expected_version {
        Ok(())
    } else {
        Err(ServerError::Backend(format!(
            "omp version mismatch: expected {}, got {}",
            config.expected_version, actual
        )))
    }
}
