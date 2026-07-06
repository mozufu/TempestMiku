//! Lets the chat model propose a mode transition mid-turn instead of the old silent keyword
//! auto-switch (§21.4). The model calls the `mode_suggest(target_mode, reason)` tool; that
//! surfaces as a normal approval event (reusing [`crate::ApprovalBroker`], the same
//! propose/confirm primitive `run_memory_write_proposal` uses) and only applies the switch —
//! via [`commit_mode_state`] — once the user confirms. Declining or timing out leaves the
//! session in its current mode; either way the model gets the outcome back as its tool result
//! and keeps the turn going coherently.
//!
//! Wired only on the [`ChatRunner`] path (`post_message`'s non-native-coding-backend turns);
//! the native coding backend has no mediator seam in v1 (see the doc comment on
//! `post_message`'s `mode_suggest_mediator` closure).

use async_trait::async_trait;
use tm_core::{FunctionSpec, ToolMediator, ToolSpec};

use super::modes::{commit_mode_state, validate_mode};
use super::*;

/// How long a `mode_suggest` confirmation waits for the user before treating it as declined.
pub(super) const MODE_SUGGEST_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct ModeSuggestMediator<S, M, C> {
    state: AppState<S, M, C>,
    session_id: Uuid,
    sink: Arc<dyn CodingEventSink>,
    timeout: Duration,
}

impl<S, M, C> ModeSuggestMediator<S, M, C> {
    pub(super) fn new(
        state: AppState<S, M, C>,
        session_id: Uuid,
        sink: Arc<dyn CodingEventSink>,
        timeout: Duration,
    ) -> Self {
        Self {
            state,
            session_id,
            sink,
            timeout,
        }
    }
}

fn mediator_error(err: impl std::fmt::Display) -> tm_core::Error {
    tm_core::Error::Sandbox(err.to_string())
}

