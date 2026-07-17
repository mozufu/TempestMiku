use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tm_agents::{ActorDigest, ActorError, ActorExecutor, ActorSpec};
use tm_artifacts::ArtifactStore;
use tm_core::{Agent, AgentConfig, CancellationToken, EventSink, InboxDrain, LlmClient, Sandbox};
use tm_host::CapabilityGrants;
use uuid::Uuid;

use super::util::last_artifact_uri_in_text;

type ActorSandboxFactory = dyn Fn(
        Uuid,
        Option<&str>,
        &CapabilityGrants,
        Option<&str>,
        Option<Arc<dyn CancellationToken>>,
    ) -> Arc<dyn Sandbox>
    + Send
    + Sync;

impl ChatActorExecutor {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
    ) -> Self {
        Self::with_artifact_root(llm, cfg, sandbox_factory, None)
    }

    pub fn with_artifact_root(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(Uuid) -> Arc<dyn Sandbox> + Send + Sync + 'static,
        artifact_root: Option<PathBuf>,
    ) -> Self {
        Self {
            llm,
            cfg,
            sandbox_factory: Arc::new(
                move |session_id, _actor_id, _grants, _scope, _cancellation| {
                    sandbox_factory(session_id)
                },
            ),
            artifact_root,
            actor_roster: None,
        }
    }

    pub fn with_actor_context(
        llm: Arc<dyn LlmClient>,
        cfg: AgentConfig,
        sandbox_factory: impl Fn(
            Uuid,
            Option<&str>,
            &CapabilityGrants,
            Option<&str>,
            Option<Arc<dyn CancellationToken>>,
        ) -> Arc<dyn Sandbox>
        + Send
        + Sync
        + 'static,
        artifact_root: Option<PathBuf>,
        actor_roster: Arc<tm_agents::MailboxRegistry>,
    ) -> Self {
        Self {
            llm,
            cfg,
            sandbox_factory: Arc::new(sandbox_factory),
            artifact_root,
            actor_roster: Some(actor_roster),
        }
    }
}

// ─── CollectingSink ───────────────────────────────────────────────────────────

/// Captures all agent events as a plain-text transcript (P3.3).
///
/// Used by `ChatActorExecutor` to record sub-agent output for `history://` resources.
struct CollectingSink(Mutex<Vec<String>>);

impl CollectingSink {
    fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("collecting sink lock")
            .push(tm_memory::redact_dream_text(&line).text);
    }

    fn into_transcript(self) -> String {
        self.0
            .into_inner()
            .expect("collecting sink lock")
            .join("\n")
    }
}

impl EventSink for CollectingSink {
    fn on_text(&self, delta: &str) {
        self.push(format!("[text] {delta}"));
    }
    fn on_tool_call(&self, name: &str) {
        self.push(format!("[tool_call] {name}"));
    }
    fn on_cell_start(&self, code: &str) {
        self.push(format!("[cell_start] {code}"));
    }
    fn on_cell_result(&self, shaped: &str) {
        self.push(format!("[cell_result] {shaped}"));
    }
    fn on_final(&self, text: &str) {
        self.push(format!("[final] {text}"));
    }
}

// ─── ChatActorExecutor ────────────────────────────────────────────────────────

/// Runs sub-agents using the existing [`Agent`] loop without routing through
/// [`AgentChatRunner`]'s blocking channel (which would deadlock when called from
/// inside a parent actor's sandbox cell).
///
/// Injected into [`tm_agents::MailboxRegistry`] at startup; called by the `agents.run`
/// HostFn body.
pub struct ChatActorExecutor {
    llm: Arc<dyn LlmClient>,
    cfg: AgentConfig,
    sandbox_factory: Arc<ActorSandboxFactory>,
    /// Artifact root for reading child cell-spills and writing transcripts (P3.3).
    artifact_root: Option<PathBuf>,
    actor_roster: Option<Arc<tm_agents::MailboxRegistry>>,
}

struct ActorMailboxDrain {
    roster: Arc<tm_agents::MailboxRegistry>,
    actor_id: tm_agents::ActorId,
}

#[async_trait]
impl InboxDrain for ActorMailboxDrain {
    async fn drain(&self) -> tm_core::Result<Vec<String>> {
        Ok(self
            .roster
            .drain_inbox(&self.actor_id, None)
            .await
            .into_iter()
            .map(|message| {
                let reply_to = message
                    .reply_to
                    .as_ref()
                    .map(|id| format!("\nreplyTo: {}", id.as_str()))
                    .unwrap_or_default();
                format!(
                    "from: {}\nsentAt: {}\ntext: {}{}",
                    message.from.as_str(),
                    message.sent_at.to_rfc3339(),
                    message.text,
                    reply_to
                )
            })
            .collect())
    }
}

