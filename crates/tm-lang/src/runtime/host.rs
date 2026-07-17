use super::*;
pub(super) struct TracingApproval {
    pub(super) inner: Arc<dyn ApprovalPolicy>,
    pub(super) events: Arc<dyn HostEventSink>,
    pub(super) cell_id: String,
    pub(super) node_id: String,
    pub(super) machine: Arc<Mutex<EffectMachine>>,
    pub(super) sensitive: bool,
    pub(super) preview_bytes: usize,
    pub(super) output_used: Arc<AtomicUsize>,
    pub(super) output_bytes: usize,
    pub(super) parent_node_id: Option<String>,
    pub(super) event_failure: Arc<Mutex<Option<String>>>,
}

impl TracingApproval {
    async fn emit_runtime_event(&self, event: &str, payload: JsonValue) -> tm_host::Result<()> {
        match self.events.emit(event, payload).await {
            Ok(()) => Ok(()),
            Err(error) => {
                *self
                    .event_failure
                    .lock()
                    .expect("event failure lock poisoned") =
                    Some(bounded_display(&error, 64 * 1024));
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl ApprovalPolicy for TracingApproval {
    async fn request(
        &self,
        action: &str,
        timeout: std::time::Duration,
    ) -> tm_host::Result<ApprovalDecision> {
        let token = self
            .machine
            .lock()
            .expect("effect machine lock poisoned")
            .suspend()
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        let action_preview = if self.sensitive {
            "[redacted]".to_string()
        } else {
            bounded(action, self.preview_bytes)
        };
        let remaining = self.output_bytes.saturating_sub(
            self.output_used
                .load(AtomicOrdering::Relaxed)
                .min(self.output_bytes),
        );
        let encoded_bytes = json_string_encoded_len_bounded(&action_preview, remaining)
            .ok_or_else(|| HostError::HostCall("effect/output budget exceeded".into()))?;
        self.output_used
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(encoded_bytes)
                    .filter(|next| *next <= self.output_bytes)
            })
            .map_err(|_| HostError::HostCall("effect/output budget exceeded".into()))?;
        self.emit_runtime_event(
                "effect_suspended",
                json!({"cellId": self.cell_id, "nodeId": self.node_id, "parentNodeId": self.parent_node_id, "action": action_preview}),
            )
            .await?;
        let decision = self.inner.request(action, timeout).await;
        match decision {
            Ok(decision) => {
                self.machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .resume(&token)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                let decision_name = match decision {
                    ApprovalDecision::Approved => "approved",
                    ApprovalDecision::Denied => "denied",
                };
                self.emit_runtime_event("effect_resumed", json!({"cellId": self.cell_id, "nodeId": self.node_id, "parentNodeId": self.parent_node_id, "decision": decision_name})).await?;
                Ok(decision)
            }
            Err(error) => {
                self.machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .fail()
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                Err(error)
            }
        }
    }
}

pub(super) fn host_error(error: HostError) -> RuntimeError {
    const MAX_HOST_ERROR_BYTES: usize = 64 * 1024;
    let retained_bytes = match &error {
        HostError::UnknownScheme { scheme, registered } => {
            registered.iter().fold(scheme.len(), |bytes, value| {
                bytes.saturating_add(value.len())
            })
        }
        HostError::CapabilityDenied(value)
        | HostError::ApprovalDenied(value)
        | HostError::ApprovalTimeout(value)
        | HostError::NotFound(value)
        | HostError::InvalidArgs(value)
        | HostError::InvalidPath(value)
        | HostError::NotImplemented(value)
        | HostError::QuotaExceeded(value)
        | HostError::Timeout(value)
        | HostError::OutputTruncated(value)
        | HostError::HostCall(value) => value.len(),
    };
    let message = bounded_display(&error, MAX_HOST_ERROR_BYTES);
    let payload = if retained_bytes <= MAX_HOST_ERROR_BYTES {
        serde_json::to_value(error.to_payload())
            .expect("HostErrorPayload serialization cannot fail")
    } else {
        json!({"name": error.sdk_name(), "message": message.clone(), "detailsTruncated": true})
    };
    RuntimeError::Effect {
        name: error.sdk_name().into(),
        message,
        payload: Some(payload),
    }
}

pub(super) fn runtime_event_error(error: HostError) -> RuntimeError {
    RuntimeError::Persistence(bounded_display(&error, 64 * 1024))
}

pub(super) fn runtime_error_payload(
    error: &RuntimeError,
    max_bytes: usize,
    max_depth: usize,
) -> Value {
    match error {
        RuntimeError::Effect {
            payload: Some(payload),
            ..
        } if json_value_size_bounded(payload, max_bytes, max_depth) <= max_bytes
            && json_encoded_len_bounded(payload, max_bytes).is_some() =>
        {
            Value::from_json(payload.clone())
        }
        _ => Value::Record(BTreeMap::from([(
            "message".into(),
            Value::String(bounded_display(error, max_bytes)),
        )])),
    }
}

pub(super) fn rethrow_error(value: &Value, preview_bytes: usize, max_depth: usize) -> RuntimeError {
    let Value::Tagged { name, payload } = value else {
        return RuntimeError::Effect {
            name: "Rethrown".into(),
            message: render_value_bounded(value, preview_bytes, max_depth)
                .unwrap_or_else(|_| "[rethrow payload exceeded budget]".into()),
            payload: None,
        };
    };
    let payload_value = payload.as_deref();
    let message = match payload_value {
        Some(Value::Record(fields)) => fields
            .get("message")
            .and_then(|value| match value {
                Value::String(message) => Some(bounded(message, preview_bytes)),
                _ => None,
            })
            .unwrap_or_else(|| {
                payload_value
                    .and_then(|payload| {
                        render_value_bounded(payload, preview_bytes, max_depth).ok()
                    })
                    .unwrap_or_else(|| "[rethrow payload exceeded budget]".into())
            }),
        Some(payload) => render_value_bounded(payload, preview_bytes, max_depth)
            .unwrap_or_else(|_| "[rethrow payload exceeded budget]".into()),
        None => bounded(name, preview_bytes),
    };
    let payload = payload_value.and_then(|payload| {
        (value_size_bounded(payload, preview_bytes, max_depth) <= preview_bytes)
            .then(|| value_json_bounded(payload, preview_bytes, false, max_depth))
            .flatten()
            .map(|_| payload.to_json())
    });
    RuntimeError::Effect {
        name: name.clone(),
        message,
        payload,
    }
}

pub(super) fn bounded(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        value.into()
    } else {
        let marker = if bytes >= 3 { "..." } else { "" };
        let mut end = bytes.saturating_sub(marker.len()).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{marker}", &value[..end])
    }
}
