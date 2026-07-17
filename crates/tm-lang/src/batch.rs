use std::collections::BTreeSet;

use crate::{
    Cell, Diagnostic, Expr, ExprKind, Form, FormKind, Pattern, PatternKind, Result, Span,
    lexer::lex_bounded, parser::parse_bounded,
};

#[cfg(test)]
const DEFAULT_MAX_SOURCE_BYTES: usize = 256 * 1024;
#[cfg(test)]
const DEFAULT_MAX_SYNTAX_NODES: usize = 100_000;
#[cfg(test)]
const DEFAULT_MAX_PARSE_DEPTH: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BindingUsage {
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
}

#[cfg(test)]
pub(crate) fn binding_usage(source: &str) -> Result<BindingUsage> {
    binding_usage_bounded(
        source,
        DEFAULT_MAX_SOURCE_BYTES,
        DEFAULT_MAX_SYNTAX_NODES,
        DEFAULT_MAX_PARSE_DEPTH,
    )
}

/// Computes a batch cell's free reads and persistent writes under the same source, syntax, and
/// nesting limits used by runtime evaluation. Interpolation fragments get one additional, shared
/// source/token allowance so the analysis remains a bounded constant-factor pass over the cell.
pub(crate) fn binding_usage_bounded(
    source: &str,
    max_source_bytes: usize,
    max_syntax_nodes: usize,
    max_parse_depth: usize,
) -> Result<BindingUsage> {
    let cell = parse_bounded(source, max_source_bytes, max_syntax_nodes, max_parse_depth)?;
    let mut analyzer = Analyzer::new(source, max_source_bytes, max_syntax_nodes, max_parse_depth);
    analyzer.cell(&cell, true)?;
    Ok(analyzer.usage)
}

#[derive(Default)]
struct ScopedBoundSet {
    names: BTreeSet<String>,
    undo: Vec<String>,
    scopes: Vec<usize>,
}

impl ScopedBoundSet {
    fn begin_scope(&mut self) -> usize {
        let checkpoint = self.undo.len();
        self.scopes.push(checkpoint);
        checkpoint
    }

    fn rollback_scope(&mut self, checkpoint: usize) {
        debug_assert_eq!(self.scopes.pop(), Some(checkpoint));
        while self.undo.len() > checkpoint {
            let name = self.undo.pop().expect("undo length checked");
            self.names.remove(&name);
        }
    }

    fn insert(&mut self, name: String) {
        if self.names.insert(name.clone()) && !self.scopes.is_empty() {
            self.undo.push(name);
        }
    }

    fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

struct Analyzer<'a> {
    root_source: &'a str,
    usage: BindingUsage,
    bound: ScopedBoundSet,
    remaining_fragment_bytes: usize,
    remaining_fragment_tokens: usize,
    max_fragment_tokens: usize,
    analysis_depth: usize,
    max_parse_depth: usize,
}

impl<'a> Analyzer<'a> {
    fn new(
        root_source: &'a str,
        max_source_bytes: usize,
        max_syntax_nodes: usize,
        max_parse_depth: usize,
    ) -> Self {
        Self {
            root_source,
            usage: BindingUsage::default(),
            bound: ScopedBoundSet::default(),
            remaining_fragment_bytes: max_source_bytes,
            // A token occupies at least one source byte except for EOF, so twice the source budget
            // bounds the cumulative token work while still allowing many individually bounded
            // interpolation expressions in one otherwise-valid string.
            remaining_fragment_tokens: max_source_bytes.saturating_mul(2),
            max_fragment_tokens: max_syntax_nodes,
            analysis_depth: 0,
            max_parse_depth,
        }
    }

    fn with_scope<T>(&mut self, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let checkpoint = self.bound.begin_scope();
        let result = body(self);
        self.bound.rollback_scope(checkpoint);
        result
    }

    fn cell(&mut self, cell: &Cell, persistent: bool) -> Result<()> {
        for form in &cell.forms {
            self.form(form, persistent)?;
        }
        Ok(())
    }

