use super::*;
#[derive(Clone)]
pub(super) struct Evaluator {
    pub(super) env: Environment,
    pub(super) catalog: CapabilityCatalog,
    pub(super) registry: Arc<HostRegistry>,
    pub(super) invocation: InvocationCtx,
    pub(super) limits: RuntimeLimits,
    pub(super) steps: Arc<AtomicU64>,
    pub(super) stdout: Arc<Mutex<String>>,
    pub(super) output_bytes: usize,
    pub(super) output_used: Arc<AtomicUsize>,
    pub(super) cell_id: String,
    pub(super) node_counter: Arc<AtomicU64>,
    pub(super) constructors: BTreeMap<String, bool>,
    pub(super) committed: BTreeSet<String>,
    pub(super) active: Arc<Mutex<Option<ActiveExecution>>>,
    pub(super) terminal_selected: Arc<std::sync::atomic::AtomicBool>,
    pub(super) scope_id: Option<String>,
    pub(super) sensitive_cell: bool,
    pub(super) current_error: Option<RuntimeError>,
    pub(super) depth: usize,
}

impl Evaluator {
    pub(super) async fn cell(&mut self, cell: &Cell) -> RuntimeResult<Value> {
        let mut value = Value::Null;
        for form in &cell.forms {
            value = self.form(form, true).await?;
        }
        Ok(value)
    }

