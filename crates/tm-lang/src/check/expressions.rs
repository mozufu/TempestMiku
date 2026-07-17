use super::*;

impl Checker<'_> {
    pub(super) fn expr(&mut self, expr: &Expr) -> Result<Fact> {
        self.context.enter_expr(expr, self.source)?;
        let result = self.expr_inner(expr);
        self.context.exit_expr();
        result
    }

    pub(super) fn expr_inner(&mut self, expr: &Expr) -> Result<Fact> {
        match &expr.node {
            ExprKind::String(value) => {
                let mut fact = pure(ValueType::String);
                for interpolation in interpolations(value) {
                    match interpolation {
                        Interpolation::Name(name) => {
                            if !self.env.contains_key(&name) && !self.row_context {
                                return Err(Diagnostic::new(
                                    "TM3020",
                                    format!("unbound interpolation name {name}"),
                                    expr.span,
                                    self.source,
                                ));
                            }
                        }
                        Interpolation::Expression(source) => {
                            let cell = self.context.parse_interpolation(&source)?;
                            if !matches!(
                                cell.forms.as_slice(),
                                [Form {
                                    node: FormKind::Expr(_),
                                    ..
                                }]
                            ) {
                                return Err(Diagnostic::new(
                                    "TM3021",
                                    "interpolation must contain one expression",
                                    expr.span,
                                    self.source,
                                ));
                            }
                            let mut checker = Checker::with_context(
                                &source,
                                self.catalog,
                                Rc::clone(&self.context),
                            );
                            checker.env.extend(
                                self.env.keys().cloned().map(|name| (name, ValueType::Any)),
                            );
                            let checked = checker.cell(&cell)?;
                            fact.effects = fact.effects.union(checked.effects);
                        }
                    }
                }
                Ok(fact)
            }
            ExprKind::Int(_) => Ok(pure(ValueType::Int)),
            ExprKind::Decimal(_) => Ok(pure(ValueType::Decimal)),
            ExprKind::Bool(_) => Ok(pure(ValueType::Bool)),
            ExprKind::Null => Ok(pure(ValueType::Null)),
            ExprKind::Uri(uri) => {
                let scheme = uri.split(':').next().unwrap_or_default();
                if !self.catalog.has_scheme(scheme) {
                    return Err(Diagnostic::new(
                        "TM3005",
                        format!("unknown resource scheme {scheme}"),
                        expr.span,
                        self.source,
                    ));
                }
                Ok(pure(ValueType::Uri))
            }
            ExprKind::Name(name) => {
                let ty = self
                    .env
                    .get(name)
                    .cloned()
                    .or_else(|| self.row_context.then_some(ValueType::Any))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "TM3006",
                            format!("unbound name {name}"),
                            expr.span,
                            self.source,
                        )
                    })?;
                let mut fact = pure(ty);
                if name == "display" {
                    fact.effects.presentation.insert("display".into());
                } else if name == "print" {
                    fact.effects.errors.insert("ResourceLimitError".into());
                } else if name == "rethrow" {
                    fact.effects.errors.extend(
                        self.rethrow_errors
                            .clone()
                            .unwrap_or_else(|| BTreeSet::from(["Rethrown".into()])),
                    );
                }
                Ok(fact)
            }
            ExprKind::Constructor(name) => {
                let Some((local, payload)) = self.constructors.get(name) else {
                    if matches!(name.as_str(), "Some" | "None") {
                        return Ok(pure(ValueType::Any));
                    }
                    return Err(Diagnostic::new(
                        "TM3007",
                        format!("unknown constructor {name}"),
                        expr.span,
                        self.source,
                    ));
                };
                let result = ValueType::Local(local.clone());
                Ok(pure(payload.clone().map_or(result.clone(), |payload| {
                    ValueType::Function(Box::new(payload), Box::new(result))
                })))
            }
            ExprKind::Capability(name) => {
                let signature = self.catalog.effect(name).ok_or_else(|| {
                    Diagnostic::new(
                        "TM3008",
                        format!("unknown capability {name}"),
                        expr.span,
                        self.source,
                    )
                })?;
                if !self.catalog.permits(name) {
                    return Err(Diagnostic::new(
                        "TM3009",
                        format!("ungranted capability {name}"),
                        expr.span,
                        self.source,
                    ));
                }
                let mut effects = EffectRows::default();
                effects.authority.insert(name.clone());
                effects.errors.extend(signature.errors.clone());
                Ok(Fact {
                    ty: ValueType::Function(
                        Box::new(signature.args.clone()),
                        Box::new(signature.result.clone()),
                    ),
                    effects,
                })
            }
            ExprKind::List(values) => {
                let mut effects = EffectRows::default();
                let mut item = None;
                for value in values {
                    let fact = self.expr(value)?;
                    item = Some(match item {
                        Some(item) => unify(item, fact.ty),
                        None => fact.ty,
                    });
                    effects = effects.union(fact.effects);
                }
                Ok(Fact {
                    ty: ValueType::List(Box::new(item.unwrap_or(ValueType::Any))),
                    effects,
                })
            }
            ExprKind::Record(fields) => {
                let mut map = BTreeMap::new();
                let mut effects = EffectRows::default();
                for (name, value) in fields {
                    if map.contains_key(name) {
                        return Err(Diagnostic::new(
                            "TM3010",
                            format!("duplicate record field {name}"),
                            value.span,
                            self.source,
                        ));
                    }
                    let fact = self.expr(value)?;
                    map.insert(name.clone(), fact.ty);
                    effects = effects.union(fact.effects);
                }
                Ok(Fact {
                    ty: ValueType::Record(map),
                    effects,
                })
            }
            ExprKind::Lambda { params, body } => {
                self.ensure_function_arity(params.len(), expr.span)?;
                let body = self.with_scope(|checker| {
                    for param in params {
                        for name in pattern_names(checker.source, param)? {
                            checker.env.insert(name, ValueType::Any);
                        }
                    }
                    checker.expr(body)
                })?;
                let mut ty = body.ty;
                for _ in params.iter().rev() {
                    ty = ValueType::Function(Box::new(ValueType::Any), Box::new(ty));
                }
                Ok(Fact {
                    ty,
                    effects: body.effects,
                })
            }
            ExprKind::Apply { function, argument } => {
                let row_argument = matches!(
                    &function.node,
                    ExprKind::Name(name)
                        if matches!(name.as_str(), "where" | "select" | "sort_by" | "group_by" | "aggregate")
                );
                let function = self.expr(function)?;
                let previous_row_context = self.row_context;
                self.row_context |= row_argument;
                let argument = self.expr(argument);
                self.row_context = previous_row_context;
                let argument = argument?;
                let ty = match function.ty {
                    ValueType::Function(expected, result) => {
                        ensure_assignable(self.source, expr, &expected, &argument.ty)?;
                        *result
                    }
                    ValueType::Any => ValueType::Any,
                    other => {
                        return Err(Diagnostic::new(
                            "TM3011",
                            format!("cannot apply {other:?}"),
                            expr.span,
                            self.source,
                        ));
                    }
                };
                Ok(Fact {
                    ty,
                    effects: function.effects.union(argument.effects),
                })
            }
            ExprKind::Field { target, field } => {
                let target = self.expr(target)?;
                let ty = match &target.ty {
                    ValueType::Record(fields) => fields.get(field).cloned().ok_or_else(|| {
                        Diagnostic::new(
                            "TM3012",
                            format!("record has no field {field}"),
                            expr.span,
                            self.source,
                        )
                    })?,
                    ValueType::Any => ValueType::Any,
                    _ => {
                        return Err(Diagnostic::new(
                            "TM3013",
                            "field access requires a record",
                            expr.span,
                            self.source,
                        ));
                    }
                };
                Ok(Fact {
                    ty,
                    effects: target.effects,
                })
            }
            ExprKind::Unary { op, value } => {
                let fact = self.expr(value)?;
                let expected = if *op == UnaryOp::Not {
                    ValueType::Bool
                } else {
                    ValueType::Decimal
                };
                if !numeric_compatible(&expected, &fact.ty) {
                    ensure_assignable(self.source, expr, &expected, &fact.ty)?;
                }
                Ok(Fact {
                    ty: if *op == UnaryOp::Not {
                        ValueType::Bool
                    } else {
                        fact.ty
                    },
                    effects: fact.effects,
                })
            }
            ExprKind::Binary { op, left, right } => self.binary(expr, *op, left, right),
            ExprKind::Pipe { value, target } => {
                let piped = self.expr(value)?;
                let target = self.expr(target)?;
                let ty = apply_last_type(self.source, expr, target.ty, &piped.ty)?;
                Ok(Fact {
                    ty,
                    effects: piped.effects.union(target.effects),
                })
            }
            ExprKind::If {
                condition,
                then_value,
                else_value,
            } => {
                let condition = self.expr(condition)?;
                ensure_assignable(self.source, expr, &ValueType::Bool, &condition.ty)?;
                let yes = self.expr(then_value)?;
                let no = self.expr(else_value)?;
                Ok(Fact {
                    ty: unify(yes.ty, no.ty),
                    effects: condition.effects.union(yes.effects).union(no.effects),
                })
            }
            ExprKind::Match { value, arms } => self.match_expr(expr, value, arms),
            ExprKind::Handle { value, arms } => {
                let mut fact = self.expr(value)?;
                let original_errors = fact.effects.errors.clone();
                let mut arm_effects = EffectRows::default();
                let mut ty = fact.ty.clone();
                let mut handled = BTreeSet::new();
                let mut catches_all = false;
                for arm in arms {
                    self.check_pattern(&arm.pattern, &ValueType::Any)?;
                    let rethrow_errors = match &arm.pattern.node {
                        PatternKind::Wildcard | PatternKind::Bind(_) => {
                            catches_all = true;
                            original_errors.clone()
                        }
                        PatternKind::Constructor { name, .. } => {
                            handled.insert(name.clone());
                            BTreeSet::from([name.clone()])
                        }
                        _ => BTreeSet::new(),
                    };
                    let old_rethrow_errors = self.rethrow_errors.replace(rethrow_errors);
                    let arm_result = self.with_scope(|checker| {
                        for name in pattern_names(checker.source, &arm.pattern)? {
                            checker.env.insert(name, ValueType::Any);
                        }
                        checker.expr(&arm.value)
                    });
                    self.rethrow_errors = old_rethrow_errors;
                    let arm = arm_result?;
                    ty = unify(ty, arm.ty);
                    arm_effects = arm_effects.union(arm.effects);
                }
                if catches_all {
                    fact.effects.errors.clear();
                } else {
                    fact.effects.errors.retain(|error| !handled.contains(error));
                }
                Ok(Fact {
                    ty,
                    effects: fact.effects.union(arm_effects),
                })
            }
            ExprKind::Block(forms) => self.with_scope(|checker| {
                let mut fact = pure(ValueType::Null);
                for form in forms {
                    let next = checker.form(form, false)?;
                    fact = Fact {
                        ty: next.ty,
                        effects: fact.effects.union(next.effects),
                    };
                }
                Ok(fact)
            }),
        }
    }

    pub(super) fn ensure_function_arity(&self, arity: usize, span: Span) -> Result<()> {
        if arity > MAX_FUNCTION_ARITY {
            Err(Diagnostic::new(
                "TM3023",
                format!(
                    "function arity budget exceeded: {arity} parameters exceeds {MAX_FUNCTION_ARITY}"
                ),
                span,
                self.source,
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn binary(
        &mut self,
        expr: &Expr,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Fact> {
        let left = self.expr(left)?;
        let right = self.expr(right)?;
        let ty = match op {
            BinaryOp::Or | BinaryOp::And => {
                ensure_assignable(self.source, expr, &ValueType::Bool, &left.ty)?;
                ensure_assignable(self.source, expr, &ValueType::Bool, &right.ty)?;
                ValueType::Bool
            }
            BinaryOp::Equal | BinaryOp::NotEqual => ValueType::Bool,
            BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
                if !numeric(&left.ty) || !numeric(&right.ty) {
                    return Err(Diagnostic::new(
                        "TM3014",
                        "ordered comparison requires numbers and cannot be chained",
                        expr.span,
                        self.source,
                    ));
                }
                ValueType::Bool
            }
            BinaryOp::Cons => match &right.ty {
                ValueType::List(item) => {
                    ValueType::List(Box::new(unify(left.ty.clone(), (**item).clone())))
                }
                ValueType::Any => ValueType::List(Box::new(ValueType::Any)),
                _ => {
                    return Err(Diagnostic::new(
                        "TM3014",
                        "cons tail must be a list",
                        expr.span,
                        self.source,
                    ));
                }
            },
            BinaryOp::Add if left.ty == ValueType::String && right.ty == ValueType::String => {
                ValueType::String
            }
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo => {
                if !numeric(&left.ty) || !numeric(&right.ty) {
                    return Err(Diagnostic::new(
                        "TM3014",
                        "numeric operator requires numbers",
                        expr.span,
                        self.source,
                    ));
                }
                unify(left.ty.clone(), right.ty.clone())
            }
        };
        let mut effects = left.effects.union(right.effects);
        if matches!(op, BinaryOp::Divide | BinaryOp::Modulo) {
            effects.errors.insert("DivisionByZero".into());
        }
        Ok(Fact { ty, effects })
    }
}
