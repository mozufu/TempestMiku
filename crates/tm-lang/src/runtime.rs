use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{Arc, Mutex},
};

use futures::future::LocalBoxFuture;
use serde_json::{Value as JsonValue, json};
use tm_host::{
    ApprovalDecision, ApprovalPolicy, HostError, HostEventSink, HostRegistry, InvocationCtx,
};

use crate::{
    BinaryOp, Callable, CapabilityCatalog, Cell, Diagnostic, EffectMachine, EffectSignature,
    Environment, Expr, ExprKind, Form, FormKind, Pattern, PatternKind, Span, UnaryOp, Value,
    ValueType, check_with_bindings, parse,
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Diagnostic(#[from] Diagnostic),
    #[error("{name}: {message}")]
    Effect {
        name: String,
        message: String,
        payload: Option<JsonValue>,
    },
    #[error("TypeError: {0}")]
    Type(String),
    #[error("ResourceLimitError: {0}")]
    Limit(String),
    #[error("CancellationError: cell cancelled")]
    Cancelled,
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    pub steps: u64,
    pub print_bytes: usize,
    pub preview_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            steps: 100_000,
            print_bytes: 8 * 1024,
            preview_bytes: 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeOutput {
    pub stdout: String,
    pub value: Value,
    pub committed: BTreeSet<String>,
}

#[derive(Clone)]
pub struct Interpreter {
    env: Environment,
    catalog: CapabilityCatalog,
    registry: Arc<HostRegistry>,
    invocation: InvocationCtx,
    limits: RuntimeLimits,
    cell_counter: u64,
}

impl Interpreter {
    pub fn new(
        catalog: CapabilityCatalog,
        registry: Arc<HostRegistry>,
        invocation: InvocationCtx,
        limits: RuntimeLimits,
    ) -> Self {
        Self {
            env: prelude(),
            catalog,
            registry,
            invocation,
            limits,
            cell_counter: 0,
        }
    }

    pub fn environment(&self) -> &Environment {
        &self.env
    }
    pub fn reset(&mut self) {
        self.env = prelude();
        self.cell_counter = 0;
    }

    pub(crate) fn fork_for_parallel(&self, offset: u64) -> Self {
        let mut fork = self.clone();
        fork.cell_counter += offset;
        fork
    }

    pub(crate) fn finish_parallel_batch(&mut self, count: u64) {
        self.cell_counter += count;
    }

    pub(crate) fn merge_committed_from(&mut self, fork: &Self, committed: &BTreeSet<String>) {
        for name in committed {
            if let Some(value) = fork.env.get(name) {
                self.env.insert(name.clone(), value.clone());
            }
        }
    }

    pub(crate) async fn emit_dependency_failure(
        &mut self,
        source: &str,
        error: &str,
    ) -> RuntimeResult<()> {
        self.cell_counter += 1;
        let cell_id = format!("cell-{}", self.cell_counter);
        self.invocation
            .emit_event(
                "cell_start",
                json!({"cellId": cell_id, "sourcePreview": bounded(source, self.limits.preview_bytes)}),
            )
            .await
            .map_err(host_error)?;
        self.invocation
            .emit_event(
                "cell_result",
                json!({"cellId": cell_id, "status": "failed", "error": bounded(error, self.limits.preview_bytes)}),
            )
            .await
            .map_err(host_error)
    }

    pub async fn eval(
        &mut self,
        source: &str,
        output_bytes: usize,
    ) -> RuntimeResult<RuntimeOutput> {
        let cell = parse(source)?;
        check_with_bindings(source, &cell, &self.catalog, self.env.keys().cloned())?;
        self.cell_counter += 1;
        let cell_id = format!("cell-{}", self.cell_counter);
        let mut evaluator = Evaluator {
            env: self.env.clone(),
            catalog: self.catalog.clone(),
            registry: Arc::clone(&self.registry),
            invocation: self.invocation.clone(),
            limits: self.limits.clone(),
            steps: 0,
            stdout: String::new(),
            output_bytes,
            cell_id: cell_id.clone(),
            node_counter: 0,
            constructors: BTreeMap::new(),
            committed: BTreeSet::new(),
        };
        evaluator.emit("cell_start", json!({"cellId": cell_id, "sourcePreview": bounded(source, self.limits.preview_bytes)})).await?;
        let result = evaluator.cell(&cell).await;
        match result {
            Ok(value) => {
                let json = value.to_json();
                if json.to_string().len() > output_bytes && !has_artifact_reference(&json) {
                    let error = RuntimeError::Limit("result/output budget exceeded".into());
                    let _ = evaluator.emit("cell_result", json!({"cellId": cell_id, "status": "failed", "error": error.to_string()})).await;
                    return Err(error);
                }
                evaluator
                    .emit(
                        "binding_committed",
                        json!({"cellId": cell_id, "names": evaluator.committed}),
                    )
                    .await?;
                evaluator.emit("cell_result", json!({"cellId": cell_id, "status": "completed", "resultPreview": bounded(&json.to_string(), self.limits.preview_bytes)})).await?;
                for name in &evaluator.committed {
                    if let Some(value) = evaluator.env.get(name) {
                        self.env.insert(name.clone(), value.clone());
                    }
                }
                Ok(RuntimeOutput {
                    stdout: evaluator.stdout,
                    value,
                    committed: evaluator.committed,
                })
            }
            Err(error) => {
                let _ = evaluator.emit("cell_result", json!({"cellId": cell_id, "status": "failed", "error": bounded(&error.to_string(), self.limits.preview_bytes)})).await;
                Err(error)
            }
        }
    }
}

struct Evaluator {
    env: Environment,
    catalog: CapabilityCatalog,
    registry: Arc<HostRegistry>,
    invocation: InvocationCtx,
    limits: RuntimeLimits,
    steps: u64,
    stdout: String,
    output_bytes: usize,
    cell_id: String,
    node_counter: u64,
    constructors: BTreeMap<String, bool>,
    committed: BTreeSet<String>,
}

impl Evaluator {
    async fn cell(&mut self, cell: &Cell) -> RuntimeResult<Value> {
        let mut value = Value::Null;
        for form in &cell.forms {
            value = self.form(form, true).await?;
        }
        Ok(value)
    }

    fn form<'a>(
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
                    let bindings = match_pattern(pattern, &value)
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
                    let function = Value::Callable(Rc::new(Callable::User {
                        params: params.clone(),
                        body: body.clone(),
                        captured: self.env.clone(),
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

    fn expr<'a>(&'a mut self, expr: &'a Expr) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            self.step(expr.span)?;
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
                    let mut output = Vec::new();
                    for value in values {
                        output.push(self.expr(value).await?);
                    }
                    Ok(Value::List(output))
                }
                ExprKind::Record(fields) => {
                    let mut output = BTreeMap::new();
                    for (name, value) in fields {
                        output.insert(name.clone(), self.expr(value).await?);
                    }
                    Ok(Value::Record(output))
                }
                ExprKind::Lambda { params, body } => Ok(Value::Callable(Rc::new(Callable::User {
                    params: params.clone(),
                    body: (**body).clone(),
                    captured: self.env.clone(),
                    args: Vec::new(),
                }))),
                ExprKind::Apply { function, argument } => {
                    let row_argument = matches!(
                        &function.node,
                        ExprKind::Name(name)
                            if matches!(name.as_str(), "where" | "select" | "sort_by" | "group_by" | "aggregate")
                    );
                    let function = self.expr(function).await?;
                    let argument = if row_argument {
                        Value::Callable(Rc::new(Callable::Row {
                            body: (**argument).clone(),
                            captured: self.env.clone(),
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
                    binary(*op, left, right)
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
                        if let Some(bindings) = match_pattern(&arm.pattern, &value) {
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
                    Err(error) => {
                        let tagged = Value::Tagged {
                            name: error_name(&error).into(),
                            payload: Some(Box::new(runtime_error_payload(&error))),
                        };
                        for arm in arms {
                            if let Some(bindings) = match_pattern(&arm.pattern, &tagged) {
                                let old = self.env.clone();
                                self.env.extend(bindings);
                                let result = self.expr(&arm.value).await;
                                self.env = old;
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
        })
    }

    fn apply<'a>(
        &'a mut self,
        function: Value,
        argument: Value,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
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
                    let old = self.env.clone();
                    self.env = captured.clone();
                    for (name, value) in old
                        .iter()
                        .filter(|(_, value)| matches!(value, Value::Callable(_)))
                    {
                        self.env
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                    for (pattern, value) in params.iter().zip(args) {
                        let Some(bindings) = match_pattern(pattern, &value) else {
                            self.env = old;
                            return Err(RuntimeError::Type(
                                "function pattern did not match".into(),
                            ));
                        };
                        self.env.extend(bindings);
                    }
                    let result = self.expr(body).await;
                    self.env = old;
                    result
                }
                Callable::Row { body, captured } => {
                    let old = self.env.clone();
                    self.env = captured.clone();
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
        })
    }

    fn apply_last<'a>(
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

    fn builtin<'a>(
        &'a mut self,
        name: &'a str,
        args: Vec<Value>,
    ) -> LocalBoxFuture<'a, RuntimeResult<Value>> {
        Box::pin(async move {
            match name {
                "print" => {
                    let text = render(&args[0]);
                    self.push_stdout(&text)?;
                    Ok(Value::Null)
                }
                "display" => {
                    let spec = args[0].to_json();
                    let value = args[1].to_json();
                    let payload = json!({"cellId": self.cell_id, "spec": spec, "value": value});
                    if payload.to_string().len() > self.output_bytes {
                        return Err(RuntimeError::Limit("display/output budget exceeded".into()));
                    }
                    self.emit("display", payload).await?;
                    Ok(Value::from_json(value))
                }
                "length" => match &args[0] {
                    Value::List(v) => Ok(Value::Int(v.len() as i64)),
                    Value::String(v) => Ok(Value::Int(v.chars().count() as i64)),
                    Value::Record(v) => Ok(Value::Int(v.len() as i64)),
                    other => Err(RuntimeError::Type(format!("length on {}", other.kind()))),
                },
                "lines" => match &args[0] {
                    Value::String(v) => Ok(Value::List(
                        v.lines().map(|line| Value::String(line.into())).collect(),
                    )),
                    other => Err(RuntimeError::Type(format!("lines on {}", other.kind()))),
                },
                "split" => match (&args[0], &args[1]) {
                    (Value::String(delimiter), Value::String(value)) => Ok(Value::List(
                        value
                            .split(delimiter)
                            .map(|part| Value::String(part.into()))
                            .collect(),
                    )),
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
                        let mut result = base.clone();
                        result.extend(update.clone());
                        Ok(Value::Record(result))
                    }
                    _ => Err(RuntimeError::Type("merge requires records".into())),
                },
                "map" => {
                    let (function, values) = list_call_args(&args, "map")?;
                    let mut result = Vec::new();
                    for value in values {
                        result.push(self.apply(function.clone(), value).await?);
                    }
                    Ok(Value::List(result))
                }
                "flatmap" => {
                    let (function, values) = list_call_args(&args, "flatmap")?;
                    let mut result = Vec::new();
                    for value in values {
                        match self.apply(function.clone(), value).await? {
                            Value::List(items) => result.extend(items),
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
                    let (function, values) = list_call_args(&args, name)?;
                    let mut result = Vec::new();
                    for value in values {
                        if self.apply(function.clone(), value.clone()).await? == Value::Bool(true) {
                            result.push(value);
                        }
                    }
                    Ok(Value::List(result))
                }
                "select" | "aggregate" => {
                    let (function, values) = list_call_args(&args, name)?;
                    let mut result = Vec::with_capacity(values.len());
                    for value in values {
                        result.push(self.apply(function.clone(), value).await?);
                    }
                    Ok(Value::List(result))
                }
                "sort_by" => {
                    let function = args[0].clone();
                    let descending = matches!(&args[1], Value::String(value) if value == "desc");
                    let Value::List(values) = &args[2] else {
                        return Err(RuntimeError::Type("sort_by requires a list".into()));
                    };
                    let mut keyed = Vec::with_capacity(values.len());
                    for value in values {
                        keyed.push((
                            self.apply(function.clone(), value.clone()).await?,
                            value.clone(),
                        ));
                    }
                    keyed.sort_by(|left, right| compare_sort_keys(&left.0, &right.0));
                    if descending {
                        keyed.reverse();
                    }
                    Ok(Value::List(
                        keyed.into_iter().map(|(_, value)| value).collect(),
                    ))
                }
                "group_by" => {
                    let (function, values) = list_call_args(&args, "group_by")?;
                    let mut groups = BTreeMap::<String, (Value, Vec<Value>)>::new();
                    for value in values {
                        let key = self.apply(function.clone(), value.clone()).await?;
                        groups
                            .entry(group_key(&key)?)
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
                            Ok(Value::from_json(docs))
                        }
                        _ => Ok(Value::String(render(&args[0]))),
                    },
                    _ => Ok(Value::String(render(&args[0]))),
                },
                "rethrow" => Err(RuntimeError::Effect {
                    name: "Rethrown".into(),
                    message: render(&args[0]),
                    payload: None,
                }),
                other => Err(RuntimeError::Type(format!(
                    "unsupported prelude function {other}"
                ))),
            }
        })
    }

    async fn parallel(&mut self, value: Value) -> RuntimeResult<Value> {
        self.node_counter += 1;
        let node = format!("{}-scope-{}", self.cell_id, self.node_counter);
        self.emit(
            "scope_start",
            json!({"cellId": self.cell_id, "nodeId": node, "kind": "par"}),
        )
        .await?;
        // Host handlers retain their own concurrency/idempotency contracts. This first backend
        // executes children in deterministic order while exposing one cancellation-owning scope.
        self.emit(
            "scope_result",
            json!({"cellId": self.cell_id, "nodeId": node, "status": "completed"}),
        )
        .await?;
        Ok(value)
    }

    async fn parallel_map(&mut self, args: Vec<Value>) -> RuntimeResult<Value> {
        let (function, values) = list_call_args(&args, "par map")?;
        self.node_counter += 1;
        let node = format!("{}-scope-{}", self.cell_id, self.node_counter);
        self.emit(
            "scope_start",
            json!({"cellId": self.cell_id, "nodeId": node, "kind": "par_map", "total": values.len()}),
        )
        .await?;
        let total = values.len();
        let mut output = Vec::with_capacity(total);
        for (index, value) in values.into_iter().enumerate() {
            match self.apply(function.clone(), value).await {
                Ok(value) => {
                    output.push(value);
                    self.emit(
                        "scope_progress",
                        json!({"cellId": self.cell_id, "nodeId": node, "completed": index + 1, "total": total}),
                    )
                    .await?;
                }
                Err(error) => {
                    self.emit(
                        "scope_result",
                        json!({"cellId": self.cell_id, "nodeId": node, "status": "failed", "cancelledSiblings": total.saturating_sub(index + 1)}),
                    )
                    .await?;
                    return Err(error);
                }
            }
        }
        self.emit(
            "scope_result",
            json!({"cellId": self.cell_id, "nodeId": node, "status": "completed", "total": total}),
        )
        .await?;
        Ok(Value::List(output))
    }

    async fn perform(&mut self, name: &str, argument: Value) -> RuntimeResult<Value> {
        self.node_counter += 1;
        let node_id = format!("{}-node-{}", self.cell_id, self.node_counter);
        let args = normalize_effect_args(name, argument);
        let preview = if self
            .catalog
            .effect(name)
            .is_some_and(|effect| effect.sensitive)
        {
            "[redacted]".to_string()
        } else {
            bounded(&args.to_string(), self.limits.preview_bytes)
        };
        self.emit("effect_start", json!({"cellId": self.cell_id, "nodeId": node_id, "capability": name, "argsPreview": preview})).await?;
        let mut ctx = self.invocation.clone();
        let machine = Arc::new(Mutex::new(EffectMachine::new(
            self.cell_id.clone(),
            node_id.clone(),
            ctx.session_id.clone(),
        )));
        ctx.approvals = Arc::new(TracingApproval {
            inner: Arc::clone(&ctx.approvals),
            events: Arc::clone(&ctx.events),
            cell_id: self.cell_id.clone(),
            node_id: node_id.clone(),
            machine: Arc::clone(&machine),
        });
        match self.registry.invoke(name, args, &ctx).await {
            Ok(value) => {
                machine
                    .lock()
                    .expect("effect machine lock poisoned")
                    .complete()
                    .map_err(|error| RuntimeError::Effect {
                        name: "EffectStateError".into(),
                        message: error.to_string(),
                        payload: None,
                    })?;
                self.emit("effect_result", json!({"cellId": self.cell_id, "nodeId": node_id, "status": "completed", "resultPreview": bounded(&value.to_string(), self.limits.preview_bytes)})).await?;
                Ok(Value::from_json(value))
            }
            Err(error) => {
                let _ = machine.lock().expect("effect machine lock poisoned").fail();
                self.emit("effect_result", json!({"cellId": self.cell_id, "nodeId": node_id, "status": "failed", "error": bounded(&error.to_string(), self.limits.preview_bytes)})).await?;
                Err(host_error(error))
            }
        }
    }

    fn interpolate<'a>(&'a mut self, source: &'a str) -> LocalBoxFuture<'a, RuntimeResult<String>> {
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
                    let cell = parse(&fragment)?;
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
                    output.push_str(&render(&self.expr(expr).await?));
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
                output.push_str(&render(value));
                index = end;
            }
            Ok(output)
        })
    }

    async fn emit(&self, event: &str, payload: JsonValue) -> RuntimeResult<()> {
        self.invocation
            .events
            .emit(event, payload)
            .await
            .map_err(host_error)
    }
    fn step(&mut self, _span: Span) -> RuntimeResult<()> {
        self.steps += 1;
        if self.steps > self.limits.steps {
            Err(RuntimeError::Limit("step budget exceeded".into()))
        } else {
            Ok(())
        }
    }
    fn push_stdout(&mut self, text: &str) -> RuntimeResult<()> {
        let next = self.stdout.len() + text.len() + 1;
        if next > self.output_bytes.min(self.limits.print_bytes) {
            return Err(RuntimeError::Limit("print/output budget exceeded".into()));
        }
        self.stdout.push_str(text);
        self.stdout.push('\n');
        Ok(())
    }
}

struct TracingApproval {
    inner: Arc<dyn ApprovalPolicy>,
    events: Arc<dyn HostEventSink>,
    cell_id: String,
    node_id: String,
    machine: Arc<Mutex<EffectMachine>>,
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
        self.events
            .emit(
                "effect_suspended",
                json!({"cellId": self.cell_id, "nodeId": self.node_id, "action": action}),
            )
            .await?;
        let decision = self.inner.request(action, timeout).await;
        if let Ok(decision) = decision {
            self.machine
                .lock()
                .expect("effect machine lock poisoned")
                .resume(&token)
                .map_err(|error| HostError::HostCall(error.to_string()))?;
            self.events.emit("effect_resumed", json!({"cellId": self.cell_id, "nodeId": self.node_id, "decision": format!("{decision:?}").to_lowercase()})).await?;
            Ok(decision)
        } else {
            decision
        }
    }
}

fn host_error(error: HostError) -> RuntimeError {
    let payload = serde_json::to_value(error.to_payload())
        .expect("HostErrorPayload serialization cannot fail");
    RuntimeError::Effect {
        name: error.sdk_name().into(),
        message: error.to_string(),
        payload: Some(payload),
    }
}

fn runtime_error_payload(error: &RuntimeError) -> Value {
    match error {
        RuntimeError::Effect {
            payload: Some(payload),
            ..
        } => Value::from_json(payload.clone()),
        _ => Value::Record(BTreeMap::from([(
            "message".into(),
            Value::String(error.to_string()),
        )])),
    }
}
fn bounded(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        value.into()
    } else {
        let mut end = bytes.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &value[..end])
    }
}
fn render(value: &Value) -> String {
    match value {
        Value::String(value) | Value::Uri(value) => value.clone(),
        _ => value.to_json().to_string(),
    }
}
fn compare_sort_keys(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => left.cmp(right),
        (Value::Int(left), Value::Decimal(right)) => {
            (*left as f64).partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Decimal(left), Value::Int(right)) => left
            .partial_cmp(&(*right as f64))
            .unwrap_or(Ordering::Equal),
        (Value::Decimal(left), Value::Decimal(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Int(_) | Value::Decimal(_), _) => Ordering::Less,
        (_, Value::Int(_) | Value::Decimal(_)) => Ordering::Greater,
        _ => render(left).cmp(&render(right)),
    }
}
fn error_name(error: &RuntimeError) -> &str {
    match error {
        RuntimeError::Effect { name, .. } => name,
        RuntimeError::Type(_) => "TypeError",
        RuntimeError::Limit(_) => "ResourceLimitError",
        RuntimeError::Cancelled => "CancellationError",
        RuntimeError::Diagnostic(_) => "DiagnosticError",
    }
}