    pub(super) fn form<'a>(
        &'a mut self,
        form: &'a Form,
        persistent: bool,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.step(form.span)?;
            match &form.node {
                FormKind::Type(decl) => {
                    for variant in &decl.variants {
                        self.constructors
                            .insert(variant.name.clone(), variant.payload.is_some());
                        self.env.insert(
                            variant.name.clone(),
                            if variant.payload.is_some() {
                                Value::Callable(Rc::new(Callable::Constructor {
                                    name: variant.name.clone(),
                                }))
                            } else {
                                Value::Tagged {
                                    name: variant.name.clone(),
                                    payload: None,
                                }
                            },
                        );
                    }
                    Ok(Value::Null)
                }
                FormKind::Let { pattern, value } => {
                    let value = self.expr(value).await?;
                    let bindings = self
                        .match_pattern(pattern, &value)?
                        .ok_or_else(|| RuntimeError::Type("let pattern did not match".into()))?;
                    for (name, value) in bindings {
                        self.env.insert(name.clone(), value);
                        if persistent {
                            self.committed.insert(name);
                        }
                    }
                    Ok(value)
                }
                FormKind::Fun { name, params, body } => {
                    self.ensure_capture(name.len())?;
                    let function = Value::Callable(Rc::new(Callable::User {
                        params: params.clone(),
                        body: body.clone(),
                        captured: Rc::new(self.env.clone()),
                        args: Vec::new(),
                    }));
                    self.env.insert(name.clone(), function.clone());
                    if persistent {
                        self.committed.insert(name.clone());
                    }
                    Ok(function)
                }
                FormKind::Expr(expr) => self.expr(expr).await,
            }
        })
    }

    pub(super) fn expr<'a>(
        &'a mut self,
        expr: &'a Expr,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.enter_runtime_depth()?;
            let evaluated = async {
                self.step(expr.span)?;
                let result: RuntimeResult<Value> = async {
                match &expr.node {
                ExprKind::String(value) => Ok(Value::String(self.interpolate(value).await?)),
                ExprKind::Int(value) => Ok(Value::Int(*value)),
                ExprKind::Decimal(value) => Ok(Value::Decimal(*value)),
                ExprKind::Bool(value) => Ok(Value::Bool(*value)),
                ExprKind::Null => Ok(Value::Null),
                ExprKind::Uri(value) => Ok(Value::Uri(value.clone())),
                ExprKind::Name(name) => self
                    .env
                    .get(name)
                    .cloned()
                    .ok_or_else(|| RuntimeError::Type(format!("unbound name {name}"))),
                ExprKind::Constructor(name) => {
                    Ok(self.env.get(name).cloned().unwrap_or_else(|| {
                        if name == "None" {
                            return Value::Tagged {
                                name: name.clone(),
                                payload: None,
                            };
                        }
                        Value::Callable(Rc::new(Callable::Constructor { name: name.clone() }))
                    }))
                }
                ExprKind::Capability(name) => Ok(Value::Callable(Rc::new(Callable::Capability {
                    name: name.clone(),
                }))),
                ExprKind::List(values) => {
                    let mut output = Vec::with_capacity(values.len());
                    let mut allocated = 0;
                    for value in values {
                        let value = self.expr(value).await?;
                        self.add_value_size(&mut allocated, &value, 0)?;
                        output.push(value);
                    }
                    Ok(Value::List(output))
                }
                ExprKind::Record(fields) => {
                    let mut output = BTreeMap::new();
                    let mut allocated = 0;
                    for (name, value) in fields {
                        let value = self.expr(value).await?;
                        self.add_value_size(&mut allocated, &value, name.len())?;
                        output.insert(name.clone(), value);
                    }
                    Ok(Value::Record(output))
                }
                ExprKind::Lambda { params, body } => {
                    self.ensure_capture(0)?;
                    Ok(Value::Callable(Rc::new(Callable::User {
                        params: params.clone(),
                        body: (**body).clone(),
                        captured: Rc::new(self.env.clone()),
                        args: Vec::new(),
                    })))
                }
                ExprKind::Apply { function, argument }
                    if matches!(&function.node, ExprKind::Name(name) if name == "par")
                        && matches!(&argument.node, ExprKind::Record(_)) =>
                {
                    self.parallel_expr(argument).await
                }
                ExprKind::Apply { function, argument } => {
                    let row_argument = matches!(
                        &function.node,
                        ExprKind::Name(name)
                            if matches!(name.as_str(), "where" | "select" | "sort_by" | "group_by" | "aggregate")
                    );
                    let function = self.expr(function).await?;
                    let argument = if row_argument {
                        self.ensure_capture(0)?;
                        Value::Callable(Rc::new(Callable::Row {
                            body: (**argument).clone(),
                            captured: Rc::new(self.env.clone()),
                        }))
                    } else {
                        self.expr(argument).await?
                    };
                    self.apply(function, argument).await
                }
                ExprKind::Field { target, field } => match self.expr(target).await? {
                    Value::Record(mut fields) => fields
                        .remove(field)
                        .ok_or_else(|| RuntimeError::Type(format!("missing field {field}"))),
                    other => Err(RuntimeError::Type(format!(
                        "field access on {}",
                        other.kind()
                    ))),
                },
                ExprKind::Unary { op, value } => unary(*op, self.expr(value).await?),
                ExprKind::Binary { op, left, right } => {
                    let left = self.expr(left).await?;
                    match (*op, &left) {
                        (BinaryOp::And, Value::Bool(false)) | (BinaryOp::Or, Value::Bool(true)) => {
                            return Ok(left);
                        }
                        (BinaryOp::And | BinaryOp::Or, Value::Bool(true | false)) => {}
                        (BinaryOp::And | BinaryOp::Or, other) => {
                            return Err(RuntimeError::Type(format!(
                                "{} requires boolean operands; found {}",
                                logical_name(*op),
                                other.kind()
                            )));
                        }
                        _ => {}
                    }
                    let right = self.expr(right).await?;
                    match op {
                        BinaryOp::Equal => Ok(Value::Bool(self.values_equal(&left, &right)?)),
                        BinaryOp::NotEqual => Ok(Value::Bool(!self.values_equal(&left, &right)?)),
                        _ => binary(*op, left, right),
                    }
                }
                ExprKind::Pipe { value, target } => {
                    let value = self.expr(value).await?;
                    let target = self.expr(target).await?;
                    self.apply_last(target, value).await
                }
                ExprKind::If {
                    condition,
                    then_value,
                    else_value,
                } => match self.expr(condition).await? {
                    Value::Bool(true) => self.expr(then_value).await,
                    Value::Bool(false) => self.expr(else_value).await,
                    other => Err(RuntimeError::Type(format!(
                        "if condition is {}",
                        other.kind()
                    ))),
                },
                ExprKind::Match { value, arms } => {
                    let value = self.expr(value).await?;
                    for arm in arms {
                        if let Some(bindings) = self.match_pattern(&arm.pattern, &value)? {
                            let old = self.env.clone();
                            self.env.extend(bindings);
                            let result = self.expr(&arm.value).await;
                            self.env = old;
                            return result;
                        }
                    }
                    Err(RuntimeError::Type(
                        "non-exhaustive match reached runtime".into(),
                    ))
                }
                ExprKind::Handle { value, arms } => match self.expr(value).await {
                    Ok(value) => Ok(value),
                    Err(error @ RuntimeError::Persistence(_)) => Err(error),
                    Err(error) => {
                        let tagged = Value::Tagged {
                            name: error_name(&error).into(),
                            payload: Some(Box::new(runtime_error_payload(
                                &error,
                                self.limits.value_bytes,
                                self.limits.runtime_depth,
                            ))),
                        };
                        for arm in arms {
                            if let Some(bindings) = self.match_pattern(&arm.pattern, &tagged)? {
                                let old = self.env.clone();
                                let old_error = self.current_error.replace(error.clone());
                                self.env.extend(bindings);
                                let result = self.expr(&arm.value).await;
                                self.env = old;
                                self.current_error = old_error;
                                return result;
                            }
                        }
                        Err(error)
                    }
                },
                ExprKind::Block(forms) => {
                    let old = self.env.clone();
                    let old_constructors = self.constructors.clone();
                    let result: RuntimeResult<Value> = async {
                        let mut value = Value::Null;
                        for form in forms {
                            value = self.form(form, false).await?;
                        }
                        Ok(value)
                    }
                    .await;
                    self.env = old;
                    self.constructors = old_constructors;
                    result
                }
                    }
                }
                .await;
                let value = result?;
                self.ensure_value(&value)?;
                Ok(value)
            }
            .await;
            self.leave_runtime_depth();
            evaluated
        })
    }

    pub(super) fn apply<'a>(
        &'a mut self,
        function: Value,
        argument: Value,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.enter_runtime_depth()?;
            let applied = async {
                let Value::Callable(callable) = function else {
                    return Err(RuntimeError::Type(format!(
                        "{} is not callable",
                        function.kind()
                    )));
                };
                match callable.as_ref() {
                Callable::Capability { name } => self.perform(name, argument).await,
                Callable::Constructor { name } => Ok(Value::Tagged {
                    name: name.clone(),
                    payload: Some(Box::new(argument)),
                }),
                Callable::Builtin { name, args, arity } => {
                    if name == "par" && args.is_empty() {
                        if matches!(
                            &argument,
                            Value::Callable(callable)
                                if matches!(callable.as_ref(), Callable::Builtin { name, args, .. } if name == "map" && args.is_empty())
                        ) {
                            return Ok(Value::Callable(Rc::new(Callable::Builtin {
                                name: "par_map".into(),
                                args: Vec::new(),
                                arity: 2,
                            })));
                        }
                        return self.parallel(argument).await;
                    }
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() < *arity {
                        Ok(Value::Callable(Rc::new(Callable::Builtin {
                            name: name.clone(),
                            args,
                            arity: *arity,
                        })))
                    } else {
                        self.builtin(name, args).await
                    }
                }
                Callable::BuiltinDataLast {
                    name,
                    args,
                    data,
                    arity,
                } => {
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() + 1 < *arity {
                        Ok(Value::Callable(Rc::new(Callable::BuiltinDataLast {
                            name: name.clone(),
                            args,
                            data: data.clone(),
                            arity: *arity,
                        })))
                    } else {
                        args.push(data.clone());
                        self.builtin(name, args).await
                    }
                }
                Callable::User {
                    params,
                    body,
                    captured,
                    args,
                } => {
                    let mut args = args.clone();
                    args.push(argument);
                    if args.len() < params.len() {
                        return Ok(Value::Callable(Rc::new(Callable::User {
                            params: params.clone(),
                            body: body.clone(),
                            captured: captured.clone(),
                            args,
                        })));
                    }
                    let old = std::mem::replace(&mut self.env, captured.as_ref().clone());
                    for (name, value) in old
                        .iter()
                        .filter(|(_, value)| matches!(value, Value::Callable(_)))
                    {
                        self.env
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                    let result: RuntimeResult<Value> = async {
                        for (pattern, value) in params.iter().zip(args) {
                            let Some(bindings) = self.match_pattern(pattern, &value)? else {
                                return Err(RuntimeError::Type(
                                    "function pattern did not match".into(),
                                ));
                            };
                            self.env.extend(bindings);
                        }
                        self.expr(body).await
                    }
                    .await;
                    self.env = old;
                    result
                }
                Callable::Row { body, captured } => {
                    let old = std::mem::replace(&mut self.env, captured.as_ref().clone());
                    if let Value::Record(fields) = &argument {
                        self.env.insert("row".into(), argument.clone());
                        for (name, value) in fields {
                            self.env
                                .entry(name.clone())
                                .or_insert_with(|| value.clone());
                        }
                        if let Some(Value::List(rows)) = fields.get("rows") {
                            self.env
                                .entry("count".into())
                                .or_insert(Value::Int(rows.len() as i64));
                            let mut columns = BTreeMap::<String, Vec<Value>>::new();
                            for row in rows {
                                if let Value::Record(fields) = row {
                                    for (name, value) in fields {
                                        columns
                                            .entry(name.clone())
                                            .or_default()
                                            .push(value.clone());
                                    }
                                }
                            }
                            for (name, values) in columns {
                                self.env.entry(name).or_insert(Value::List(values));
                            }
                        }
                    } else {
                        self.env.insert("row".into(), argument);
                    }
                    let result = self.expr(body).await;
                    self.env = old;
                    result
                    }
                }
            }
            .await;
            self.leave_runtime_depth();
            applied
        })
    }

    pub(super) fn apply_last<'a>(
        &'a mut self,
        function: Value,
        argument: Value,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            let Value::Callable(callable) = &function else {
                return self.apply(function, argument).await;
            };
            match callable.as_ref() {
                Callable::Builtin { name, args, arity } if args.len() + 1 < *arity => {
                    Ok(Value::Callable(Rc::new(Callable::BuiltinDataLast {
                        name: name.clone(),
                        args: args.clone(),
                        data: argument,
                        arity: *arity,
                    })))
                }
                _ => self.apply(function, argument).await,
            }
        })
    }
}
