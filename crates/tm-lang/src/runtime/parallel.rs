use super::*;

impl Evaluator {
    pub(super) async fn parallel_expr(&mut self, expr: &Expr) -> RuntimeResult<Value> {
        let total = match &expr.node {
            ExprKind::Record(fields) => fields.len(),
            _ => 1,
        };
        let node = self.start_scope("par", Some(total)).await?;
        let result = if let ExprKind::Record(fields) = &expr.node {
            let work = fields
                .iter()
                .map(|(name, expr)| (name.clone(), expr.clone()))
                .collect::<Vec<_>>();
            self.run_parallel(work, &node, |mut child, expr| async move {
                let result = child.expr(&expr).await;
                (result, child)
            })
            .await
            .map(|values| Value::Record(values.into_iter().collect()))
            .inspect(|_| {
                debug_assert_eq!(total, fields.len());
            })
        } else {
            let mut child = self.fork_child(&node);
            child.expr(expr).await
        };
        let cancelled = if result.is_err() {
            self.cancel_scope_effects(&node, "parallel sibling failed")
                .await?
        } else {
            0
        };
        self.finish_scope(&node, result.as_ref().err(), cancelled)
            .await?;
        result
    }

    pub(super) async fn parallel(&mut self, value: Value) -> RuntimeResult<Value> {
        let node = self.start_scope("par", None).await?;
        self.finish_scope(&node, None, 0).await?;
        Ok(value)
    }

    pub(super) async fn parallel_map(&mut self, args: Vec<Value>) -> RuntimeResult<Value> {
        let (function, values) = list_call_args(args, "par map")?;
        let total = values.len();
        let node = self.start_scope("par_map", Some(total)).await?;
        let work = values.into_iter().enumerate().collect::<Vec<_>>();
        let result = self
            .run_parallel(work, &node, move |mut child, value| {
                let function = function.clone();
                async move {
                    let result = child.apply(function, value).await;
                    (result, child)
                }
            })
            .await
            .map(|values| Value::List(values.into_iter().map(|(_, value)| value).collect()));
        let cancelled = if result.is_err() {
            self.cancel_scope_effects(&node, "parallel sibling failed")
                .await?
        } else {
            0
        };
        self.finish_scope(&node, result.as_ref().err(), cancelled)
            .await?;
        result
    }

    pub(super) async fn run_parallel<K, T, F, Fut>(
        &mut self,
        work: Vec<(K, T)>,
        scope: &str,
        mut evaluate: F,
    ) -> RuntimeResult<Vec<(K, Value)>>
    where
        K: Clone + Ord + 'static,
        F: FnMut(Evaluator, T) -> Fut,
        Fut: std::future::Future<Output = (RuntimeResult<Value>, Evaluator)> + 'static,
    {
        let total = work.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let parallelism = self.limits.parallelism.max(1).min(total);
        let mut remaining = work.into_iter();
        let mut pending: FuturesUnordered<
            LocalBoxFuture<'static, (K, RuntimeResult<Value>, Evaluator)>,
        > = FuturesUnordered::new();
        for _ in 0..parallelism {
            let Some((key, item)) = remaining.next() else {
                break;
            };
            let child = self.fork_child(scope);
            let future = evaluate(child, item);
            pending.push(Box::pin(async move {
                let (result, child) = future.await;
                (key, result, child)
            }));
        }
        let mut completed = 0usize;
        let mut output = Vec::with_capacity(total);
        let mut allocated = 0;
        while let Some((key, result, _child)) = pending.next().await {
            match result {
                Ok(value) => {
                    completed += 1;
                    self.add_value_size(&mut allocated, &value, 0)?;
                    output.push((key, value));
                    if let Some(active) = self
                        .active
                        .lock()
                        .expect("active execution lock poisoned")
                        .as_mut()
                        && let Some(active_scope) = active.scopes.get_mut(scope)
                    {
                        active_scope.completed = completed;
                    }
                    self.emit(
                        "scope_progress",
                        json!({"cellId": self.cell_id, "nodeId": scope, "parentNodeId": self.scope_id, "completed": completed, "total": total}),
                    )
                    .await?;
                    if let Some((next_key, next_item)) = remaining.next() {
                        let child = self.fork_child(scope);
                        let future = evaluate(child, next_item);
                        pending.push(Box::pin(async move {
                            let (result, child) = future.await;
                            (next_key, result, child)
                        }));
                    }
                }
                Err(error) => {
                    drop(pending);
                    return Err(error);
                }
            }
        }
        output.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(output)
    }

    pub(super) fn fork_child(&self, scope: &str) -> Self {
        let mut child = self.clone();
        child.committed.clear();
        child.scope_id = Some(scope.to_string());
        child
    }