fn logical_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        _ => unreachable!("logical_name only accepts boolean operators"),
    }
}
fn prelude() -> Environment {
    [
        ("print", 1),
        ("display", 2),
        ("length", 1),
        ("lines", 1),
        ("split", 2),
        ("contains", 2),
        ("take", 2),
        ("merge", 2),
        ("map", 2),
        ("flatmap", 2),
        ("filter", 2),
        ("where", 2),
        ("select", 2),
        ("sort_by", 3),
        ("group_by", 2),
        ("aggregate", 2),
        ("sum", 1),
        ("table", 1),
        ("par", 1),
        ("help", 1),
        ("rethrow", 1),
    ]
    .into_iter()
    .map(|(name, arity)| {
        (
            name.into(),
            Value::Callable(Rc::new(Callable::Builtin {
                name: name.into(),
                args: Vec::new(),
                arity,
            })),
        )
    })
    .chain([
        ("asc".into(), Value::String("asc".into())),
        ("desc".into(), Value::String("desc".into())),
    ])
    .collect()
}
fn list_call_args(args: &[Value], name: &str) -> RuntimeResult<(Value, Vec<Value>)> {
    match (&args[0], &args[1]) {
        (function @ Value::Callable(_), Value::List(values)) => {
            Ok((function.clone(), values.clone()))
        }
        _ => Err(RuntimeError::Type(format!(
            "{name} requires function and list"
        ))),
    }
}
fn match_pattern(pattern: &Pattern, value: &Value) -> Option<Environment> {
    let mut env = Environment::new();
    fn walk(pattern: &Pattern, value: &Value, env: &mut Environment) -> bool {
        match (&pattern.node, value) {
            (PatternKind::Wildcard, _) => true,
            (PatternKind::Bind(name), value) => {
                env.insert(name.clone(), value.clone());
                true
            }
            (PatternKind::String(a), Value::String(b)) => a == b,
            (PatternKind::Int(a), Value::Int(b)) => a == b,
            (PatternKind::Bool(a), Value::Bool(b)) => a == b,
            (PatternKind::Null, Value::Null) => true,
            (
                PatternKind::Constructor { name, payload },
                Value::Tagged {
                    name: actual,
                    payload: value,
                },
            ) if name == actual => match (payload, value) {
                (None, None) => true,
                (Some(pattern), Some(value)) => walk(pattern, value, env),
                _ => false,
            },
            (PatternKind::List(patterns), Value::List(values))
                if patterns.len() == values.len() =>
            {
                patterns
                    .iter()
                    .zip(values)
                    .all(|(pattern, value)| walk(pattern, value, env))
            }
            (PatternKind::Cons { head, tail }, Value::List(values)) if !values.is_empty() => {
                walk(head, &values[0], env) && walk(tail, &Value::List(values[1..].to_vec()), env)
            }
            (PatternKind::Record { fields, rest }, Value::Record(values)) => {
                (*rest || fields.len() == values.len())
                    && fields.iter().all(|(name, pattern)| {
                        values
                            .get(name)
                            .is_some_and(|value| walk(pattern, value, env))
                    })
            }
            _ => false,
        }
    }
    walk(pattern, value, &mut env).then_some(env)
}
fn unary(op: UnaryOp, value: Value) -> RuntimeResult<Value> {
    match (op, value) {
        (UnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnaryOp::Negate, Value::Int(value)) => value
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type("integer overflow in negation".into())),
        (UnaryOp::Negate, Value::Decimal(value)) => Ok(Value::Decimal(-value)),
        (_, value) => Err(RuntimeError::Type(format!(
            "invalid unary operand {}",
            value.kind()
        ))),
    }
}
fn binary(op: BinaryOp, left: Value, right: Value) -> RuntimeResult<Value> {
    use BinaryOp::*;
    match op {
        Equal => Ok(Value::Bool(left == right)),
        NotEqual => Ok(Value::Bool(left != right)),
        And | Or => match (left, right) {
            (Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(match op {
                And => left && right,
                Or => left || right,
                _ => unreachable!(),
            })),
            (left, right) => Err(RuntimeError::Type(format!(
                "{} requires boolean operands; found {} and {}",
                logical_name(op),
                left.kind(),
                right.kind()
            ))),
        },
        Cons => match right {
            Value::List(mut values) => {
                values.insert(0, left);
                Ok(Value::List(values))
            }
            _ => Err(RuntimeError::Type(":: tail must be a list".into())),
        },
        Add => match (left, right) {
            (Value::String(mut left), Value::String(right)) => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (left, right) => {
                numeric_binary(left, right, i64::checked_add, |a, b| a + b, "addition")
            }
        },
        Subtract => numeric_binary(left, right, i64::checked_sub, |a, b| a - b, "subtraction"),
        Multiply => numeric_binary(
            left,
            right,
            i64::checked_mul,
            |a, b| a * b,
            "multiplication",
        ),
        Divide if right == Value::Int(0) || right == Value::Decimal(0.0) => {
            Err(RuntimeError::Type("division by zero".into()))
        }
        Divide => numeric_binary(left, right, i64::checked_div, |a, b| a / b, "division"),
        Modulo if right == Value::Int(0) || right == Value::Decimal(0.0) => {
            Err(RuntimeError::Type("modulo by zero".into()))
        }
        Modulo => numeric_binary(left, right, i64::checked_rem, |a, b| a % b, "modulo"),
        Less | LessEqual | Greater | GreaterEqual => {
            let (a, b) = numbers(left, right)?;
            Ok(Value::Bool(match op {
                Less => a < b,
                LessEqual => a <= b,
                Greater => a > b,
                GreaterEqual => a >= b,
                _ => unreachable!(),
            }))
        }
    }
}
fn numeric_binary(
    left: Value,
    right: Value,
    ints: impl FnOnce(i64, i64) -> Option<i64>,
    decimals: impl FnOnce(f64, f64) -> f64,
    operation: &str,
) -> RuntimeResult<Value> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => ints(a, b)
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::Type(format!("integer overflow in {operation}"))),
        (a, b) => {
            let (a, b) = numbers(a, b)?;
            Ok(Value::Decimal(decimals(a, b)))
        }
    }
}
fn numbers(left: Value, right: Value) -> RuntimeResult<(f64, f64)> {
    let number = |value| match value {
        Value::Int(v) => Some(v as f64),
        Value::Decimal(v) => Some(v),
        _ => None,
    };
    let a = number(left).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    let b = number(right).ok_or_else(|| RuntimeError::Type("expected number".into()))?;
    Ok((a, b))
}

