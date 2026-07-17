use super::*;

impl Checker<'_> {
    pub(super) fn match_expr(
        &mut self,
        expr: &Expr,
        value: &Expr,
        arms: &[MatchArm],
    ) -> Result<Fact> {
        let value = self.expr(value)?;
        let mut effects = value.effects;
        let mut ty = ValueType::Never;
        let mut covered = BTreeSet::new();
        let mut catch_all = false;
        for arm in arms {
            self.check_pattern(&arm.pattern, &value.ty)?;
            match &arm.pattern.node {
                PatternKind::Wildcard | PatternKind::Bind(_) => catch_all = true,
                PatternKind::Constructor { name, .. } => {
                    covered.insert(name.clone());
                }
                PatternKind::Bool(value) => {
                    covered.insert(value.to_string());
                }
                _ => {}
            }
            let arm = self.with_scope(|checker| {
                for (name, ty) in checker.pattern_bindings(&arm.pattern, &value.ty)? {
                    checker.env.insert(name, ty);
                }
                checker.expr(&arm.value)
            })?;
            ty = unify(ty, arm.ty);
            effects = effects.union(arm.effects);
        }
        if !catch_all {
            let expected: BTreeSet<String> = match &value.ty {
                ValueType::Bool => ["true".into(), "false".into()].into_iter().collect(),
                ValueType::Local(name) => self
                    .local_types
                    .get(name)
                    .map(|variants| variants.keys().cloned().collect())
                    .unwrap_or_default(),
                _ => BTreeSet::new(),
            };
            if !expected.is_empty() && !expected.is_subset(&covered) {
                return Err(Diagnostic::new(
                    "TM3015",
                    format!(
                        "non-exhaustive match; missing {:?}",
                        expected.difference(&covered).collect::<Vec<_>>()
                    ),
                    expr.span,
                    self.source,
                ));
            }
        }
        Ok(Fact { ty, effects })
    }

    pub(super) fn check_pattern(&self, pattern: &Pattern, expected: &ValueType) -> Result<()> {
        pattern_names(self.source, pattern)?;

        fn mismatch(
            checker: &Checker<'_>,
            pattern: &Pattern,
            expected: &ValueType,
            detail: impl Into<String>,
        ) -> Diagnostic {
            Diagnostic::new(
                "TM3022",
                format!("pattern cannot match {expected:?}: {}", detail.into()),
                pattern.span,
                checker.source,
            )
        }

        fn walk(checker: &Checker<'_>, pattern: &Pattern, expected: &ValueType) -> Result<()> {
            match &pattern.node {
                PatternKind::Wildcard | PatternKind::Bind(_) => Ok(()),
                PatternKind::String(_)
                    if matches!(expected, ValueType::Any | ValueType::String) =>
                {
                    Ok(())
                }
                PatternKind::Int(_) if matches!(expected, ValueType::Any | ValueType::Int) => {
                    Ok(())
                }
                PatternKind::Bool(_) if matches!(expected, ValueType::Any | ValueType::Bool) => {
                    Ok(())
                }
                PatternKind::Null if matches!(expected, ValueType::Any | ValueType::Null) => Ok(()),
                PatternKind::Constructor { payload, .. } if expected == &ValueType::Any => {
                    if let Some(payload) = payload {
                        walk(checker, payload, &ValueType::Any)?;
                    }
                    Ok(())
                }
                PatternKind::Constructor { name, payload } => {
                    let Some((owner, payload_type)) = checker.constructors.get(name) else {
                        return Err(mismatch(checker, pattern, expected, "unknown constructor"));
                    };
                    if expected != &ValueType::Local(owner.clone()) {
                        return Err(mismatch(
                            checker,
                            pattern,
                            expected,
                            format!("constructor {name} belongs to {owner}"),
                        ));
                    }
                    match (payload, payload_type) {
                        (Some(pattern), Some(expected)) => walk(checker, pattern, expected),
                        (None, None) => Ok(()),
                        _ => Err(mismatch(
                            checker,
                            pattern,
                            expected,
                            format!("constructor {name} payload shape differs"),
                        )),
                    }
                }
                PatternKind::List(patterns) => {
                    let element = match expected {
                        ValueType::Any => &ValueType::Any,
                        ValueType::List(element) => element.as_ref(),
                        _ => return Err(mismatch(checker, pattern, expected, "expected a list")),
                    };
                    for pattern in patterns {
                        walk(checker, pattern, element)?;
                    }
                    Ok(())
                }
                PatternKind::Cons { head, tail } => {
                    let element = match expected {
                        ValueType::Any => &ValueType::Any,
                        ValueType::List(element) => element.as_ref(),
                        _ => return Err(mismatch(checker, pattern, expected, "expected a list")),
                    };
                    walk(checker, head, element)?;
                    walk(checker, tail, expected)
                }
                PatternKind::Record { fields, rest } => {
                    let expected_fields = match expected {
                        ValueType::Any => {
                            for (_, pattern) in fields {
                                walk(checker, pattern, &ValueType::Any)?;
                            }
                            return Ok(());
                        }
                        ValueType::Record(fields) => fields,
                        _ => return Err(mismatch(checker, pattern, expected, "expected a record")),
                    };
                    if !rest && fields.len() != expected_fields.len() {
                        return Err(mismatch(
                            checker,
                            pattern,
                            expected,
                            "exact record pattern has a different field count",
                        ));
                    }
                    for (name, pattern) in fields {
                        let Some(expected) = expected_fields.get(name) else {
                            return Err(mismatch(
                                checker,
                                pattern,
                                expected,
                                format!("record has no field {name}"),
                            ));
                        };
                        walk(checker, pattern, expected)?;
                    }
                    Ok(())
                }
                _ => Err(mismatch(
                    checker,
                    pattern,
                    expected,
                    "literal has an incompatible type",
                )),
            }
        }

        walk(self, pattern, expected)
    }

    pub(super) fn pattern_bindings(
        &self,
        pattern: &Pattern,
        expected: &ValueType,
    ) -> Result<BTreeMap<String, ValueType>> {
        fn walk(
            checker: &Checker<'_>,
            pattern: &Pattern,
            expected: &ValueType,
            bindings: &mut BTreeMap<String, ValueType>,
        ) {
            match &pattern.node {
                PatternKind::Bind(name) => {
                    bindings.insert(name.clone(), expected.clone());
                }
                PatternKind::Constructor {
                    name,
                    payload: Some(payload),
                } => {
                    let payload_type = checker
                        .constructors
                        .get(name)
                        .and_then(|(_, payload)| payload.as_ref())
                        .cloned()
                        .unwrap_or(ValueType::Any);
                    walk(checker, payload, &payload_type, bindings);
                }
                PatternKind::List(values) => {
                    let element = match expected {
                        ValueType::List(element) => element.as_ref(),
                        _ => &ValueType::Any,
                    };
                    for value in values {
                        walk(checker, value, element, bindings);
                    }
                }
                PatternKind::Cons { head, tail } => {
                    let element = match expected {
                        ValueType::List(element) => element.as_ref(),
                        _ => &ValueType::Any,
                    };
                    walk(checker, head, element, bindings);
                    walk(checker, tail, expected, bindings);
                }
                PatternKind::Record { fields, .. } => {
                    for (name, value) in fields {
                        let field_type = match expected {
                            ValueType::Record(expected_fields) => {
                                expected_fields.get(name).unwrap_or(&ValueType::Any)
                            }
                            _ => &ValueType::Any,
                        };
                        walk(checker, value, field_type, bindings);
                    }
                }
                _ => {}
            }
        }

        self.check_pattern(pattern, expected)?;
        let mut bindings = BTreeMap::new();
        walk(self, pattern, expected, &mut bindings);
        Ok(bindings)
    }
}
