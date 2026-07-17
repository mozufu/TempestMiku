use super::*;
pub(super) struct RetainedSizer {
    size: usize,
    limit: usize,
    depth: usize,
    max_depth: usize,
    callables: BTreeSet<usize>,
}

impl RetainedSizer {
    fn new(limit: usize, max_depth: usize) -> Self {
        Self {
            size: 0,
            limit,
            depth: 0,
            max_depth,
            callables: BTreeSet::new(),
        }
    }

    fn add(&mut self, bytes: usize) -> bool {
        self.size = self.size.saturating_add(bytes);
        self.size <= self.limit
    }

    fn entry(&mut self, name: &str, value: &Value) -> bool {
        self.add(MAP_SLOT_OVERHEAD.saturating_add(name.len())) && self.value(value)
    }

    fn enter_depth(&mut self) -> bool {
        if self.depth >= self.max_depth {
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn environment(&mut self, environment: &Environment) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = self.add(std::mem::size_of::<Environment>())
            && environment
                .iter()
                .all(|(name, value)| self.entry(name, value));
        self.leave_depth();
        within_budget
    }

    fn pattern_heap(&mut self, pattern: &Pattern) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &pattern.node {
            PatternKind::Wildcard
            | PatternKind::Int(_)
            | PatternKind::Bool(_)
            | PatternKind::Null => true,
            PatternKind::Bind(value) | PatternKind::String(value) => self.add(value.capacity()),
            PatternKind::Constructor { name, payload } => {
                self.add(name.capacity())
                    && payload.as_deref().is_none_or(|payload| {
                        self.add(std::mem::size_of::<Pattern>()) && self.pattern_heap(payload)
                    })
            }
            PatternKind::List(values) => {
                self.add(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Pattern>()),
                ) && values.iter().all(|value| self.pattern_heap(value))
            }
            PatternKind::Cons { head, tail } => {
                self.add(2usize.saturating_mul(std::mem::size_of::<Pattern>()))
                    && self.pattern_heap(head)
                    && self.pattern_heap(tail)
            }
            PatternKind::Record { fields, .. } => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, Pattern)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.pattern_heap(value))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn type_term_heap(&mut self, term: &TypeTerm) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match term {
            TypeTerm::Named(name) => self.add(name.capacity()),
            TypeTerm::List(value) | TypeTerm::Option(value) => {
                self.add(std::mem::size_of::<TypeTerm>()) && self.type_term_heap(value)
            }
            TypeTerm::Record(fields) => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, TypeTerm)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.type_term_heap(value))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn type_decl_heap(&mut self, decl: &TypeDecl) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = self.add(decl.name.capacity())
            && self.add(
                decl.variants
                    .capacity()
                    .saturating_mul(std::mem::size_of::<crate::VariantDecl>()),
            )
            && decl.variants.iter().all(|variant| {
                self.add(variant.name.capacity())
                    && variant
                        .payload
                        .as_ref()
                        .is_none_or(|payload| self.type_term_heap(payload))
            });
        self.leave_depth();
        within_budget
    }

    fn match_arm_heap(&mut self, arm: &MatchArm) -> bool {
        self.pattern_heap(&arm.pattern) && self.expr_heap(&arm.value)
    }

    fn form_heap(&mut self, form: &Form) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &form.node {
            FormKind::Type(decl) => self.type_decl_heap(decl),
            FormKind::Let { pattern, value } => self.pattern_heap(pattern) && self.expr_heap(value),
            FormKind::Fun { name, params, body } => {
                self.add(name.capacity())
                    && self.add(
                        params
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Pattern>()),
                    )
                    && params.iter().all(|param| self.pattern_heap(param))
                    && self.expr_heap(body)
            }
            FormKind::Expr(expr) => self.expr_heap(expr),
        };
        self.leave_depth();
        within_budget
    }

    fn boxed_expr_heap(&mut self, expr: &Expr) -> bool {
        self.add(std::mem::size_of::<Expr>()) && self.expr_heap(expr)
    }

    fn expr_heap(&mut self, expr: &Expr) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = match &expr.node {
            ExprKind::String(value)
            | ExprKind::Uri(value)
            | ExprKind::Name(value)
            | ExprKind::Constructor(value)
            | ExprKind::Capability(value) => self.add(value.capacity()),
            ExprKind::Int(_) | ExprKind::Decimal(_) | ExprKind::Bool(_) | ExprKind::Null => true,
            ExprKind::List(values) => {
                self.add(
                    values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Expr>()),
                ) && values.iter().all(|value| self.expr_heap(value))
            }
            ExprKind::Record(fields) => {
                self.add(
                    fields
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, Expr)>()),
                ) && fields
                    .iter()
                    .all(|(name, value)| self.add(name.capacity()) && self.expr_heap(value))
            }
            ExprKind::Lambda { params, body } => {
                self.add(
                    params
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Pattern>()),
                ) && params.iter().all(|param| self.pattern_heap(param))
                    && self.boxed_expr_heap(body)
            }
            ExprKind::Apply { function, argument } => {
                self.boxed_expr_heap(function) && self.boxed_expr_heap(argument)
            }
            ExprKind::Field { target, field } => {
                self.boxed_expr_heap(target) && self.add(field.capacity())
            }
            ExprKind::Unary { value, .. } => self.boxed_expr_heap(value),
            ExprKind::Binary { left, right, .. }
            | ExprKind::Pipe {
                value: left,
                target: right,
            } => self.boxed_expr_heap(left) && self.boxed_expr_heap(right),
            ExprKind::If {
                condition,
                then_value,
                else_value,
            } => {
                self.boxed_expr_heap(condition)
                    && self.boxed_expr_heap(then_value)
                    && self.boxed_expr_heap(else_value)
            }
            ExprKind::Match { value, arms } | ExprKind::Handle { value, arms } => {
                self.boxed_expr_heap(value)
                    && self.add(
                        arms.capacity()
                            .saturating_mul(std::mem::size_of::<MatchArm>()),
                    )
                    && arms.iter().all(|arm| self.match_arm_heap(arm))
            }
            ExprKind::Block(forms) => {
                self.add(forms.capacity().saturating_mul(std::mem::size_of::<Form>()))
                    && forms.iter().all(|form| self.form_heap(form))
            }
        };
        self.leave_depth();
        within_budget
    }

    fn value(&mut self, value: &Value) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = (|| {
            if !self.add(std::mem::size_of::<Value>()) {
                return false;
            }
            match value {
                Value::Null | Value::Bool(_) | Value::Int(_) | Value::Decimal(_) => true,
                Value::String(value) | Value::Uri(value) => self.add(value.capacity()),
                Value::List(values) => {
                    self.add(
                        values
                            .capacity()
                            .saturating_sub(values.len())
                            .saturating_mul(std::mem::size_of::<Value>()),
                    ) && values.iter().all(|value| self.value(value))
                }
                Value::Record(fields) => {
                    self.add(std::mem::size_of::<Environment>())
                        && fields.iter().all(|(name, value)| self.entry(name, value))
                }
                Value::Tagged { name, payload } => {
                    self.add(name.capacity())
                        && payload.as_deref().is_none_or(|payload| {
                            self.add(std::mem::size_of::<Value>()) && self.value(payload)
                        })
                }
                Value::Callable(callable) => self.callable(callable),
            }
        })();
        self.leave_depth();
        within_budget
    }

    fn callable(&mut self, callable: &Rc<Callable>) -> bool {
        if !self.enter_depth() {
            return false;
        }
        let within_budget = (|| {
            let pointer = Rc::as_ptr(callable) as usize;
            if !self.callables.insert(pointer) {
                return true;
            }
            if !self.add(
                std::mem::size_of::<Callable>().saturating_add(2 * std::mem::size_of::<usize>()),
            ) {
                return false;
            }
            match callable.as_ref() {
                Callable::Builtin { name, args, .. } => {
                    self.add(name.capacity()) && args.iter().all(|value| self.value(value))
                }
                Callable::BuiltinDataLast {
                    name, args, data, ..
                } => {
                    self.add(name.capacity())
                        && args.iter().all(|value| self.value(value))
                        && self.value(data)
                }
                Callable::User {
                    params,
                    body,
                    captured,
                    args,
                } => {
                    self.add(
                        params
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Pattern>()),
                    ) && params.iter().all(|param| self.pattern_heap(param))
                        && self.expr_heap(body)
                        && self.environment(captured)
                        && args.iter().all(|value| self.value(value))
                }
                Callable::Row { body, captured } => {
                    self.expr_heap(body) && self.environment(captured)
                }
                Callable::Capability { name } | Callable::Constructor { name } => {
                    self.add(name.capacity())
                }
            }
        })();
        self.leave_depth();
        within_budget
    }
}

