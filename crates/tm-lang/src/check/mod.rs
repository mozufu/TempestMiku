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
}

mod expressions;
mod helpers;
mod patterns;

use helpers::*;

#[cfg(test)]
mod tests;
