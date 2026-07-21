use super::*;

pub(super) struct ScriptedLlm {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ScriptedLlm {
    pub(super) fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> CoreResult<BoxStream<'static, CoreResult<StreamEvent>>> {
        self.requests
            .lock()
            .map_err(|_| CoreError::Llm("scripted request lock poisoned".to_string()))?
            .push(req.messages.clone());
        let events = self
            .scripts
            .lock()
            .map_err(|_| CoreError::Llm("scripted LLM lock poisoned".to_string()))?
            .pop_front()
            .ok_or_else(|| CoreError::Llm("scripted LLM exhausted".to_string()))?;
        Ok(Box::pin(stream::iter(
            events.into_iter().map(Ok::<StreamEvent, CoreError>),
        )))
    }
}

pub(super) struct ArtifactBackend {
    pub(super) root: PathBuf,
}

pub(super) struct ActorSmokeBackend {
    pub(super) root: PathBuf,
    pub(super) broker: Arc<ApprovalBroker>,
    pub(super) roster: Arc<MailboxRegistry>,
}

#[async_trait]
impl CodingBackend for ArtifactBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        assert_eq!(turn.mode, ModeId::from("serious_engineer"));
        let store = tm_artifacts::ArtifactStore::open(&self.root, turn.session_id.to_string())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let artifact = store
            .put_text(
                "tm-e2e transcript artifact\n",
                Some("tm-e2e transcript".to_string()),
                "text/plain",
            )
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sink.emit("text", json!({ "delta": "coding through backend" }))
            .await?;
        sink.emit(
            "artifact",
            json!({
                "backend": "tm-e2e-test",
                "artifact": artifact,
            }),
        )
        .await?;
        let final_text = "E2E coding backend complete. Open loop: keep the hatch covered. Decision: keep the hatch HTTP-only. artifact://0".to_string();
        sink.emit(
            "final",
            to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }
}

#[async_trait]
impl CodingBackend for ActorSmokeBackend {
    async fn run_turn(
        &self,
        turn: CodingTurn,
        sink: Arc<dyn CodingEventSink>,
    ) -> tm_server::Result<CodingTurnResult> {
        assert_eq!(turn.mode, ModeId::from("serious_engineer"));
        let actor_id =
            ActorId::new("Worker0").map_err(|err| ServerError::InvalidRequest(err.to_string()))?;
        let actor_id_text = actor_id.to_string();
        let actor_session_id = turn.session_id.to_string();
        self.roster
            .track_for_session(
                &actor_session_id,
                actor_record(
                    actor_id.clone(),
                    "worker",
                    ActorStatus::Running,
                    false,
                    None,
                ),
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sink.emit(
            "actor_spawned",
            json!({
                "actor_id": actor_id_text,
                "role": "worker",
                "task": "actor smoke",
            }),
        )
        .await?;
        let approval = self
            .broker
            .request_permission_detailed_for_backend(
                turn.session_id,
                "native-tm",
                ApprovalPrompt {
                    action: "proc.run cargo clean".to_string(),
                    scope: json!({
                        "actorId": actor_id_text,
                        "action": "proc.run cargo clean",
                        "capability": "proc.run",
                    }),
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
                },
                Duration::from_secs(5),
                Arc::clone(&sink),
            )
            .await?;
        assert_eq!(approval.status, ApprovalStatus::Approved);

        let store = tm_artifacts::ArtifactStore::open(&self.root, turn.session_id.to_string())
            .map_err(|err| ServerError::Store(err.to_string()))?;
        let artifact = store
            .put_text(
                "child smoke artifact opened through the resource gateway\n",
                Some("child smoke artifact".to_string()),
                "text/plain",
            )
            .map_err(|err| ServerError::Store(err.to_string()))?;
        self.roster
            .store_transcript_for_session(
                &actor_session_id,
                &actor_id,
                "child smoke transcript\n[cell_result] artifact://0\n".to_string(),
            )
            .await;
        self.roster
            .mark_complete_with_resources_for_session(
                &actor_session_id,
                &actor_id,
                Some("artifact://0".to_string()),
                Some(format!("history://{actor_id_text}")),
            )
            .await;
        sink.emit(
            "artifact",
            json!({
                "backend": "tm-e2e-actor-smoke",
                "artifact": artifact,
            }),
        )
        .await?;
        sink.emit(
            "actor_completed",
            json!({
                "actor_id": actor_id_text,
                "summary": "child smoke complete",
                "artifact_uri": "artifact://0",
                "history_uri": format!("history://{actor_id_text}"),
            }),
        )
        .await?;
        sink.emit(
            "actor_resources_linked",
            json!({
                "kind": "resources_linked",
                "actor_id": actor_id_text,
                "source_event_type": "actor_completed",
                "source_event_seq": null,
                "artifact_uri": "artifact://0",
                "history_uri": format!("history://{actor_id_text}"),
            }),
        )
        .await?;
        let cancelled_actor_id = ActorId::new("CancelledWorker")
            .map_err(|err| ServerError::InvalidRequest(err.to_string()))?;
        let cancelled_actor_id_text = cancelled_actor_id.to_string();
        self.roster
            .track_for_session(
                &actor_session_id,
                actor_record(
                    cancelled_actor_id,
                    "watcher",
                    ActorStatus::Terminated,
                    true,
                    Some(FailureReason::Cancelled),
                ),
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        sink.emit(
            "actor_cancelled",
            json!({
                "kind": "cancelled",
                "actor_id": cancelled_actor_id_text,
                "cancelled_at": Utc::now(),
            }),
        )
        .await?;
        let final_text = "Actor smoke complete with artifact://0".to_string();
        sink.emit(
            "final",
            to_value(StoreEvent::Final {
                text: final_text.clone(),
            })?,
        )
        .await?;
        Ok(CodingTurnResult {
            final_text,
            transcript_artifact: None,
        })
    }
}

fn actor_record(
    id: ActorId,
    mode: &str,
    status: ActorStatus,
    cancelled: bool,
    failure_reason: Option<FailureReason>,
) -> ActorRecord {
    let now = Utc::now();
    ActorRecord {
        id,
        parent: None,
        status,
        mode: Some(mode.to_string()),
        budget: ActorBudget::default(),
        spawned_at: now,
        completed_at: (status == ActorStatus::Terminated).then_some(now),
        cancelled,
        failure_reason,
        artifact_uri: None,
        history_uri: None,
    }
}