pub(super) fn value_size_bounded(value: &Value, limit: usize, max_depth: usize) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    if sizer.value(value) {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

pub(super) fn environment_size_bounded(
    environment: &Environment,
    limit: usize,
    max_depth: usize,
) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    if sizer.environment(environment) {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

pub(super) fn merged_environment_size_bounded(
    base: &Environment,
    overlay: &Environment,
    committed: &BTreeSet<String>,
    limit: usize,
    max_depth: usize,
) -> usize {
    let mut sizer = RetainedSizer::new(limit, max_depth);
    let within_budget = sizer.add(std::mem::size_of::<Environment>())
        && base
            .iter()
            .filter(|(name, _)| !committed.contains(*name))
            .all(|(name, value)| sizer.entry(name, value))
        && committed.iter().all(|name| {
            overlay
                .get(name)
                .is_some_and(|value| sizer.entry(name, value))
        });
    if within_budget {
        sizer.size
    } else {
        limit.saturating_add(1)
    }
}

pub(super) fn environment_clone_size_bounded(
    environment: &Environment,
    limit: usize,
    max_depth: usize,
) -> usize {
    fn add(size: &mut usize, bytes: usize, limit: usize) -> bool {
        *size = size.saturating_add(bytes);
        *size <= limit
    }

    fn value_heap(
        value: &Value,
        size: &mut usize,
        limit: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            return false;
        }
        match value {
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Decimal(_) => true,
            Value::String(value) | Value::Uri(value) => add(size, value.len(), limit),
            Value::List(values) => {
                add(
                    size,
                    values.len().saturating_mul(std::mem::size_of::<Value>()),
                    limit,
                ) && values
                    .iter()
                    .all(|value| value_heap(value, size, limit, depth + 1, max_depth))
            }
            Value::Record(fields) => fields.iter().all(|(name, value)| {
                add(size, MAP_SLOT_OVERHEAD.saturating_add(name.len()), limit)
                    && value_heap(value, size, limit, depth + 1, max_depth)
            }),
            Value::Tagged { name, payload } => {
                add(size, name.len(), limit)
                    && payload.as_deref().is_none_or(|payload| {
                        add(size, std::mem::size_of::<Value>(), limit)
                            && value_heap(payload, size, limit, depth + 1, max_depth)
                    })
            }
            Value::Callable(_) => true,
        }
    }

    let mut size = 0usize;
    let within_budget = environment.iter().all(|(name, value)| {
        add(
            &mut size,
            MAP_SLOT_OVERHEAD.saturating_add(name.len()),
            limit,
        ) && value_heap(value, &mut size, limit, 0, max_depth)
    });
    if within_budget {
        size
    } else {
        limit.saturating_add(1)
    }
}

pub(super) fn json_value_size_bounded(value: &JsonValue, limit: usize, max_depth: usize) -> usize {
    fn walk(
        value: &JsonValue,
        size: &mut usize,
        limit: usize,
        depth: usize,
        max_depth: usize,
    ) -> bool {
        if depth >= max_depth {
            return false;
        }
        *size = size.saturating_add(std::mem::size_of::<JsonValue>());
        if *size > limit {
            return false;
        }
        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => true,
            JsonValue::String(value) => {
                *size = size.saturating_add(value.capacity());
                *size <= limit
            }
            JsonValue::Array(values) => values
                .iter()
                .all(|value| walk(value, size, limit, depth + 1, max_depth)),
            JsonValue::Object(fields) => fields.iter().all(|(name, value)| {
                *size = size
                    .saturating_add(MAP_SLOT_OVERHEAD)
                    .saturating_add(name.len());
                *size <= limit && walk(value, size, limit, depth + 1, max_depth)
            }),
        }
    }

    let mut size = 0usize;
    if walk(value, &mut size, limit, 0, max_depth) {
        size
    } else {
        limit.saturating_add(1)
    }
}