    fn form(&mut self, form: &Form, persistent: bool) -> Result<()> {
        match &form.node {
            FormKind::Type(_) => {}
            FormKind::Let { pattern, value } => {
                self.expr(value)?;
                if persistent {
                    record_pattern_names(pattern, &mut self.usage.writes);
                }
                bind_pattern(pattern, &mut self.bound);
            }
            FormKind::Fun { name, params, body } => {
                self.with_scope(|analyzer| {
                    analyzer.bound.insert(name.clone());
                    for param in params {
                        bind_pattern(param, &mut analyzer.bound);
                    }
                    analyzer.expr(body)
                })?;
                if persistent {
                    self.usage.writes.insert(name.clone());
                }
                self.bound.insert(name.clone());
            }
            FormKind::Expr(expr) => self.expr(expr)?,
        }
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<()> {
        if self.analysis_depth >= self.max_parse_depth {
            return Err(Diagnostic::new(
                "TM2021",
                "batch analysis nesting budget exceeded",
                expr.span,
                self.root_source,
            ));
        }
        self.analysis_depth += 1;
        let result = self.expr_inner(expr);
        self.analysis_depth -= 1;
        result
    }

    fn expr_inner(&mut self, expr: &Expr) -> Result<()> {
        match &expr.node {
            ExprKind::String(value) => self.interpolations(value)?,
            ExprKind::Name(name) => {
                if !self.bound.contains(name) {
                    self.usage.reads.insert(name.clone());
                }
            }
            ExprKind::List(values) => {
                for value in values {
                    self.expr(value)?;
                }
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.expr(value)?;
                }
            }
            ExprKind::Lambda { params, body } => {
                self.with_scope(|analyzer| {
                    for param in params {
                        bind_pattern(param, &mut analyzer.bound);
                    }
                    analyzer.expr(body)
                })?;
            }
            ExprKind::Apply { function, argument } => {
                self.expr(function)?;
                self.expr(argument)?;
            }
            ExprKind::Field { target, .. } | ExprKind::Unary { value: target, .. } => {
                self.expr(target)?;
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left)?;
                self.expr(right)?;
            }
            ExprKind::Pipe { value, target } => {
                self.expr(value)?;
                self.expr(target)?;
            }
            ExprKind::If {
                condition,
                then_value,
                else_value,
            } => {
                self.expr(condition)?;
                self.expr(then_value)?;
                self.expr(else_value)?;
            }
            ExprKind::Match { value, arms } | ExprKind::Handle { value, arms } => {
                self.expr(value)?;
                for arm in arms {
                    self.with_scope(|analyzer| {
                        bind_pattern(&arm.pattern, &mut analyzer.bound);
                        analyzer.expr(&arm.value)
                    })?;
                }
            }
            ExprKind::Block(forms) => {
                self.with_scope(|analyzer| {
                    for form in forms {
                        analyzer.form(form, false)?;
                    }
                    Ok(())
                })?;
            }
            ExprKind::Int(_)
            | ExprKind::Decimal(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Uri(_)
            | ExprKind::Constructor(_)
            | ExprKind::Capability(_) => {}
        }
        Ok(())
    }

    fn interpolations(&mut self, source: &str) -> Result<()> {
        let chars: Vec<char> = source.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            if chars[index] == '\\' {
                index = (index + 2).min(chars.len());
                continue;
            }
            if chars[index] != '#' {
                index += 1;
                continue;
            }
            if chars.get(index + 1) == Some(&'{') {
                let start = index + 2;
                let mut end = start;
                let mut depth = 1usize;
                while end < chars.len() && depth > 0 {
                    match chars[end] {
                        '{' => depth = depth.saturating_add(1),
                        '}' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 {
                        end += 1;
                    }
                }
                if depth == 0 {
                    let fragment: String = chars[start..end].iter().collect();
                    self.interpolation_expression(&fragment)?;
                    index = end + 1;
                    continue;
                }
                break;
            }
            let start = index + 1;
            let mut end = start;
            while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                end += 1;
            }
            if end > start {
                let name: String = chars[start..end].iter().collect();
                if !self.bound.contains(&name) {
                    self.usage.reads.insert(name);
                }
                index = end;
            } else {
                index += 1;
            }
        }
        Ok(())
    }

    fn interpolation_expression(&mut self, source: &str) -> Result<()> {
        if source.len() > self.remaining_fragment_bytes {
            return Err(Diagnostic::new(
                "TM2019",
                "interpolation analysis source budget exceeded",
                Span::new(0, source.len()),
                source,
            ));
        }
        if self.analysis_depth >= self.max_parse_depth {
            return Err(Diagnostic::new(
                "TM2021",
                "interpolation analysis nesting budget exceeded",
                Span::new(0, source.len()),
                source,
            ));
        }

        // Count tokens before parsing so all recursively parsed fragments share one syntax budget.
        // Re-lexing in `parse_bounded` is a fixed 2x cost and avoids an unbounded parser API.
        let tokens = lex_bounded(
            source,
            self.max_fragment_tokens.min(self.remaining_fragment_tokens),
        )?;
        let token_count = tokens.len();
        self.remaining_fragment_bytes -= source.len();
        self.remaining_fragment_tokens -= token_count;
        let remaining_depth = self.max_parse_depth - self.analysis_depth;
        let cell = parse_bounded(source, source.len(), token_count, remaining_depth)?;

        self.with_scope(|analyzer| analyzer.cell(&cell, false))
    }
}

