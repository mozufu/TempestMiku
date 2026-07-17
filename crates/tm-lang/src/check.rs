use std::{
    cell::Cell as Counter,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use crate::{
    BinaryOp, CapabilityCatalog, Cell, Diagnostic, Expr, ExprKind, Form, FormKind, MatchArm,
    Pattern, PatternKind, Result, Span, TypeDecl, TypeTerm, UnaryOp, ValueType, lexer::lex_bounded,
    parser::parse_bounded,
};

const DEFAULT_MAX_SOURCE_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_SYNTAX_NODES: usize = 100_000;
const DEFAULT_MAX_PARSE_DEPTH: usize = 256;
const MAX_FUNCTION_ARITY: usize = 256;

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
    check_bounded(
        source,
        cell,
        catalog,
        DEFAULT_MAX_SOURCE_BYTES,
        DEFAULT_MAX_SYNTAX_NODES,
        DEFAULT_MAX_PARSE_DEPTH,
    )
}

pub fn check_bounded(
    source: &str,
    cell: &Cell,
    catalog: &CapabilityCatalog,
    max_source_bytes: usize,
    max_syntax_nodes: usize,
    max_parse_depth: usize,
) -> Result<CheckedCell> {
    Checker::new_bounded(
        source,
        catalog,
        max_source_bytes,
        max_syntax_nodes,
        max_parse_depth,
    )
    .cell(cell)
}

pub fn check_with_bindings(
    source: &str,
    cell: &Cell,
    catalog: &CapabilityCatalog,
    bindings: impl IntoIterator<Item = String>,
) -> Result<CheckedCell> {
    check_with_bindings_bounded(
        source,
        cell,
        catalog,
        bindings,
        DEFAULT_MAX_SOURCE_BYTES,
        DEFAULT_MAX_SYNTAX_NODES,
        DEFAULT_MAX_PARSE_DEPTH,
    )
}

pub fn check_with_bindings_bounded(
    source: &str,
    cell: &Cell,
    catalog: &CapabilityCatalog,
    bindings: impl IntoIterator<Item = String>,
    max_source_bytes: usize,
    max_syntax_nodes: usize,
    max_parse_depth: usize,
) -> Result<CheckedCell> {
    let mut checker = Checker::new_bounded(
        source,
        catalog,
        max_source_bytes,
        max_syntax_nodes,
        max_parse_depth,
    );
    checker
        .env
        .extend(bindings.into_iter().map(|name| (name, ValueType::Any)));
    checker.cell(cell)
}

/// Shared across the root checker and every recursively parsed interpolation fragment. The extra
/// fragment allowance matches batch analysis: one source-sized byte budget, a bounded 2x token
/// budget, and one combined AST/interpolation nesting counter.
struct CheckContext {
    remaining_fragment_bytes: Counter<usize>,
    remaining_fragment_tokens: Counter<usize>,
    max_fragment_tokens: usize,
    check_depth: Counter<usize>,
    max_parse_depth: usize,
}

impl CheckContext {
    fn new(max_source_bytes: usize, max_syntax_nodes: usize, max_parse_depth: usize) -> Self {
        Self {
            remaining_fragment_bytes: Counter::new(max_source_bytes),
            remaining_fragment_tokens: Counter::new(max_source_bytes.saturating_mul(2)),
            max_fragment_tokens: max_syntax_nodes,
            check_depth: Counter::new(0),
            max_parse_depth,
        }
    }

    fn enter_expr(&self, expr: &Expr, source: &str) -> Result<()> {
        let depth = self.check_depth.get();
        if depth >= self.max_parse_depth {
            return Err(Diagnostic::new(
                "TM2021",
                "checker nesting budget exceeded",
                expr.span,
                source,
            ));
        }
        self.check_depth.set(depth + 1);
        Ok(())
    }

    fn exit_expr(&self) {
        let depth = self.check_depth.get();
        debug_assert!(depth > 0);
        self.check_depth.set(depth.saturating_sub(1));
    }

    fn parse_interpolation(&self, source: &str) -> Result<Cell> {
        let remaining_bytes = self.remaining_fragment_bytes.get();
        if source.len() > remaining_bytes {
            return Err(Diagnostic::new(
                "TM2019",
                "interpolation check source budget exceeded",
                Span::new(0, source.len()),
                source,
            ));
        }

        let depth = self.check_depth.get();
        if depth >= self.max_parse_depth {
            return Err(Diagnostic::new(
                "TM2021",
                "interpolation check nesting budget exceeded",
                Span::new(0, source.len()),
                source,
            ));
        }

        // Count before parsing so every recursive fragment consumes the same cumulative syntax
        // allowance. `parse_bounded` re-lexes once, keeping this a fixed bounded-factor pass.
        let remaining_tokens = self.remaining_fragment_tokens.get();
        let tokens = lex_bounded(source, self.max_fragment_tokens.min(remaining_tokens))?;
        let token_count = tokens.len();
        self.remaining_fragment_bytes
            .set(remaining_bytes - source.len());
        self.remaining_fragment_tokens
            .set(remaining_tokens - token_count);

        parse_bounded(
            source,
            source.len(),
            token_count,
            self.max_parse_depth - depth,
        )
    }
}

