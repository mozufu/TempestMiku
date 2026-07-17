use super::*;

impl Evaluator {
    pub(super) async fn perform(&mut self, name: &str, argument: Value) -> RuntimeResult<Value> {
        let node_index = self.node_counter.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let node_id = format!("{}-node-{node_index}", self.cell_id);
        let args = normalize_effect_args(name, argument);
        if json_value_size_bounded(&args, self.limits.value_bytes, self.limits.runtime_depth)
            > self.limits.value_bytes
            || json_encoded_len_bounded(&args, self.limits.value_bytes).is_none()
        {
            return Err(RuntimeError::Limit(
                "effect argument value budget exceeded".into(),
            ));
        }
        let sensitive = self.catalog.effect(name).is_some();
        let preview = if sensitive {
            "[redacted]".to_string()
        } else {
            json_preview_bounded(&args, self.limits.preview_bytes)
        };
        self.reserve_preview(&preview)
            .map_err(|_| RuntimeError::Limit("effect/output budget exceeded".into()))?;
        let mut ctx = self.invocation.clone();
        let machine = Arc::new(Mutex::new(EffectMachine::new(
            self.cell_id.clone(),
            node_id.clone(),
            ctx.session_id.clone(),
        )));
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.effects.insert(
                node_id.clone(),
                ActiveEffect {
                    scope_id: self.scope_id.clone(),
                    machine: Arc::clone(&machine),
                    terminal: None,
                },
            );
        }
        if let Err(error) = self.emit("effect_start", json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "capability": name, "argsPreview": preview})).await {
            self.clear_active_effect(&node_id);
            return Err(error);
        }
        let approval_event_failure = Arc::new(Mutex::new(None));
        ctx.approvals = Arc::new(TracingApproval {
            inner: Arc::clone(&ctx.approvals),
            events: Arc::clone(&ctx.events),
            cell_id: self.cell_id.clone(),
            node_id: node_id.clone(),
            machine: Arc::clone(&machine),
            sensitive,
            preview_bytes: self.limits.preview_bytes,
            output_used: Arc::clone(&self.output_used),
            output_bytes: self.output_bytes,
            parent_node_id: self.scope_id.clone(),
            event_failure: Arc::clone(&approval_event_failure),
        });
        let invocation = self.registry.invoke(name, args, &ctx).await;
        let approval_event_error = approval_event_failure
            .lock()
            .expect("event failure lock poisoned")
            .take();
        if let Some(error) = approval_event_error {
            let _ = machine.lock().expect("effect machine lock poisoned").fail();
            return Err(RuntimeError::Persistence(error));
        }
        match invocation {
            Ok(value) => {
                if json_value_size_bounded(
                    &value,
                    self.limits.value_bytes,
                    self.limits.runtime_depth,
                ) > self.limits.value_bytes
                    || json_encoded_len_bounded(&value, self.limits.value_bytes).is_none()
                {
                    let _ = machine.lock().expect("effect machine lock poisoned").fail();
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit(
                        "effect result value budget exceeded".into(),
                    ));
                }
                let result_preview = if sensitive {
                    "[redacted]".to_string()
                } else {
                    json_preview_bounded(&value, self.limits.preview_bytes)
                };
                if self.reserve_preview(&result_preview).is_err() {
                    let _ = machine.lock().expect("effect machine lock poisoned").fail();
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit("effect/output budget exceeded".into()));
                }
                machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .complete()
                    .map_err(|error| RuntimeError::Effect {
                        name: "EffectStateError".into(),
                        message: error.to_string(),
                        payload: None,
                    })?;
                self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "completed", "resultPreview": result_preview})).await?;
                let value = Value::from_json(value);
                self.ensure_value(&value)?;
                Ok(value)
            }
            Err(error) => {
                let _ = machine.lock().expect("effect machine lock poisoned").fail();
                let error_preview = if sensitive {
                    "[redacted]".to_string()
                } else {
                    bounded_display(&error, self.limits.preview_bytes)
                };
                if self.reserve_preview(&error_preview).is_err() {
                    self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": "[redacted]"})).await?;
                    return Err(RuntimeError::Limit("effect/output budget exceeded".into()));
                }
                self.emit_effect_terminal(&node_id, json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": self.scope_id, "status": "failed", "error": error_preview})).await?;
                Err(host_error(error))
            }
        }
    }

    pub(super) fn clear_active_effect(&self, node_id: &str) {
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.effects.remove(node_id);
        }
    }

    pub(super) async fn emit_effect_terminal(
        &self,
        node_id: &str,
        payload: JsonValue,
    ) -> RuntimeResult<()> {
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
            && let Some(effect) = active.effects.get_mut(node_id)
        {
            effect.terminal = Some(payload.clone());
        }
        self.emit("effect_result", payload).await?;
        self.clear_active_effect(node_id);
        Ok(())
    }

    pub(super) fn interpolate<'a>(
        &'a mut self,
        source: &'a str,
    ) -> LocalBoxFuture<'a, RuntimeResult<String>> {
        Box::pin(async move {
            let mut output = String::new();
            let chars: Vec<char> = source.chars().collect();
            let mut index = 0;
            while index < chars.len() {
                if chars[index] == '\\' && chars.get(index + 1) == Some(&'#') {
                    output.push('#');
                    index += 2;
                    continue;
                }
                if chars[index] != '#' {
                    output.push(chars[index]);
                    index += 1;
                    continue;
                }
                if chars.get(index + 1) == Some(&'{') {
                    let start = index + 2;
                    let mut depth = 1;
                    let mut end = start;
                    while end < chars.len() && depth > 0 {
                        match chars[end] {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            end += 1;
                        }
                    }
                    if depth != 0 {
                        return Err(RuntimeError::Type(
                            "unterminated string interpolation".into(),
                        ));
                    }
                    let fragment: String = chars[start..end].iter().collect();
                    let cell = parse_bounded(
                        &fragment,
                        self.limits.source_bytes,
                        self.limits.syntax_nodes,
                        self.limits.parse_depth,
                    )?;
                    let [
                        Form {
                            node: FormKind::Expr(expr),
                            ..
                        },
                    ] = cell.forms.as_slice()
                    else {
                        return Err(RuntimeError::Type(
                            "interpolation must contain one expression".into(),
                        ));
                    };
                    let value = self.expr(expr).await?;
                    let rendered = render_value_bounded(
                        &value,
                        self.limits.value_bytes.saturating_sub(output.len()),
                        self.limits.runtime_depth,
                    )?;
                    self.push_interpolation(&mut output, &rendered)?;
                    index = end + 1;
                    continue;
                }
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                    end += 1;
                }
                if end == start {
                    output.push('#');
                    index += 1;
                    continue;
                }
                let name: String = chars[start..end].iter().collect();
                let value = self.env.get(&name).ok_or_else(|| {
                    RuntimeError::Type(format!("unbound interpolation name {name}"))
                })?;
                let rendered = render_value_bounded(
                    value,
                    self.limits.value_bytes.saturating_sub(output.len()),
                    self.limits.runtime_depth,
                )?;
                self.push_interpolation(&mut output, &rendered)?;
                index = end;
            }
            Ok(output)
        })
    }

    pub(super) async fn emit(&self, event: &str, payload: JsonValue) -> RuntimeResult<()> {
        self.invocation
            .events
            .emit(event, payload)
            .await
            .map_err(runtime_event_error)
    }

    pub(super) async fn emit_cell_terminal(&self, payload: JsonValue) -> RuntimeResult<()> {
        self.terminal_selected.store(true, AtomicOrdering::Release);
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.cell_terminal = Some(payload.clone());
        }
        let result = self.emit("cell_result", payload).await;
        *self.active.lock().expect("active execution lock poisoned") = None;
        result
    }

    pub(super) async fn terminal_error(
        &self,
        payload: JsonValue,
        error: RuntimeError,
    ) -> RuntimeError {
        match self.emit_cell_terminal(payload).await {
            Ok(()) => error,
            Err(persistence) => persistence,
        }
    }
}
