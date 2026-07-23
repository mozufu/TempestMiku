//! Approval-backed mode suggestions exposed through the sandbox SDK.
//!
//! `modes.suggest` is a capability-gated [`tm_host::HostFn`], not a second native model tool.
//! The core agent loop still advertises exactly one tool (`execute`); an unlocked normal chat
//! turn may call `@modes.suggest {...}` from inside that tool. Coding backends, actors,
//! scheduler runs, and locked turns never receive the `modes.suggest` grant.

use async_trait::async_trait;
use tm_host::{GrantDoc, HostError, HostFn, InvocationCtx, ToolDocs, ToolErrorDoc, ToolExample};

use super::modes::{commit_mode_state_parts, validate_mode};
use super::*;

pub(super) const MODE_SUGGEST_CAPABILITY: &str = "modes.suggest";
pub(super) const MODE_SUGGEST_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModeSuggestArgs {
    target_mode: String,
    #[serde(default)]
    reason: Option<String>,
}

type SessionEventSender = dyn Fn(Uuid) -> broadcast::Sender<SessionEvent> + Send + Sync + 'static;

pub(super) struct ModeSuggestHostFn<S> {
    store: Arc<S>,
    persona: ModesConfig,
    approval_broker: Arc<ApprovalBroker>,
    sender_for: Arc<SessionEventSender>,
    timeout: Duration,
    docs: ToolDocs,
}

impl<S> ModeSuggestHostFn<S>
where
    S: Store,
{
    pub(super) fn new<M, C>(state: &AppState<S, M, C>, timeout: Duration) -> Self {
        let live_events = Arc::clone(&state.live_events);
        let sender_for = Arc::new(move |session_id| {
            live_events
                .lock()
                .entry(session_id)
                .or_insert_with(|| broadcast::channel(256).0)
                .clone()
        });
        let known_modes = state
            .persona
            .load_assets()
            .modes
            .modes
            .iter()
            .map(|profile| profile.mode.as_str().to_string())
            .collect::<Vec<_>>();
        Self {
            store: Arc::clone(&state.store),
            persona: state.persona.clone(),
            approval_broker: Arc::clone(&state.approval_broker),
            sender_for,
            timeout,
            docs: mode_suggest_docs(known_modes),
        }
    }

    fn outcome(
        status: &str,
        current_mode: &ModeId,
        target_mode: &str,
        changed: bool,
        message: impl Into<String>,
    ) -> Value {
        json!({
            "status": status,
            "currentMode": current_mode.as_str(),
            "targetMode": target_mode,
            "changed": changed,
            "message": message.into(),
        })
    }
}