/// A map with lexical checkpoints. Mutations made while a checkpoint is active are recorded as
/// deltas and undone in reverse order when the scope exits. Persistent, top-level mutations do not
/// accumulate undo history.
struct ScopedMap<V> {
    values: BTreeMap<String, V>,
    undo: Vec<(String, Option<V>)>,
    scopes: Vec<usize>,
}

impl<V> ScopedMap<V> {
    fn new(values: BTreeMap<String, V>) -> Self {
        Self {
            values,
            undo: Vec::new(),
            scopes: Vec::new(),
        }
    }

    fn begin_scope(&mut self) -> usize {
        let checkpoint = self.undo.len();
        self.scopes.push(checkpoint);
        checkpoint
    }

    fn rollback_scope(&mut self, checkpoint: usize) {
        debug_assert_eq!(self.scopes.pop(), Some(checkpoint));
        while self.undo.len() > checkpoint {
            let (name, previous) = self.undo.pop().expect("undo length checked");
            if let Some(previous) = previous {
                self.values.insert(name, previous);
            } else {
                self.values.remove(&name);
            }
        }
    }

    fn insert(&mut self, name: String, value: V) {
        let previous = self.values.insert(name.clone(), value);
        if self.scopes.is_empty() {
            return;
        }
        self.undo.push((name, previous));
    }

    fn extend(&mut self, values: impl IntoIterator<Item = (String, V)>) {
        for (name, value) in values {
            self.insert(name, value);
        }
    }

    fn contains_key(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    fn get(&self, name: &str) -> Option<&V> {
        self.values.get(name)
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }
}

#[derive(Clone, Copy)]
struct ScopeCheckpoint {
    env: usize,
    local_types: usize,
    constructors: usize,
}

struct Checker<'a> {
    source: &'a str,
    catalog: &'a CapabilityCatalog,
    context: Rc<CheckContext>,
    env: ScopedMap<ValueType>,
    local_types: ScopedMap<BTreeMap<String, Option<ValueType>>>,
    constructors: ScopedMap<(String, Option<ValueType>)>,
    row_context: bool,
    rethrow_errors: Option<BTreeSet<String>>,
}

impl<'a> Checker<'a> {
    fn new_bounded(
        source: &'a str,
        catalog: &'a CapabilityCatalog,
        max_source_bytes: usize,
        max_syntax_nodes: usize,
        max_parse_depth: usize,
    ) -> Self {
        Self::with_context(
            source,
            catalog,
            Rc::new(CheckContext::new(
                max_source_bytes,
                max_syntax_nodes,
                max_parse_depth,
            )),
        )
    }

    fn with_context(
        source: &'a str,
        catalog: &'a CapabilityCatalog,
        context: Rc<CheckContext>,
    ) -> Self {
        Self {
            source,
            catalog,
            context,
            env: ScopedMap::new(prelude_types()),
            local_types: ScopedMap::new(BTreeMap::new()),
            constructors: ScopedMap::new(BTreeMap::new()),
            row_context: false,
            rethrow_errors: None,
        }
    }

    fn begin_scope(&mut self) -> ScopeCheckpoint {
        ScopeCheckpoint {
            env: self.env.begin_scope(),
            local_types: self.local_types.begin_scope(),
            constructors: self.constructors.begin_scope(),
        }
    }

    fn rollback_scope(&mut self, checkpoint: ScopeCheckpoint) {
        self.constructors.rollback_scope(checkpoint.constructors);
        self.local_types.rollback_scope(checkpoint.local_types);
        self.env.rollback_scope(checkpoint.env);
    }

