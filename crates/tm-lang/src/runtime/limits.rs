use super::*;

impl Evaluator {
    pub(super) fn step(&mut self, _span: Span) -> RuntimeResult<()> {
        self.charge_work(1)
    }

    pub(super) fn enter_runtime_depth(&mut self) -> RuntimeResult<()> {
        if self.depth >= self.limits.runtime_depth {
            return Err(RuntimeError::Limit(
                "runtime nesting budget exceeded".into(),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    pub(super) fn leave_runtime_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(super) fn charge_work(&self, units: u64) -> RuntimeResult<()> {
        self.steps
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(units)
                    .filter(|next| *next <= self.limits.steps)
            })
            .map(|_| ())
            .map_err(|_| RuntimeError::Limit("step budget exceeded".into()))
    }

    pub(super) fn values_equal(&self, left: &Value, right: &Value) -> RuntimeResult<bool> {
        let remaining = self.limits.steps.saturating_sub(
            self.steps
                .load(AtomicOrdering::Relaxed)
                .min(self.limits.steps),
        );
        let Some((equal, visits)) = json_semantic_eq_bounded(left, right, remaining as usize)
        else {
            return Err(RuntimeError::Limit("step budget exceeded".into()));
        };
        self.charge_work(visits as u64)?;
        Ok(equal)
    }

    pub(super) fn match_pattern(
        &self,
        pattern: &Pattern,
        value: &Value,
    ) -> RuntimeResult<Option<Environment>> {
        let remaining = self.limits.steps.saturating_sub(
            self.steps
                .load(AtomicOrdering::Relaxed)
                .min(self.limits.steps),
        );
        let (bindings, visits) = match_pattern_counted(
            pattern,
            value,
            remaining as usize,
            self.limits.runtime_depth,
        );
        if visits > remaining as usize {
            return Err(RuntimeError::Limit("step budget exceeded".into()));
        }
        self.charge_work(visits as u64)?;
        Ok(bindings)
    }

    pub(super) async fn cooperate(&self, iterations: &mut u64) -> RuntimeResult<()> {
        self.charge_work(1)?;
        *iterations = iterations.saturating_add(1);
        if (*iterations).is_multiple_of(256) {
            tokio::task::yield_now().await;
        }
        Ok(())
    }
    pub(super) fn push_stdout(&mut self, text: &str) -> RuntimeResult<()> {
        let mut stdout = self.stdout.lock().expect("stdout lock poisoned");
        let bytes = text.len().saturating_add(1);
        let next = stdout.len().saturating_add(bytes);
        if next > self.limits.print_bytes || self.reserve_output(bytes).is_err() {
            return Err(RuntimeError::Limit("print/output budget exceeded".into()));
        }
        stdout.push_str(text);
        stdout.push('\n');
        Ok(())
    }

    pub(super) fn reserve_output(&self, bytes: usize) -> RuntimeResult<()> {
        self.output_used
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.output_bytes)
            })
            .map(|_| ())
            .map_err(|_| RuntimeError::Limit("output budget exceeded".into()))
    }

    pub(super) fn reserve_preview(&self, preview: &str) -> RuntimeResult<()> {
        let bytes = json_string_encoded_len_bounded(preview, self.remaining_output())
            .ok_or_else(|| RuntimeError::Limit("output budget exceeded".into()))?;
        self.reserve_output(bytes)
    }

    pub(super) fn remaining_output(&self) -> usize {
        self.output_bytes.saturating_sub(
            self.output_used
                .load(AtomicOrdering::Relaxed)
                .min(self.output_bytes),
        )
    }

    pub(super) fn ensure_value(&self, value: &Value) -> RuntimeResult<()> {
        let size = value_size_bounded(value, self.limits.value_bytes, self.limits.runtime_depth);
        if size > self.limits.value_bytes {
            Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn ensure_capture(&self, binding_name_bytes: usize) -> RuntimeResult<()> {
        let limit = self.limits.environment_bytes;
        let retained = environment_size_bounded(&self.env, limit, self.limits.runtime_depth);
        if retained > limit {
            return Err(RuntimeError::Limit(
                "persistent environment budget exceeded".into(),
            ));
        }
        let remaining = limit.saturating_sub(retained);
        let clone_cost =
            environment_clone_size_bounded(&self.env, remaining, self.limits.runtime_depth);
        let projected = retained
            .saturating_add(clone_cost)
            .saturating_add(binding_name_bytes)
            .saturating_add(std::mem::size_of::<Callable>())
            .saturating_add(std::mem::size_of::<Value>());
        if projected > limit {
            Err(RuntimeError::Limit(
                "persistent environment budget exceeded".into(),
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn add_value_size(
        &self,
        allocated: &mut usize,
        value: &Value,
        metadata_bytes: usize,
    ) -> RuntimeResult<()> {
        let remaining = self.limits.value_bytes.saturating_sub(*allocated);
        let value_bytes = value_size_bounded(value, remaining, self.limits.runtime_depth);
        let next = allocated
            .saturating_add(metadata_bytes)
            .saturating_add(value_bytes);
        if next > self.limits.value_bytes {
            return Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ));
        }
        *allocated = next;
        Ok(())
    }

    pub(super) fn push_interpolation(&self, output: &mut String, value: &str) -> RuntimeResult<()> {
        if output.len().saturating_add(value.len()) > self.limits.value_bytes {
            return Err(RuntimeError::Limit(
                "intermediate value budget exceeded".into(),
            ));
        }
        output.push_str(value);
        Ok(())
    }
}