#[async_trait]
impl<S> HostFn for ModeSuggestHostFn<S>
where
    S: Store,
{
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let session_id = ctx.session_id.parse::<Uuid>().map_err(|_| {
            HostError::InvalidArgs("modes.suggest requires an authorized session context".into())
        })?;
        let args: ModeSuggestArgs = serde_json::from_value(args)
            .map_err(|error| HostError::InvalidArgs(error.to_string()))?;
        let target = args.target_mode.trim();
        if target.is_empty()
            || target.len() > 128
            || !target.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(HostError::InvalidArgs(
                "targetMode must be 1-128 safe ASCII characters".to_string(),
            ));
        }
        let reason = args
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("model-proposed mode switch");
        if reason.len() > 2_048 {
            return Err(HostError::InvalidArgs(
                "reason must be at most 2048 bytes".to_string(),
            ));
        }
        let reason = tm_memory::redact_dream_text(reason).text;

        // Always reload authoritative state. A handler retained by a cached runtime session never
        // retains a per-turn sink or a mode snapshot.
        let session = self
            .store
            .get_session(session_id)
            .await
            .map_err(host_error)?;
        let current_mode = session.mode_state.mode.clone();
        if session.mode_state.lock_source.is_some() {
            return Ok(Self::outcome(
                "locked",
                &current_mode,
                target,
                false,
                format!("Mode is locked by the user; the switch to {target} was not offered."),
            ));
        }

        let target_mode = match validate_mode(&self.persona, ModeId::from(target)) {
            Ok(mode) => mode,
            Err(_) => {
                return Ok(Self::outcome(
                    "invalid_target",
                    &current_mode,
                    target,
                    false,
                    format!("{target} is not a known mode; the switch was not offered."),
                ));
            }
        };
        if target_mode == current_mode {
            return Ok(Self::outcome(
                "already_active",
                &current_mode,
                target,
                false,
                format!("Already in {target_mode}."),
            ));
        }

        let profile = mode_profile(&self.persona, &target_mode);
        let prompt = ApprovalPrompt {
            action: format!("Switch to {} mode", profile.label),
            scope: json!({
                "targetMode": target_mode.as_str(),
                "reason": reason,
                "currentMode": current_mode.as_str(),
                "summary": format!("切換到「{}」模式", profile.label),
            }),
            options: vec![
                ApprovalOption {
                    option_id: "allow".to_string(),
                    name: "Switch mode".to_string(),
                    kind: "allow_once".to_string(),
                },
                ApprovalOption {
                    option_id: "reject".to_string(),
                    name: "Stay in current mode".to_string(),
                    kind: "reject_once".to_string(),
                },
            ],
        };
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&self.store),
            (self.sender_for)(session_id),
        ));
        let approval = self
            .approval_broker
            .request_permission_detailed_for_backend(session_id, "mode", prompt, self.timeout, sink)
            .await
            .map_err(host_error)?;

        match approval.status {
            ApprovalStatus::Approved => {
                // Recheck lock and mode after approval; the user may have changed either while
                // the proposal was pending.
                let latest = self
                    .store
                    .get_session(session_id)
                    .await
                    .map_err(host_error)?;
                if latest.mode_state.lock_source.is_some() {
                    return Ok(Self::outcome(
                        "locked",
                        &latest.mode_state.mode,
                        target,
                        false,
                        format!(
                            "Mode was locked by the user while the proposal was pending; staying in {}.",
                            latest.mode_state.mode
                        ),
                    ));
                }
                if latest.mode_state.mode == target_mode {
                    return Ok(Self::outcome(
                        "already_active",
                        &latest.mode_state.mode,
                        target,
                        false,
                        format!("Already in {target_mode}."),
                    ));
                }
                if latest.mode_state.mode != current_mode {
                    return Ok(Self::outcome(
                        "stale",
                        &latest.mode_state.mode,
                        target,
                        false,
                        format!(
                            "Mode changed to {} while the proposal was pending; the stale switch to {target_mode} was not applied.",
                            latest.mode_state.mode
                        ),
                    ));
                }

                let mut next = latest.mode_state.clone();
                next.mode = target_mode.clone();
                next.router_reason = Some(reason);
                next.override_source = Some("model_suggestion".to_string());
                next.updated_at = Utc::now();
                let (updated, changed) = commit_mode_state_parts(
                    &self.store,
                    &self.persona,
                    (self.sender_for)(session_id),
                    latest,
                    next,
                )
                .await
                .map_err(host_error)?;
                Ok(Self::outcome(
                    "approved",
                    &updated.mode_state.mode,
                    target,
                    changed,
                    format!("User confirmed the switch to {target_mode}."),
                ))
            }
            ApprovalStatus::Denied => Ok(Self::outcome(
                "denied",
                &current_mode,
                target,
                false,
                format!("User declined the switch; staying in {current_mode}."),
            )),
            ApprovalStatus::TimedOut => Ok(Self::outcome(
                "timed_out",
                &current_mode,
                target,
                false,
                format!("No response to the switch proposal; staying in {current_mode}."),
            )),
            ApprovalStatus::Cancelled => Ok(Self::outcome(
                "cancelled",
                &current_mode,
                target,
                false,
                format!("The switch proposal was cancelled; staying in {current_mode}."),
            )),
        }
    }
}

fn host_error(error: impl std::fmt::Display) -> HostError {
    HostError::HostCall(error.to_string())
}