pub(super) fn group_key(value: &Value, limit: usize, max_depth: usize) -> RuntimeResult<String> {
    fn write_key(writer: &mut BudgetWriter, bytes: &[u8]) -> RuntimeResult<()> {
        writer
            .write_all(bytes)
            .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))
    }

    fn append(
        value: &Value,
        writer: &mut BudgetWriter,
        depth: usize,
        max_depth: usize,
    ) -> RuntimeResult<()> {
        if depth >= max_depth {
            return Err(RuntimeError::Limit(
                "group key nesting budget exceeded".into(),
            ));
        }
        match value {
            Value::Null => write_key(writer, b"n")?,
            Value::Bool(value) => write_key(writer, if *value { b"b1" } else { b"b0" })?,
            Value::Int(value) => write!(writer, "i{value}")
                .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?,
            Value::Decimal(value) => write!(writer, "d{:016x}", value.to_bits())
                .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?,
            Value::String(value) => {
                write_key(writer, b"s")?;
                serde_json::to_writer(writer, value)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
            }
            Value::Uri(value) => {
                write_key(writer, b"u")?;
                serde_json::to_writer(writer, value)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
            }
            Value::List(values) => {
                write_key(writer, b"l[")?;
                for value in values {
                    append(value, writer, depth + 1, max_depth)?;
                    write_key(writer, b";")?;
                }
                write_key(writer, b"]")?;
            }
            Value::Record(fields) => {
                write_key(writer, b"r{")?;
                for (name, value) in fields {
                    serde_json::to_writer(&mut *writer, name).map_err(|_| {
                        RuntimeError::Limit("group key value budget exceeded".into())
                    })?;
                    write_key(writer, b":")?;
                    append(value, writer, depth + 1, max_depth)?;
                    write_key(writer, b";")?;
                }
                write_key(writer, b"}")?;
            }
            Value::Tagged { name, payload } => {
                write_key(writer, b"t")?;
                serde_json::to_writer(&mut *writer, name)
                    .map_err(|_| RuntimeError::Limit("group key value budget exceeded".into()))?;
                if let Some(payload) = payload {
                    write_key(writer, b":")?;
                    append(payload, writer, depth + 1, max_depth)?;
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

    let mut writer = BudgetWriter::new(limit, true);
    append(value, &mut writer, 0, max_depth)?;
    let (_, key, exceeded) = writer.finish();
    if exceeded {
        Err(RuntimeError::Limit(
            "group key value budget exceeded".into(),
        ))
    } else {
        Ok(key)
    }
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