fn bind_pattern(pattern: &Pattern, bound: &mut ScopedBoundSet) {
    let mut pending = vec![pattern];
    while let Some(pattern) = pending.pop() {
        match &pattern.node {
            PatternKind::Bind(name) => bound.insert(name.clone()),
            PatternKind::Constructor {
                payload: Some(payload),
                ..
            } => pending.push(payload),
            PatternKind::List(values) => pending.extend(values.iter()),
            PatternKind::Cons { head, tail } => pending.extend([head.as_ref(), tail.as_ref()]),
            PatternKind::Record { fields, .. } => {
                pending.extend(fields.iter().map(|(_, value)| value));
            }
            _ => {}
        }
    }
}

fn record_pattern_names(pattern: &Pattern, names: &mut BTreeSet<String>) {
    let mut pending = vec![pattern];
    while let Some(pattern) = pending.pop() {
        match &pattern.node {
            PatternKind::Bind(name) => {
                names.insert(name.clone());
            }
            PatternKind::Constructor {
                payload: Some(payload),
                ..
            } => pending.push(payload),
            PatternKind::List(values) => pending.extend(values.iter()),
            PatternKind::Cons { head, tail } => pending.extend([head.as_ref(), tail.as_ref()]),
            PatternKind::Record { fields, .. } => {
                pending.extend(fields.iter().map(|(_, value)| value));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_tracks_free_reads_and_persistent_writes() {
        let usage = binding_usage(
            "let first = source; fun add item = item + first; \"#{add suffix} #first\"",
        )
        .unwrap();
        assert_eq!(usage.writes, BTreeSet::from(["add".into(), "first".into()]));
        assert_eq!(
            usage.reads,
            BTreeSet::from(["source".into(), "suffix".into()])
        );
    }

    #[test]
    fn usage_respects_block_lambda_and_pattern_scopes() {
        let usage = binding_usage(
            "let result = do { let local = outer; [local] |> map (fun item -> item + local) }; \
             match result { | head :: tail -> head + length tail | [] -> fallback }",
        )
        .unwrap();
        assert_eq!(usage.writes, BTreeSet::from(["result".into()]));
        assert_eq!(
            usage.reads,
            BTreeSet::from([
                "fallback".into(),
                "length".into(),
                "map".into(),
                "outer".into()
            ])
        );
    }

    #[test]
    fn shadowed_names_are_restored_after_each_scope() {
        let usage = binding_usage(
            "let value = outer; \
             (fun value -> value + local) 1; \
             match value { | value -> value | _ -> fallback }; \
             value + final_read",
        )
        .unwrap();
        assert_eq!(usage.writes, BTreeSet::from(["value".into()]));
        assert_eq!(
            usage.reads,
            BTreeSet::from([
                "fallback".into(),
                "final_read".into(),
                "local".into(),
                "outer".into()
            ])
        );
    }

    #[test]
    fn bounded_usage_rejects_source_syntax_and_depth_overruns() {
        let source = "external_name";
        assert_eq!(
            binding_usage_bounded(source, 4, 100, 32).unwrap_err().code,
            "TM2019"
        );

        let source = "[a, b, c, d]";
        assert_eq!(
            binding_usage_bounded(source, 1024, 4, 32).unwrap_err().code,
            "TM1007"
        );

        let source = "((((value))))";
        assert_eq!(
            binding_usage_bounded(source, 1024, 100, 2)
                .unwrap_err()
                .code,
            "TM2021"
        );
    }

    #[test]
    fn many_function_scopes_remain_exact() {
        let mut source = String::from("let captured = external;");
        for index in 0..2_000 {
            source.push_str(&format!(" fun f{index} item = item + captured;"));
        }
        source.push_str(" f1999 tail");

        let usage = binding_usage(&source).unwrap();
        assert_eq!(usage.writes.len(), 2_001);
        assert_eq!(
            usage.reads,
            BTreeSet::from(["external".into(), "tail".into()])
        );
    }
}