#[async_trait]
impl<S, M, C> ToolMediator for ModeSuggestMediator<S, M, C>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    fn tool_specs(&self) -> Vec<ToolSpec> {
        let known_modes: Vec<String> = self
            .state
            .persona
            .load_assets()
            .modes
            .modes
            .iter()
            .map(|profile| profile.mode.as_str().to_string())
            .collect();
        vec![ToolSpec {
            kind: "function".to_string(),
            function: FunctionSpec {
                name: "mode_suggest".to_string(),
                description: "Propose switching the session's capability mode (e.g. into \
                    Serious Engineer for real repo work, or Handoff to delegate). The user must \
                    confirm before the switch takes effect — you are not authorized to assume \
                    it happened. You'll get the outcome back as this call's result; continue the \
                    conversation coherently whether it was confirmed, declined, or timed out."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target_mode": {
                            "type": "string",
                            "enum": known_modes,
                            "description": "The mode id to switch to."
                        },
                        "reason": {
                            "type": "string",
                            "description": "Why this switch is needed; shown to the user."
                        }
                    },
                    "required": ["target_mode"]
                }),
            },
        }]
    }

    async fn handle(&self, name: &str, arguments: &Value) -> tm_core::Result<String> {
        if name != "mode_suggest" {
            return Ok(format!("{name} is not a recognized tool."));
        }

        let Some(target) = arguments.get("target_mode").and_then(Value::as_str) else {
            return Ok("mode_suggest requires a target_mode argument.".to_string());
        };
        let reason = arguments
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("model-proposed mode switch")
            .to_string();

        // Always re-load fresh: never act on a mode_state captured at turn start.
        let session = self
            .state
            .store
            .get_session(self.session_id)
            .await
            .map_err(mediator_error)?;

        if session.mode_state.lock_source.is_some() {
            return Ok(format!(
                "Mode is locked by the user; the switch to {target} was not offered."
            ));
        }

        let target_mode = match validate_mode(&self.state.persona, ModeId::from(target)) {
            Ok(mode) => mode,
            Err(_) => {
                return Ok(format!(
                    "{target} is not a known mode; the switch was not offered."
                ));
            }
        };

        if target_mode == session.mode_state.mode {
            return Ok(format!("Already in {target_mode}."));
        }

        let profile = mode_profile(&self.state.persona, &target_mode);
        let prompt = ApprovalPrompt {
            action: format!("Switch to {} mode", profile.label),
            scope: json!({
                "targetMode": target_mode.as_str(),
                "reason": reason,
                "currentMode": session.mode_state.mode.as_str(),
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

        let approval = self
            .state
            .approval_broker
            .request_permission_detailed_for_backend(
                self.session_id,
                "mode",
                prompt,
                self.timeout,
                Arc::clone(&self.sink),
            )
            .await
            .map_err(mediator_error)?;

        let current_mode = session.mode_state.mode.clone();
        match approval.status {
            ApprovalStatus::Approved => {
                let mut next = session.mode_state.clone();
                next.mode = target_mode.clone();
                next.router_reason = Some(reason);
                // Marks this as a deliberate, user-confirmed switch — distinct from a raw
                // client `override`, but sticky the same way: nothing auto-reverts it.
                next.override_source = Some("model_suggestion".to_string());
                next.updated_at = Utc::now();
                commit_mode_state(&self.state, session, next)
                    .await
                    .map_err(mediator_error)?;
                Ok(format!("User confirmed the switch to {target_mode}."))
            }
            ApprovalStatus::Denied => Ok(format!(
                "User declined the switch; staying in {current_mode}."
            )),
            ApprovalStatus::TimedOut | ApprovalStatus::Cancelled => Ok(format!(
                "No response to the switch proposal; staying in {current_mode}."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApprovalResolveDecision, EchoChatRunner, InMemoryStore, NewSession, ResolveApprovalRequest,
        StoreMemoryProvider,
    };

    type TestState = AppState<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner>;

    fn test_state() -> TestState {
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        AppState::new(
            store,
            memory,
            chat,
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

    fn mediator(
        state: &TestState,
        session_id: Uuid,
        timeout: Duration,
    ) -> ModeSuggestMediator<InMemoryStore, StoreMemoryProvider<InMemoryStore>, EchoChatRunner>
    {
        let sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        ModeSuggestMediator::new(state.clone(), session_id, sink, timeout)
    }

    async fn wait_for_approval_id(state: &TestState, session_id: Uuid) -> Uuid {
        for _ in 0..200 {
            let events = state.store.events_after(session_id, None).await.unwrap();
            if let Some(event) = events.iter().find(|e| e.event_type == "approval") {
                return serde_json::from_value(event.payload_json["approvalId"].clone()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("approval event was not persisted")
    }

    #[tokio::test]
    async fn approve_applies_the_switch_and_emits_mode_changed() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let m = mediator(&state, session_id, Duration::from_secs(5));

        let handle = tokio::spawn(async move {
            m.handle(
                "mode_suggest",
                &json!({"target_mode": "serious_engineer", "reason": "fix a bug"}),
            )
            .await
            .unwrap()
        });

        let approval_id = wait_for_approval_id(&state, session_id).await;
        state
            .approval_broker
            .resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Approve,
                    option_id: None,
                },
            )
            .unwrap();

        let result = handle.await.unwrap();
        assert!(
            result.contains("confirmed the switch to serious_engineer"),
            "got: {result}"
        );

        let session = state.store.get_session(session_id).await.unwrap();
        assert_eq!(session.mode_state.mode, ModeId::from("serious_engineer"));
        assert_eq!(
            session.mode_state.override_source.as_deref(),
            Some("model_suggestion")
        );

        let events = state.store.events_after(session_id, None).await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.event_type == "mode"
                    && e.payload_json["mode"] == json!("serious_engineer"))
        );
    }

    #[tokio::test]
    async fn deny_leaves_the_mode_unchanged() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let m = mediator(&state, session_id, Duration::from_secs(5));

        let handle = tokio::spawn(async move {
            m.handle(
                "mode_suggest",
                &json!({"target_mode": "serious_engineer", "reason": "fix a bug"}),
            )
            .await
            .unwrap()
        });

        let approval_id = wait_for_approval_id(&state, session_id).await;
        state
            .approval_broker
            .resolve(
                session_id,
                approval_id,
                ResolveApprovalRequest {
                    decision: ApprovalResolveDecision::Deny,
                    option_id: None,
                },
            )
            .unwrap();

        let result = handle.await.unwrap();
        assert!(result.contains("declined"), "got: {result}");

        let session = state.store.get_session(session_id).await.unwrap();
        assert_eq!(session.mode_state.mode, ModeId::from("general"));
    }

    #[tokio::test]
    async fn timeout_leaves_the_mode_unchanged() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let m = mediator(&state, session_id, Duration::from_millis(5));

        let result = m
            .handle("mode_suggest", &json!({"target_mode": "serious_engineer"}))
            .await
            .unwrap();
        assert!(result.contains("No response"), "got: {result}");

        let session = state.store.get_session(session_id).await.unwrap();
        assert_eq!(session.mode_state.mode, ModeId::from("general"));
    }

    #[tokio::test]
    async fn locked_mode_suppresses_the_proposal_without_an_approval_event() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let session = state.store.get_session(session_id).await.unwrap();
        let mut locked = session.mode_state.clone();
        locked.lock_source = Some("user".to_string());
        state
            .store
            .set_mode_state(session_id, locked)
            .await
            .unwrap();

        let m = mediator(&state, session_id, Duration::from_secs(5));
        let result = m
            .handle("mode_suggest", &json!({"target_mode": "serious_engineer"}))
            .await
            .unwrap();
        assert!(result.contains("locked"), "got: {result}");

        let events = state.store.events_after(session_id, None).await.unwrap();
        assert!(!events.iter().any(|e| e.event_type == "approval"));
        let session = state.store.get_session(session_id).await.unwrap();
        assert_eq!(session.mode_state.mode, ModeId::from("general"));
    }

    #[tokio::test]
    async fn unknown_target_mode_is_reported_without_an_approval_event() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let m = mediator(&state, session_id, Duration::from_secs(5));

        let result = m
            .handle("mode_suggest", &json!({"target_mode": "not_a_real_mode"}))
            .await
            .unwrap();
        assert!(result.contains("not a known mode"), "got: {result}");

        let events = state.store.events_after(session_id, None).await.unwrap();
        assert!(!events.iter().any(|e| e.event_type == "approval"));
    }

    #[tokio::test]
    async fn already_in_target_mode_is_a_no_op() {
        let state = test_state();
        let session_id = new_general_session(&state).await;
        let m = mediator(&state, session_id, Duration::from_secs(5));

        let result = m
            .handle("mode_suggest", &json!({"target_mode": "general"}))
            .await
            .unwrap();
        assert!(result.contains("Already in"), "got: {result}");

        let events = state.store.events_after(session_id, None).await.unwrap();
        assert!(!events.iter().any(|e| e.event_type == "approval"));
    }
}
