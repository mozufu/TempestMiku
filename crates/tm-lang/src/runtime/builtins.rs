use super::*;

impl Evaluator {
    pub(super) fn builtin<'a>(
        &'a mut self,
        name: &'a str,
        args: Vec<Value>,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            let result: RuntimeResult<Value> = async {
                match name {
                    "print" => {
                        let text = render_value_bounded(
                            &args[0],
                            self.limits.value_bytes.min(self.remaining_output()),
                            self.limits.runtime_depth,
                        )?;
                        self.push_stdout(&text)?;
                        Ok(Value::Null)
                    }
                    "display" => {
                        let payload = if self.sensitive_cell {
                            json!({"cellId": self.cell_id, "spec": "[redacted]", "value": "[redacted]"})
                        } else {
                            let remaining = self.remaining_output();
                            let spec_bytes = value_json_bounded(
                                &args[0],
                                remaining,
                                false,
                                self.limits.runtime_depth,
                            )
                                .map(|(bytes, _)| bytes)
                                .ok_or_else(|| {
                                    RuntimeError::Limit(
                                        "display/output budget exceeded".into(),
                                    )
                                })?;
                            let value_bytes = value_json_bounded(
                                &args[1],
                                remaining.saturating_sub(spec_bytes),
                                false,
                                self.limits.runtime_depth,
                            )
                            .map(|(bytes, _)| bytes)
                            .ok_or_else(|| {
                                RuntimeError::Limit("display/output budget exceeded".into())
                            })?;
                            let control_bytes = self.cell_id.len().saturating_add(32);
                            let total = spec_bytes
                                .saturating_add(value_bytes)
                                .saturating_add(control_bytes);
                            if total > remaining {
                                return Err(RuntimeError::Limit(
                                    "display/output budget exceeded".into(),
                                ));
                            }
                            json!({"cellId": self.cell_id, "spec": args[0].to_json(), "value": args[1].to_json()})
                        };
                        let payload_bytes = json_encoded_len_bounded(
                            &payload,
                            self.remaining_output(),
                        )
                        .ok_or_else(|| {
                            RuntimeError::Limit("display/output budget exceeded".into())
                        })?;
                        self.reserve_output(payload_bytes)
                            .map_err(|_| {
                                RuntimeError::Limit("display/output budget exceeded".into())
                            })?;
                        self.emit("display", payload).await?;
                        Ok(args[1].clone())
                    }
                    "length" => match &args[0] {
                        Value::List(v) => Ok(Value::Int(v.len() as i64)),
                        Value::String(v) => Ok(Value::Int(v.chars().count() as i64)),
                        Value::Record(v) => Ok(Value::Int(v.len() as i64)),
                        other => Err(RuntimeError::Type(format!("length on {}", other.kind()))),
                    },
                    "lines" => match &args[0] {
                        Value::String(v) => {
                            let mut result = Vec::new();
                            let mut allocated = 0;
                            let mut iterations = 0;
                            for line in v.lines() {
                                self.cooperate(&mut iterations).await?;
                                let value = Value::String(line.into());
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                            Ok(Value::List(result))
                        }
                        other => Err(RuntimeError::Type(format!("lines on {}", other.kind()))),
                    },
                    "split" => match (&args[0], &args[1]) {
                        (Value::String(delimiter), Value::String(value)) => {
                            let mut result = Vec::new();
                            let mut allocated = 0;
                            let mut iterations = 0;
                            for part in value.split(delimiter) {
                                self.cooperate(&mut iterations).await?;
                                let value = Value::String(part.into());
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                            Ok(Value::List(result))
                        }
                        _ => Err(RuntimeError::Type("split requires strings".into())),
                    },
                    "contains" => match (&args[0], &args[1]) {
                        (Value::String(needle), Value::String(value)) => {
                            Ok(Value::Bool(value.contains(needle)))
                        }
                        _ => Err(RuntimeError::Type("contains requires strings".into())),
                    },
                    "take" => match (&args[0], &args[1]) {
                        (Value::Int(count), Value::List(values)) => Ok(Value::List(
                            values
                                .iter()
                                .take((*count).max(0) as usize)
                                .cloned()
                                .collect(),
                        )),
                        _ => Err(RuntimeError::Type("take requires count and list".into())),
                    },
                    "merge" => match (&args[0], &args[1]) {
                        (Value::Record(update), Value::Record(base)) => {
                            let mut result = BTreeMap::new();
                            let mut allocated = 0;
                            for (name, value) in base.iter().chain(update) {
                                self.add_value_size(&mut allocated, value, name.len())?;
                                result.insert(name.clone(), value.clone());
                            }
                            Ok(Value::Record(result))
                        }
                        _ => Err(RuntimeError::Type("merge requires records".into())),
                    },
                    "map" => {
                        let (function, values) = list_call_args(args, "map")?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let value = self.apply(function.clone(), value).await?;
                            self.add_value_size(&mut allocated, &value, 0)?;
                            result.push(value);
                        }
                        Ok(Value::List(result))
                    }
                    "flatmap" => {
                        let (function, values) = list_call_args(args, "flatmap")?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            match self.apply(function.clone(), value).await? {
                                Value::List(items) => {
                                    for item in items {
                                        self.cooperate(&mut iterations).await?;
                                        self.add_value_size(&mut allocated, &item, 0)?;
                                        result.push(item);
                                    }
                                }
                                other => {
                                    return Err(RuntimeError::Type(format!(
                                        "flatmap returned {}",
                                        other.kind()
                                    )));
                                }
                            }
                        }
                        Ok(Value::List(result))
                    }
                    "filter" | "where" => {
                        let (function, values) = list_call_args(args, name)?;
                        let mut result = Vec::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            if matches!(
                                self.apply(function.clone(), value.clone()).await?,
                                Value::Bool(true)
                            ) {
                                self.add_value_size(&mut allocated, &value, 0)?;
                                result.push(value);
                            }
                        }
                        Ok(Value::List(result))
                    }
                    "select" | "aggregate" => {
                        let (function, values) = list_call_args(args, name)?;
                        let mut result = Vec::with_capacity(values.len());
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let value = self.apply(function.clone(), value).await?;
                            self.add_value_size(&mut allocated, &value, 0)?;
                            result.push(value);
                        }
                        Ok(Value::List(result))
                    }
                    "sort_by" => {
                        let function = args[0].clone();
                        let descending =
                            matches!(&args[1], Value::String(value) if value == "desc");
                        let Value::List(values) = &args[2] else {
                            return Err(RuntimeError::Type("sort_by requires a list".into()));
                        };
                        let mut keyed = Vec::with_capacity(values.len());
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let key = self.apply(function.clone(), value.clone()).await?;
                            self.add_value_size(&mut allocated, &key, 0)?;
                            self.add_value_size(&mut allocated, value, 0)?;
                            keyed.push((key, value.clone()));
                        }
                        let sort_work = (keyed.len().max(1).ilog2() as u64 + 1)
                            .saturating_mul(keyed.len() as u64);
                        self.charge_work(sort_work)?;
                        tokio::task::yield_now().await;
                        keyed.sort_by(|left, right| compare_sort_keys(&left.0, &right.0));
                        if descending {
                            keyed.reverse();
                        }
                        Ok(Value::List(
                            keyed.into_iter().map(|(_, value)| value).collect(),
                        ))
                    }
                    "group_by" => {
                        let (function, values) = list_call_args(args, "group_by")?;
                        let mut groups = BTreeMap::<String, (Value, Vec<Value>)>::new();
                        let mut allocated = 0;
                        let mut iterations = 0;
                        for value in values {
                            self.cooperate(&mut iterations).await?;
                            let key = self.apply(function.clone(), value.clone()).await?;
                            let group = group_key(
                                &key,
                                self.limits.value_bytes.saturating_sub(allocated),
                                self.limits.runtime_depth,
                            )?;
                            if !groups.contains_key(&group) {
                                self.add_value_size(&mut allocated, &key, group.len())?;
                            }
                            self.add_value_size(&mut allocated, &value, 0)?;
                            groups
                                .entry(group)
                                .or_insert_with(|| (key, Vec::new()))
                                .1
                                .push(value);
                        }
                        Ok(Value::List(
                            groups
                                .into_values()
                                .map(|(key, rows)| {
                                    Value::Record(BTreeMap::from([
                                        ("key".into(), key),
                                        ("rows".into(), Value::List(rows)),
                                    ]))
                                })
                                .collect(),
                        ))
                    }
                    "sum" => match &args[0] {
                        Value::List(values) => {
                            self.charge_work((values.len() as u64).saturating_mul(2))?;
                            tokio::task::yield_now().await;
                            if values.iter().all(|value| matches!(value, Value::Int(_))) {
                                let total = values.iter().try_fold(0_i64, |total, value| {
                                    let Value::Int(value) = value else {
                                        unreachable!("all values were checked as integers")
                                    };
                                    total.checked_add(*value).ok_or_else(|| {
                                        RuntimeError::Type("integer overflow in sum".into())
                                    })
                                })?;
                                return Ok(Value::Int(total));
                            }

                            let mut total = 0.0_f64;
                            for value in values {
                                match value {
                                    Value::Int(value) => total += *value as f64,
                                    Value::Decimal(value) => total += value,
                                    other => {
                                        return Err(RuntimeError::Type(format!(
                                            "sum on {}",
                                            other.kind()
                                        )));
                                    }
                                }
                            }
                            Ok(Value::Decimal(total))
                        }
                        other => Err(RuntimeError::Type(format!("sum on {}", other.kind()))),
                    },
                    "table" => Ok(args[0].clone()),
                    "par_map" => self.parallel_map(args).await,
                    "par" => self.parallel(args[0].clone()).await,
                    "help" => match &args[0] {
                        Value::Callable(callable) => match callable.as_ref() {
                            Callable::Capability { name } => {
                                let docs = self
                                    .registry
                                    .docs(name, &self.invocation)
                                    .map_err(host_error)?;
                                let docs = serde_json::to_value(docs).map_err(|error| {
                                    RuntimeError::Type(format!(
                                        "failed to encode capability docs: {error}"
                                    ))
                                })?;
                                if json_value_size_bounded(
                                    &docs,
                                    self.limits.value_bytes,
                                    self.limits.runtime_depth,
                                )
                                    > self.limits.value_bytes
                                {
                                    return Err(RuntimeError::Limit(
                                        "capability docs value budget exceeded".into(),
                                    ));
                                }
                                Ok(Value::from_json(docs))
                            }
                            _ => Ok(Value::String(render_value_bounded(
                                &args[0],
                                self.limits.value_bytes,
                                self.limits.runtime_depth,
                            )?)),
                        },
                        _ => Ok(Value::String(render_value_bounded(
                            &args[0],
                            self.limits.value_bytes,
                            self.limits.runtime_depth,
                        )?)),
                    },
                    "rethrow" => Err(self
                        .current_error
                        .clone()
                        .unwrap_or_else(|| {
                            rethrow_error(
                                &args[0],
                                self.limits.preview_bytes,
                                self.limits.runtime_depth,
                            )
                        })),
                    other => Err(RuntimeError::Type(format!(
                        "unsupported prelude function {other}"
                    ))),
                }
            }
            .await;
            let value = result?;
            self.ensure_value(&value)?;
            Ok(value)
        })
    }
}