    pub(super) async fn start_scope(
        &mut self,
        kind: &str,
        total: Option<usize>,
    ) -> RuntimeResult<String> {
        let index = self.node_counter.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        let node = format!("{}-scope-{index}", self.cell_id);
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.scopes.insert(
                node.clone(),
                ActiveScope {
                    total,
                    completed: 0,
                    parent_node_id: self.scope_id.clone(),
                    terminal: None,
                },
            );
        }
        if let Err(error) = self
            .emit(
                "scope_start",
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "kind": kind, "total": total}),
            )
            .await
        {
            if let Some(active) = self
                .active
                .lock()
                .expect("active execution lock poisoned")
                .as_mut()
            {
                active.scopes.remove(&node);
            }
            return Err(error);
        }
        Ok(node)
    }

    pub(super) async fn finish_scope(
        &self,
        node: &str,
        error: Option<&RuntimeError>,
        cancelled_siblings: usize,
    ) -> RuntimeResult<()> {
        let payload = if let Some(error) = error {
            let error_preview = if self.sensitive_cell {
                "[redacted]".to_string()
            } else {
                bounded_display(error, self.limits.preview_bytes)
            };
            if self.reserve_preview(&error_preview).is_ok() {
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "failed", "error": error_preview, "cancelledSiblings": cancelled_siblings})
            } else {
                json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "failed", "errorTruncated": true, "cancelledSiblings": cancelled_siblings})
            }
        } else {
            json!({"cellId": self.cell_id, "nodeId": node, "parentNodeId": self.scope_id, "status": "completed"})
        };
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
            && let Some(scope) = active.scopes.get_mut(node)
        {
            scope.terminal = Some(payload.clone());
        }
        self.emit("scope_result", payload).await?;
        if let Some(active) = self
            .active
            .lock()
            .expect("active execution lock poisoned")
            .as_mut()
        {
            active.scopes.remove(node);
        }
        Ok(())
    }

    pub(super) async fn cancel_scope_effects(
        &self,
        scope: &str,
        _reason: &str,
    ) -> RuntimeResult<usize> {
        let (effects, mut descendant_scopes, cancelled_siblings) = {
            let mut active = self.active.lock().expect("active execution lock poisoned");
            let Some(active) = active.as_mut() else {
                return Ok(0);
            };
            let cancelled_siblings = active
                .scopes
                .get(scope)
                .and_then(|scope| {
                    scope
                        .total
                        .map(|total| total.saturating_sub(scope.completed).saturating_sub(1))
                })
                .unwrap_or(0);

            // A dropped outer branch can leave a nested `par` future suspended. Discover the
            // complete scope subtree before removing anything so every child effect and scope is
            // terminalized before the failed parent.
            let mut depths = BTreeMap::from([(scope.to_string(), 0usize)]);
            loop {
                let mut changed = false;
                for (node, child) in &active.scopes {
                    if depths.contains_key(node) {
                        continue;
                    }
                    let Some(parent) = child.parent_node_id.as_ref() else {
                        continue;
                    };
                    let Some(parent_depth) = depths.get(parent).copied() else {
                        continue;
                    };
                    depths.insert(node.clone(), parent_depth.saturating_add(1));
                    changed = true;
                }
                if !changed {
                    break;
                }
            }

            let effects = active
                .effects
                .iter()
                .filter(|(_, effect)| {
                    effect
                        .scope_id
                        .as_ref()
                        .is_some_and(|scope| depths.contains_key(scope))
                })
                .map(|(node, effect)| (node.clone(), effect.clone()))
                .collect::<Vec<_>>();
            for (node, _) in &effects {
                active.effects.remove(node);
            }

            let mut descendant_scopes = depths
                .into_iter()
                .filter(|(node, _)| node != scope)
                .filter_map(|(node, depth)| {
                    active
                        .scopes
                        .remove(&node)
                        .map(|active_scope| (node, active_scope, depth))
                })
                .collect::<Vec<_>>();
            descendant_scopes
                .sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
            (effects, descendant_scopes, cancelled_siblings)
        };
        for (node_id, effect) in &effects {
            let payload = if let Some(terminal) = &effect.terminal {
                terminal.clone()
            } else {
                let _ = effect
                    .machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .cancel();
                json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": effect.scope_id, "status": "cancelled", "error": "[redacted]"})
            };
            self.emit("effect_result", payload).await?;
        }
        for (node_id, child_scope, _) in descendant_scopes.drain(..) {
            let ActiveScope {
                parent_node_id,
                terminal,
                ..
            } = child_scope;
            let payload = terminal.unwrap_or_else(|| {
                json!({"cellId": self.cell_id, "nodeId": node_id, "parentNodeId": parent_node_id, "status": "cancelled", "error": "[redacted]"})
            });
            self.emit("scope_result", payload).await?;
        }
        Ok(cancelled_siblings)
    }
}