fn mode_suggest_docs(known_modes: Vec<String>) -> ToolDocs {
    ToolDocs {
        name: MODE_SUGGEST_CAPABILITY.to_string(),
        namespace: "modes".to_string(),
        summary: "Propose an approval-backed session mode switch".to_string(),
        description: Some(
            "Ask the owner to confirm a switch to another capability mode. The result reports the authoritative outcome; never assume a switch occurred before it returns approved. Available only in unlocked normal chat turns."
                .to_string(),
        ),
        signature:
            "@modes.suggest {targetMode, reason?} -> ModeSuggestionOutcome"
                .to_string(),
        args_schema: json!({
            "type": "object",
            "required": ["targetMode"],
            "additionalProperties": false,
            "properties": {
                "targetMode": {
                    "type": "string",
                    "enum": known_modes,
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9_.:-]+$"
                },
                "reason": { "type": "string", "maxLength": 2048 }
            }
        }),
        result_schema: Some(json!({
            "type": "object",
            "required": ["status", "currentMode", "targetMode", "changed", "message"],
            "properties": {
                "status": { "type": "string" },
                "currentMode": { "type": "string" },
                "targetMode": { "type": "string" },
                "changed": { "type": "boolean" },
                "message": { "type": "string" }
            }
        })),
        examples: vec![ToolExample {
            title: Some("Ask to enter Serious Engineer".to_string()),
            code: "let outcome = @modes.suggest {targetMode: \"serious_engineer\", reason: \"This requires repository writes\"};\noutcome |> display {kind: \"json\"}".to_string(),
            notes: Some("Continue using the current mode unless outcome.status is approved.".to_string()),
        }],
        errors: vec![
            ToolErrorDoc {
                name: "CapabilityDeniedError".to_string(),
                when: "The turn is locked, is using a coding backend, is an actor/scheduler turn, or otherwise lacks modes.suggest.".to_string(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "InvalidArgsError".to_string(),
                when: "targetMode or reason is malformed.".to_string(),
                retryable: false,
            },
            ToolErrorDoc {
                name: "HostCallError".to_string(),
                when: "The authoritative session, approval, or mode store is unavailable.".to_string(),
                retryable: true,
            },
        ],
        grants: vec![GrantDoc {
            kind: "capability".to_string(),
            description: "Requires the exact modes.suggest turn grant; handler registration alone grants no authority.".to_string(),
        }],
        sensitive: false,
        approval: "always".to_string(),
        since: "P5-hardening".to_string(),
        stability: "experimental".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApprovalResolveDecision, EchoChatRunner, InMemoryStore, NewSession, ResolveApprovalRequest,
        StoreMemoryProvider,
    };
    use tm_host::{CapabilityGrants, HostRegistry};

    type TestState = AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner>;

    fn test_state() -> TestState {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        AppState::new(
            store,
            memory,
            Arc::new(EchoChatRunner),
            ModesConfig::default(),
            AuthConfig::NoAuth,
        )
    }

    async fn new_general_session(state: &TestState) -> Uuid {
        let persona_status = state.persona.load_assets().status;
        state
            .store
            .create_session(NewSession {
                mode: ModeId::from("general"),
                persona_status,
            })
            .await
            .unwrap()
            .id
    }

    fn registry(state: &TestState, timeout: Duration) -> HostRegistry {
        let mut registry = HostRegistry::new();
        registry.register(Arc::new(ModeSuggestHostFn::new(state, timeout)));
        registry
    }

    fn ctx(session_id: Uuid, granted: bool) -> InvocationCtx {
        let grants = if granted {
            CapabilityGrants::default().allow(MODE_SUGGEST_CAPABILITY)
        } else {
            CapabilityGrants::default()
        };
        InvocationCtx::new(grants).with_session_id(session_id.to_string())
    }

    async fn wait_for_approval_id(state: &TestState, session_id: Uuid) -> Uuid {
        for _ in 0..200 {
            let events = state.store.events_after(session_id, None).await.unwrap();
            if let Some(event) = events.iter().find(|event| event.event_type == "approval") {
                return serde_json::from_value(event.payload_json["approvalId"].clone()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("approval event was not persisted")
    }

    async fn resolve(
        state: &TestState,
        session_id: Uuid,
        approval_id: Uuid,
        decision: ApprovalResolveDecision,
    ) {
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        state
            .approval_broker
            .resolve_persisted(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision,
                    option_id: None,
                },
                sink,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ungranted_handler_is_denied_before_it_runs() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let error = registry(&state, Duration::from_secs(1))
            .invoke(
                MODE_SUGGEST_CAPABILITY,
                json!({"targetMode": "serious_engineer"}),
                &ctx(session_id, false),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            HostError::CapabilityDenied(MODE_SUGGEST_CAPABILITY.to_string())
        );
        assert!(
            state
                .store
                .events_after(session_id, None)
                .await
                .unwrap()
                .iter()
                .all(|event| event.event_type != "approval")
        );
    }

    #[tokio::test]
    async fn approved_suggestion_is_sticky_and_replayable() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let registry = registry(&state, Duration::from_secs(5));
        let invocation = ctx(session_id, true);
        let handle = tokio::spawn(async move {
            registry
                .invoke(
                    MODE_SUGGEST_CAPABILITY,
                    json!({
                        "targetMode": "serious_engineer",
                        "reason": "token=provider-secret-123"
                    }),
                    &invocation,
                )
                .await
                .unwrap()
        });

        let approval_id = wait_for_approval_id(&state, session_id).await;
        resolve(
            &state,
            session_id,
            approval_id,
            ApprovalResolveDecision::Approve,
        )
        .await;

        let outcome = handle.await.unwrap();
        assert_eq!(outcome["status"], "approved");
        assert_eq!(outcome["changed"], true);
        let session = state.store.get_session(session_id).await.unwrap();
        assert_eq!(session.mode_state.mode, ModeId::from("serious_engineer"));
        assert_eq!(
            session.mode_state.override_source.as_deref(),
            Some("model_suggestion")
        );
        assert_eq!(
            session.mode_state.router_reason.as_deref(),
            Some("token=[REDACTED_SECRET]")
        );
        let events = state.store.events_after(session_id, None).await.unwrap();
        assert!(events.iter().any(|event| event.event_type == "approval"));
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "approval_resolved")
        );
        assert!(events.iter().any(|event| {
            event.event_type == "mode" && event.payload_json["mode"] == json!("serious_engineer")
        }));
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("provider-secret-123")
        );
    }

    #[tokio::test]
    async fn lock_added_while_pending_invalidates_an_approval() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let registry = registry(&state, Duration::from_secs(5));
        let invocation = ctx(session_id, true);
        let handle = tokio::spawn(async move {
            registry
                .invoke(
                    MODE_SUGGEST_CAPABILITY,
                    json!({"targetMode": "serious_engineer"}),
                    &invocation,
                )
                .await
                .unwrap()
        });

        let approval_id = wait_for_approval_id(&state, session_id).await;
        let session = state.store.get_session(session_id).await.unwrap();
        let mut locked = session.mode_state.clone();
        locked.lock_source = Some("user".to_string());
        state
            .store
            .set_mode_state(session_id, locked)
            .await
            .unwrap();
        resolve(
            &state,
            session_id,
            approval_id,
            ApprovalResolveDecision::Approve,
        )
        .await;

        let outcome = handle.await.unwrap();
        assert_eq!(outcome["status"], "locked");
        assert_eq!(outcome["changed"], false);
        assert_eq!(
            state
                .store
                .get_session(session_id)
                .await
                .unwrap()
                .mode_state
                .mode,
            ModeId::from("general")
        );
    }

    #[tokio::test]
    async fn invalid_and_same_target_do_not_prompt() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let registry = registry(&state, Duration::from_secs(1));
        let invocation = ctx(session_id, true);

        let invalid = registry
            .invoke(
                MODE_SUGGEST_CAPABILITY,
                json!({"targetMode": "not_a_mode"}),
                &invocation,
            )
            .await
            .unwrap();
        assert_eq!(invalid["status"], "invalid_target");
        let same = registry
            .invoke(
                MODE_SUGGEST_CAPABILITY,
                json!({"targetMode": "general"}),
                &invocation,
            )
            .await
            .unwrap();
        assert_eq!(same["status"], "already_active");
        assert!(
            state
                .store
                .events_after(session_id, None)
                .await
                .unwrap()
                .iter()
                .all(|event| event.event_type != "approval")
        );
    }
}