fn normalize_effect_args(name: &str, argument: Value) -> JsonValue {
    match (name, argument) {
        ("fs.read", Value::String(path) | Value::Uri(path)) => json!({"path": path}),
        (_, argument) => argument.to_json(),
    }
}

fn has_artifact_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            fields
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|uri| {
                    uri.strip_prefix("artifact://").is_some_and(|id| {
                        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                    })
                })
                || fields.values().any(has_artifact_reference)
        }
        serde_json::Value::Array(values) => values.iter().any(has_artifact_reference),
        _ => false,
    }
}

fn group_key(value: &Value) -> RuntimeResult<String> {
    fn append(value: &Value, key: &mut String) -> RuntimeResult<()> {
        match value {
            Value::Null => key.push('n'),
            Value::Bool(value) => key.push_str(if *value { "b1" } else { "b0" }),
            Value::Int(value) => key.push_str(&format!("i{value}")),
            Value::Decimal(value) => key.push_str(&format!("d{:016x}", value.to_bits())),
            Value::String(value) => {
                key.push('s');
                key.push_str(
                    &serde_json::to_string(value).expect("string serialization cannot fail"),
                );
            }
            Value::Uri(value) => {
                key.push('u');
                key.push_str(&serde_json::to_string(value).expect("URI serialization cannot fail"));
            }
            Value::List(values) => {
                key.push_str("l[");
                for value in values {
                    append(value, key)?;
                    key.push(';');
                }
                key.push(']');
            }
            Value::Record(fields) => {
                key.push_str("r{");
                for (name, value) in fields {
                    key.push_str(
                        &serde_json::to_string(name).expect("field serialization cannot fail"),
                    );
                    key.push(':');
                    append(value, key)?;
                    key.push(';');
                }
                key.push('}');
            }
            Value::Tagged { name, payload } => {
                key.push('t');
                key.push_str(&serde_json::to_string(name).expect("tag serialization cannot fail"));
                if let Some(payload) = payload {
                    key.push(':');
                    append(payload, key)?;
                }
            }
            Value::Callable(_) => {
                return Err(RuntimeError::Type(
                    "group_by key cannot be a function".into(),
                ));
            }
        }
        Ok(())
    }

    let mut key = String::new();
    append(value, &mut key)?;
    Ok(key)
}

pub fn catalog_from_registry(
    registry: &HostRegistry,
    invocation: &InvocationCtx,
    schemes: impl IntoIterator<Item = String>,
) -> CapabilityCatalog {
    let mut catalog = CapabilityCatalog::new();
    for scheme in schemes {
        catalog = catalog.scheme(scheme);
    }
    for tool in registry.search("", None, usize::MAX, invocation) {
        if !tool.granted {
            continue;
        }
        if let Ok(docs) = registry.docs(&tool.name, invocation) {
            let mut signature = EffectSignature::new(&tool.name, ValueType::Any, ValueType::Any);
            signature.errors = docs.errors.into_iter().map(|error| error.name).collect();
            signature.approval = docs.approval;
            signature.sensitive = docs.sensitive;
            signature.resumable = signature.approval != "none";
            catalog = catalog.register(signature).allow(tool.name);
        }
    }
    catalog
}