#[async_trait]
impl ActorExecutor for ChatActorExecutor {
    async fn run_to_digest(&self, spec: ActorSpec) -> std::result::Result<ActorDigest, ActorError> {
        if spec.depth >= spec.budget.max_depth {
            return Err(ActorError::DepthExceeded(spec.depth));
        }
        if spec.cancellation.is_cancelled() {
            return Err(ActorError::Cancelled);
        }

        // TmSandbox sessions are !Send, so we
        // cannot await agent.run() directly in an async_trait Send future. Spawn a
        // dedicated thread with its own single-threaded tokio runtime — the same
        // isolation pattern as AgentChatRunner, but one thread per actor call rather
        // than a shared sequential queue (which would deadlock when called from inside
        // a parent actor's sandbox cell).
        let llm = Arc::clone(&self.llm);
        let mut cfg = self.cfg.clone();
        cfg.system_prompt = format!(
            "You are a specialized sub-agent. Role: {}.\n\
             Complete the assigned task. When finished, provide a plain-prose summary of your result.",
            spec.role,
        );
        let task = spec.task.clone();
        let actor_id = spec.id.clone();
        let cancellation: Arc<dyn CancellationToken> = Arc::new(spec.cancellation.clone());
        let artifact_root = self.artifact_root.clone();
        let owner_session_id = spec
            .session_id
            .parse::<Uuid>()
            .unwrap_or_else(|_| Uuid::new_v4());
        let existing_artifact_ids = artifact_root
            .as_ref()
            .and_then(|root| ArtifactStore::open(root, owner_session_id.to_string()).ok())
            .map(|store| {
                store
                    .list()
                    .into_iter()
                    .map(|artifact| artifact.id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let sandbox = (self.sandbox_factory)(
            owner_session_id,
            Some(actor_id.as_str()),
            &spec.grants,
            spec.session_scope.as_deref(),
            Some(Arc::clone(&cancellation)),
        );
        let inbox = self.actor_roster.as_ref().map(|roster| ActorMailboxDrain {
            roster: Arc::clone(roster),
            actor_id: actor_id.clone(),
        });
        let cancellation_for_loop = Arc::clone(&cancellation);

        let (tx, rx) =
            tokio::sync::oneshot::channel::<std::result::Result<(String, String), ActorError>>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("actor worker runtime builds");
            let agent = Agent::new(llm, sandbox, cfg);
            let sink = CollectingSink::new();
            let result = runtime
                .block_on(agent.run_with_controls(
                    &task,
                    &sink,
                    inbox.as_ref().map(|inbox| inbox as &dyn InboxDrain),
                    Some(cancellation_for_loop.as_ref()),
                ))
                .map_err(|e| match e {
                    tm_core::Error::Cancelled => ActorError::Cancelled,
                    other => ActorError::Execution(other.to_string()),
                });
            let transcript = sink.into_transcript();
            let _ = tx.send(result.map(|summary| (summary, transcript)));
        });

        let (summary, transcript) = rx
            .await
            .map_err(|_| ActorError::Execution("actor worker dropped response".to_string()))??;
        let summary = tm_memory::redact_dream_text(&summary).text;
        let transcript = tm_memory::redact_dream_text(&transcript).text;

        // Populate artifact_uri from this child's own transcript first; concurrent child actors
        // can write to the same session artifact store, so "latest new artifact" is only a fallback.
        let (artifact_uri, history_uri) = if let Some(ref root) = artifact_root {
            match ArtifactStore::open(root, owner_session_id.to_string()) {
                Ok(store) => {
                    let artifact_uri = last_artifact_uri_in_text(&transcript).or_else(|| {
                        store
                            .list()
                            .into_iter()
                            .rev()
                            .find(|artifact| !existing_artifact_ids.contains(&artifact.id))
                            .map(|r| r.uri)
                    });
                    let history_uri =
                        (!transcript.is_empty()).then(|| format!("history://{actor_id}"));
                    (artifact_uri, history_uri)
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        let history_content = if transcript.is_empty() {
            None
        } else {
            Some(transcript)
        };

        Ok(ActorDigest {
            actor_id,
            summary,
            artifact_uri,
            history_uri,
            history_content,
        })
    }
}
