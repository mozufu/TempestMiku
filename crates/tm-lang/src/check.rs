use std::collections::{BTreeMap, BTreeSet};

use crate::{
    BinaryOp, CapabilityCatalog, Cell, Diagnostic, Expr, ExprKind, Form, FormKind, MatchArm,
    Pattern, PatternKind, Result, TypeDecl, TypeTerm, UnaryOp, ValueType, parse,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectRows {
    pub authority: BTreeSet<String>,
    pub errors: BTreeSet<String>,
    pub presentation: BTreeSet<String>,
}

impl EffectRows {
    fn union(mut self, other: Self) -> Self {
        self.authority.extend(other.authority);
        self.errors.extend(other.errors);
        self.presentation.extend(other.presentation);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCell {
    pub result_type: ValueType,
    pub effects: EffectRows,
    pub bindings: BTreeMap<String, ValueType>,
}

#[derive(Debug, Clone)]
struct Fact {
    ty: ValueType,
    effects: EffectRows,
}

pub fn check(source: &str, cell: &Cell, catalog: &CapabilityCatalog) -> Result<CheckedCell> {
    Checker::new(source, catalog).cell(cell)
}

pub fn check_with_bindings(
    source: &str,
    cell: &Cell,
    catalog: &CapabilityCatalog,
    bindings: impl IntoIterator<Item = String>,
) -> Result<CheckedCell> {
    let mut checker = Checker::new(source, catalog);
    checker
        .env
        .extend(bindings.into_iter().map(|name| (name, ValueType::Any)));
    checker.cell(cell)
}

struct Checker<'a> {
    source: &'a str,
    catalog: &'a CapabilityCatalog,
    env: BTreeMap<String, ValueType>,
    local_types: BTreeMap<String, BTreeMap<String, Option<ValueType>>>,
    constructors: BTreeMap<String, (String, Option<ValueType>)>,
    row_context: bool,
}

impl<'a> Checker<'a> {
    fn new(source: &'a str, catalog: &'a CapabilityCatalog) -> Self {
        let env = prelude_types();
        Self {
            source,
            catalog,
            env,
            local_types: BTreeMap::new(),
            constructors: BTreeMap::new(),
            row_context: false,
        }
    }

    fn cell(mut self, cell: &Cell) -> Result<CheckedCell> {
        let mut effects = EffectRows::default();
        let mut result_type = ValueType::Null;
        let mut bindings = BTreeMap::new();
        for form in &cell.forms {
            let fact = self.form(form, true)?;
            result_type = fact.ty;
            effects = effects.union(fact.effects);
            match &form.node {
                FormKind::Let { pattern, .. } => {
                    let pattern_bindings = self.pattern_bindings(pattern, &result_type)?;
                    if contains_local(&result_type) {
                        return Err(Diagnostic::new(
                            "TM3001",
                            "local sum type cannot escape in a persistent binding",
                            form.span,
                            self.source,
                        ));
                    }
                    for (name, ty) in pattern_bindings {
                        bindings.insert(name.clone(), ty.clone());
                        self.env.insert(name, ty);
                    }
                }
                FormKind::Fun { name, .. } => {
                    if contains_local(&result_type) {
                        return Err(Diagnostic::new(
                            "TM3002",
                            "local sum type cannot escape in a persistent function",
                            form.span,
                            self.source,
                        ));
                    }
                    bindings.insert(name.clone(), result_type.clone());
                    self.env.insert(name.clone(), result_type.clone());
                }
                _ => {}
            }
        }
        Ok(CheckedCell {
            result_type,
            effects,
            bindings,
        })
    }

    fn form(&mut self, form: &Form, persistent: bool) -> Result<Fact> {
        match &form.node {
            FormKind::Type(decl) => {
                self.declare_type(decl, form)?;
                Ok(pure(ValueType::Null))
            }
            FormKind::Let { pattern, value } => {
                let fact = self.expr(value)?;
                self.check_pattern(pattern, &fact.ty)?;
                if !persistent {
                    for (name, ty) in self.pattern_bindings(pattern, &fact.ty)? {
                        self.env.insert(name, ty);
                    }
                }
                Ok(fact)
            }
            FormKind::Fun { name, params, body } => {
                let old = self.env.clone();
                self.env.insert(name.clone(), ValueType::Any);
                for param in params {
                    for name in pattern_names(self.source, param)? {
                        self.env.insert(name, ValueType::Any);
                    }
                }
                let body = self.expr(body)?;
                self.env = old;
                let mut ty = body.ty;
                for _ in params.iter().rev() {
                    ty = ValueType::Function(Box::new(ValueType::Any), Box::new(ty));
                }
                Ok(Fact {
                    ty,
                    effects: body.effects,
                })
            }
            FormKind::Expr(expr) => self.expr(expr),
        }
    }

    fn declare_type(&mut self, decl: &TypeDecl, form: &Form) -> Result<()> {
        if self.local_types.contains_key(&decl.name) {
            return Err(Diagnostic::new(
                "TM3003",
                format!("duplicate local type {}", decl.name),
                form.span,
                self.source,
            ));
        }
        let mut variants = BTreeMap::new();
        for variant in &decl.variants {
            if variants.contains_key(&variant.name) || self.constructors.contains_key(&variant.name)
            {
                return Err(Diagnostic::new(
                    "TM3004",
                    format!("duplicate constructor {}", variant.name),
                    form.span,
                    self.source,
                ));
            }
            let payload = variant.payload.as_ref().map(type_from_term);
            variants.insert(variant.name.clone(), payload.clone());
            self.constructors
                .insert(variant.name.clone(), (decl.name.clone(), payload));
        }
        self.local_types.insert(decl.name.clone(), variants);
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<Fact> {
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
                            let cell = parse(&source)?;
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
                            let checked = check_with_bindings(
                                &source,
                                &cell,
                                self.catalog,
                                self.env.keys().cloned(),
                            )?;
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
                let old = self.env.clone();
                for param in params {
                    for name in pattern_names(self.source, param)? {
                        self.env.insert(name, ValueType::Any);
                    }
                }
                let body = self.expr(body)?;
                self.env = old;
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
                let argument = self.expr(argument)?;
                self.row_context = previous_row_context;
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
                let mut arm_effects = EffectRows::default();
                let mut ty = fact.ty.clone();
                let mut handled = BTreeSet::new();
                let mut catches_all = false;
                for arm in arms {
                    self.check_pattern(&arm.pattern, &ValueType::Any)?;
                    match &arm.pattern.node {
                        PatternKind::Wildcard | PatternKind::Bind(_) => catches_all = true,
                        PatternKind::Constructor { name, .. } => {
                            handled.insert(name.clone());
                        }
                        _ => {}
                    }
                    let old = self.env.clone();
                    for name in pattern_names(self.source, &arm.pattern)? {
                        self.env.insert(name, ValueType::Any);
                    }
                    let arm = self.expr(&arm.value)?;
                    self.env = old;
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
            ExprKind::Block(forms) => {
                let old = self.env.clone();
                let old_types = self.local_types.clone();
                let old_constructors = self.constructors.clone();
                let mut fact = pure(ValueType::Null);
                for form in forms {
                    let next = self.form(form, false)?;
                    fact = Fact {
                        ty: next.ty,
                        effects: fact.effects.union(next.effects),
                    };
                }
                self.env = old;
                self.local_types = old_types;
                self.constructors = old_constructors;
                Ok(fact)
            }
        }
    }

    fn binary(&mut self, expr: &Expr, op: BinaryOp, left: &Expr, right: &Expr) -> Result<Fact> {
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

    fn match_expr(&mut self, expr: &Expr, value: &Expr, arms: &[MatchArm]) -> Result<Fact> {
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
            let old = self.env.clone();
            for (name, ty) in self.pattern_bindings(&arm.pattern, &value.ty)? {
                self.env.insert(name, ty);
            }
            let arm = self.expr(&arm.value)?;
            self.env = old;
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

    fn check_pattern(&self, pattern: &Pattern, expected: &ValueType) -> Result<()> {
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

    fn pattern_bindings(
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

fn pure(ty: ValueType) -> Fact {
    Fact {
        ty,
        effects: EffectRows::default(),
    }
}
fn contains_local(ty: &ValueType) -> bool {
    match ty {
        ValueType::Local(_) => true,
        ValueType::List(inner) => contains_local(inner),
        ValueType::Record(fields) => fields.values().any(contains_local),
        ValueType::Function(a, b) => contains_local(a) || contains_local(b),
        _ => false,
    }
}
fn numeric(ty: &ValueType) -> bool {
    matches!(ty, ValueType::Int | ValueType::Decimal | ValueType::Any)
}
fn numeric_compatible(expected: &ValueType, actual: &ValueType) -> bool {
    numeric(expected) && numeric(actual)
}
fn unify(left: ValueType, right: ValueType) -> ValueType {
    if left == ValueType::Never {
        right
    } else if right == ValueType::Never || left == right {
        left
    } else if numeric(&left) && numeric(&right) {
        ValueType::Decimal
    } else {
        ValueType::Any
    }
}
fn ensure_assignable(
    source: &str,
    expr: &Expr,
    expected: &ValueType,
    actual: &ValueType,
) -> Result<()> {
    if expected == &ValueType::Any
        || actual == &ValueType::Any
        || expected == actual
        || numeric_compatible(expected, actual)
    {
        Ok(())
    } else {
        Err(Diagnostic::new(
            "TM3016",
            format!("expected {expected:?}, found {actual:?}"),
            expr.span,
            source,
        ))
    }
}
fn apply_last_type(
    source: &str,
    expr: &Expr,
    target: ValueType,
    piped: &ValueType,
) -> Result<ValueType> {
    match target {
        ValueType::Function(argument, result) => match *result {
            result @ ValueType::Function(_, _) => Ok(ValueType::Function(
                argument,
                Box::new(apply_last_type(source, expr, result, piped)?),
            )),
            result => {
                ensure_assignable(source, expr, &argument, piped)?;
                Ok(result)
            }
        },
        ValueType::Any => Ok(ValueType::Any),
        other => Err(Diagnostic::new(
            "TM3017",
            format!("pipeline target is not callable: {other:?}"),
            expr.span,
            source,
        )),
    }
}
fn type_from_term(term: &TypeTerm) -> ValueType {
    match term {
        TypeTerm::Named(name) => match name.as_str() {
            "String" | "Path" => ValueType::String,
            "Int" => ValueType::Int,
            "Decimal" => ValueType::Decimal,
            "Bool" => ValueType::Bool,
            other => ValueType::Local(other.into()),
        },
        TypeTerm::List(inner) => ValueType::List(Box::new(type_from_term(inner))),
        TypeTerm::Option(_) => ValueType::Any,
        TypeTerm::Record(fields) => ValueType::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), type_from_term(ty)))
                .collect(),
        ),
    }
}
fn pattern_names(source: &str, pattern: &Pattern) -> Result<BTreeSet<String>> {
    fn walk(source: &str, pattern: &Pattern, names: &mut BTreeSet<String>) -> Result<()> {
        match &pattern.node {
            PatternKind::Bind(name) if !names.insert(name.clone()) => {
                return Err(Diagnostic::new(
                    "TM3018",
                    format!("duplicate pattern binding {name}"),
                    pattern.span,
                    source,
                ));
            }
            PatternKind::Bind(_) => {}
            PatternKind::Constructor {
                payload: Some(payload),
                ..
            } => walk(source, payload, names)?,
            PatternKind::List(values) => {
                for value in values {
                    walk(source, value, names)?;
                }
            }
            PatternKind::Cons { head, tail } => {
                walk(source, head, names)?;
                walk(source, tail, names)?;
            }
            PatternKind::Record { fields, .. } => {
                let mut field_names = BTreeSet::new();
                for (name, value) in fields {
                    if !field_names.insert(name) {
                        return Err(Diagnostic::new(
                            "TM3019",
                            format!("duplicate pattern field {name}"),
                            pattern.span,
                            source,
                        ));
                    }
                    walk(source, value, names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut names = BTreeSet::new();
    walk(source, pattern, &mut names)?;
    Ok(names)
}
fn prelude_types() -> BTreeMap<String, ValueType> {
    let a = ValueType::Any;
    let list = ValueType::List(Box::new(a.clone()));
    let unary = |result| ValueType::Function(Box::new(a.clone()), Box::new(result));
    let binary = |result| ValueType::Function(Box::new(a.clone()), Box::new(unary(result)));
    BTreeMap::from([
        ("print".into(), unary(ValueType::Null)),
        ("display".into(), binary(ValueType::Null)),
        ("length".into(), unary(ValueType::Int)),
        ("sum".into(), unary(a.clone())),
        ("lines".into(), unary(list.clone())),
        ("split".into(), binary(list.clone())),
        ("contains".into(), binary(ValueType::Bool)),
        ("map".into(), binary(list.clone())),
        ("flatmap".into(), binary(list.clone())),
        ("filter".into(), binary(list.clone())),
        ("take".into(), binary(list.clone())),
        ("merge".into(), binary(a.clone())),
        ("table".into(), unary(a.clone())),
        ("where".into(), binary(a.clone())),
        ("select".into(), binary(a.clone())),
        (
            "sort_by".into(),
            ValueType::Function(Box::new(a.clone()), Box::new(binary(a.clone()))),
        ),
        ("group_by".into(), binary(a.clone())),
        ("aggregate".into(), binary(a.clone())),
        ("par".into(), unary(a.clone())),
        ("help".into(), unary(a.clone())),
        ("rethrow".into(), unary(ValueType::Never)),
        ("asc".into(), a.clone()),
        ("desc".into(), a.clone()),
    ])
}

enum Interpolation {
    Name(String),
    Expression(String),
}

fn interpolations(source: &str) -> Vec<Interpolation> {
    let chars: Vec<char> = source.chars().collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] != '#' {
            index += 1;
            continue;
        }
        if chars.get(index + 1) == Some(&'{') {
            let start = index + 2;
            let mut end = start;
            let mut depth = 1;
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
            if depth == 0 {
                result.push(Interpolation::Expression(
                    chars[start..end].iter().collect(),
                ));
                index = end + 1;
            } else {
                break;
            }
        } else {
            let start = index + 1;
            let mut end = start;
            while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                end += 1;
            }
            if end > start {
                result.push(Interpolation::Name(chars[start..end].iter().collect()));
                index = end;
            } else {
                index += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectSignature, parse};

    fn catalog() -> CapabilityCatalog {
        CapabilityCatalog::new()
            .scheme("workspace")
            .register(EffectSignature::new(
                "fs.read",
                ValueType::Uri,
                ValueType::String,
            ))
            .allow("fs.read")
    }

    #[test]
    fn rejects_unknown_and_ungranted_authority_before_eval() {
        let source = "@fs.patch {patch: \"x\"}";
        let cell = parse(source).unwrap();
        assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3008");
        let catalog = catalog().register(EffectSignature::new(
            "fs.patch",
            ValueType::Any,
            ValueType::Null,
        ));
        assert_eq!(check(source, &cell, &catalog).unwrap_err().code, "TM3009");
    }

    #[test]
    fn rejects_unknown_scheme_and_non_exhaustive_closed_match() {
        let source = "@fs.read mystery:path";
        let cell = parse(source).unwrap();
        assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3005");
        let source = "match true { | true -> 1 }";
        let cell = parse(source).unwrap();
        assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3015");
    }

    #[test]
    fn rejects_duplicate_records_and_patterns() {
        for source in ["{x: 1, x: 2}", "let {x, x} = {x: 1}"] {
            let cell = parse(source).unwrap();
            assert!(check(source, &cell, &catalog()).is_err());
        }
    }

    #[test]
    fn infers_separate_authority_error_and_presentation_rows() {
        let source = "@fs.read workspace:a |> display {kind: \"text\"}";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert_eq!(
            checked.effects.authority,
            BTreeSet::from(["fs.read".into()])
        );
        assert_eq!(
            checked.effects.presentation,
            BTreeSet::from(["display".into()])
        );

        let source = "1 / 0";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert!(checked.effects.errors.contains("DivisionByZero"));
    }

    #[test]
    fn rejects_local_sum_type_escape() {
        let source = "let escaped = do { type Local = | Only; Only }";
        let cell = parse(source).unwrap();
        assert_eq!(check(source, &cell, &catalog()).unwrap_err().code, "TM3001");
    }

    #[test]
    fn preserves_types_for_destructured_bindings() {
        let source = "let {a} = {a: 1}; a + 1";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert_eq!(checked.bindings.get("a"), Some(&ValueType::Int));
        assert_eq!(checked.result_type, ValueType::Int);

        let source = "let head :: tail = [1, 2]; head + length tail";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert_eq!(checked.bindings.get("head"), Some(&ValueType::Int));
        assert_eq!(
            checked.bindings.get("tail"),
            Some(&ValueType::List(Box::new(ValueType::Int)))
        );
        assert_eq!(checked.result_type, ValueType::Int);
    }

    #[test]
    fn sum_is_available_in_the_checked_prelude() {
        let source = "let values = [1, 2]; values |> sum";
        assert!(check(source, &parse(source).unwrap(), &catalog()).is_ok());
    }

    #[test]
    fn retains_errors_not_covered_by_handle_arms() {
        let mut signature = EffectSignature::new("test.risky", ValueType::Any, ValueType::Null);
        signature.errors =
            BTreeSet::from(["ApprovalDeniedError".into(), "InvalidPathError".into()]);
        let catalog = CapabilityCatalog::new()
            .register(signature)
            .allow("test.risky");
        let source = "handle @test.risky null with error { | ApprovalDeniedError _ -> null }";
        let checked = check(source, &parse(source).unwrap(), &catalog).unwrap();

        assert_eq!(
            checked.effects.errors,
            BTreeSet::from(["InvalidPathError".into()])
        );
    }

    #[test]
    fn rejects_statically_impossible_exact_patterns() {
        for source in [
            "let {a} = {a: 1, b: 2}; a",
            "let [value] = 1; value",
            "let {missing, ...} = {present: 1}; missing",
        ] {
            let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
            assert_eq!(error.code, "TM3022", "{source}: {error:?}");
        }

        let source = "let {a, ...} = {a: 1, b: 2}; a";
        assert!(check(source, &parse(source).unwrap(), &catalog()).is_ok());
    }

    #[test]
    fn rejects_invalid_ordered_comparisons_and_cons_tails() {
        for source in ["true < false", "1 < 2 < 3", "1 :: 2"] {
            let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
            assert_eq!(error.code, "TM3014", "{source}");
        }

        for source in ["1 < 2", "1 :: [2, 3]"] {
            assert!(
                check(source, &parse(source).unwrap(), &catalog()).is_ok(),
                "{source}"
            );
        }
    }

    #[test]
    fn under_applied_data_last_pipeline_remains_a_function() {
        let source = "[1, 2] |> map";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert!(matches!(checked.result_type, ValueType::Function(_, _)));
    }
}