    fn with_scope<T>(&mut self, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let checkpoint = self.begin_scope();
        let result = body(self);
        self.rollback_scope(checkpoint);
        result
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
                self.ensure_function_arity(params.len(), form.span)?;
                let body = self.with_scope(|checker| {
                    checker.env.insert(name.clone(), ValueType::Any);
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
        }
        for (name, payload) in &variants {
            self.constructors
                .insert(name.clone(), (decl.name.clone(), payload.clone()));
        }
        self.local_types.insert(decl.name.clone(), variants);
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<Fact> {
        self.context.enter_expr(expr, self.source)?;
        let result = self.expr_inner(expr);
        self.context.exit_expr();
        result
    }

    fn expr_inner(&mut self, expr: &Expr) -> Result<Fact> {
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

    fn ensure_function_arity(&self, arity: usize, span: Span) -> Result<()> {
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
    fn rethrow_restores_the_handled_error_row() {
        let source = "handle 1 / 0 with error { | DivisionByZero _ -> rethrow null }";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert_eq!(
            checked.effects.errors,
            BTreeSet::from(["DivisionByZero".into()])
        );

        let source = "handle 1 / 0 with error { | error -> 0 }";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert!(checked.effects.errors.is_empty());
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

    #[test]
    fn lexical_deltas_restore_shadowed_bindings_and_local_types() {
        let source = "let value = 1; do { let value = true; value }; value + 1";
        let checked = check(source, &parse(source).unwrap(), &catalog()).unwrap();
        assert_eq!(checked.result_type, ValueType::Int);

        let source = "do { type Local = | Only; Only }; Only";
        let error = check(source, &parse(source).unwrap(), &catalog()).unwrap_err();
        assert_eq!(error.code, "TM3007");
    }

    #[test]
    fn lexical_deltas_are_rolled_back_when_a_scope_errors() {
        let source = "do { type Local = | Only; let leaked = Only; missing }";
        let cell = parse(source).unwrap();
        let catalog = catalog();
        let mut checker = Checker::new_bounded(
            source,
            &catalog,
            DEFAULT_MAX_SOURCE_BYTES,
            DEFAULT_MAX_SYNTAX_NODES,
            DEFAULT_MAX_PARSE_DEPTH,
        );

        assert_eq!(
            checker.form(&cell.forms[0], true).unwrap_err().code,
            "TM3006"
        );
        assert!(!checker.env.contains_key("leaked"));
        assert!(!checker.local_types.contains_key("Local"));
        assert!(!checker.constructors.contains_key("Only"));
        assert!(checker.env.undo.is_empty());
        assert!(checker.local_types.undo.is_empty());
        assert!(checker.constructors.undo.is_empty());
        assert!(checker.env.scopes.is_empty());
        assert!(checker.local_types.scopes.is_empty());
        assert!(checker.constructors.scopes.is_empty());
    }

    #[test]
    fn many_function_scopes_do_not_copy_the_accumulated_environment() {
        let mut source = String::new();
        for index in 0..2_000 {
            source.push_str(&format!("fun f{index} item = item;"));
        }
        source.push_str("f1999 1");

        let checked = check(&source, &parse(&source).unwrap(), &catalog()).unwrap();
        assert_eq!(checked.bindings.len(), 2_000);
        assert_eq!(checked.result_type, ValueType::Any);
    }

    #[test]
    fn deeply_nested_scopes_restore_each_lexical_delta() {
        let depth = 48;
        let mut source = String::new();
        for index in 0..depth {
            let value = if index == 0 {
                "1".into()
            } else {
                format!("v{}", index - 1)
            };
            source.push_str(&format!("do {{ let v{index} = {value}; "));
        }
        source.push_str(&format!("v{}", depth - 1));
        for _ in 0..depth {
            source.push_str(" }");
        }

        let checked = check(&source, &parse(&source).unwrap(), &catalog()).unwrap();
        assert_eq!(checked.result_type, ValueType::Int);
    }

    #[test]
    fn recursively_nested_interpolations_share_the_check_depth_budget() {
        let mut source = "1".to_string();
        for _ in 0..12 {
            source = serde_json::to_string(&format!("#{{{source}}}")).unwrap();
        }
        let cell = parse(&source).unwrap();
        let max_source_bytes = source.len().saturating_mul(4);

        let error = check_with_bindings_bounded(
            &source,
            &cell,
            &catalog(),
            std::iter::empty::<String>(),
            max_source_bytes,
            1_024,
            8,
        )
        .unwrap_err();

        assert_eq!(error.code, "TM2021", "{error:?}");
    }

    #[test]
    fn flat_function_arity_is_rejected_before_building_nested_function_types() {
        let parameters = std::iter::repeat_n("_", 30_000)
            .collect::<Vec<_>>()
            .join(" ");
        for source in [
            format!("fun too_wide {parameters} = null"),
            format!("let too_wide = fun {parameters} -> null"),
        ] {
            let cell = parse(&source).unwrap();
            let error = check(&source, &cell, &catalog()).unwrap_err();
            assert_eq!(error.code, "TM3023", "{error:?}");
        }
    }
}
