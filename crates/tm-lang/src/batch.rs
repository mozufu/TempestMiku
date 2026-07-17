use std::collections::BTreeSet;

use crate::{Cell, Expr, ExprKind, Form, FormKind, Pattern, PatternKind, Result, parse};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BindingUsage {
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
}

pub(crate) fn binding_usage(source: &str) -> Result<BindingUsage> {
    let cell = parse(source)?;
    let mut analyzer = Analyzer::default();
    analyzer.cell(&cell, &mut BTreeSet::new(), true);
    Ok(analyzer.usage)
}

#[derive(Default)]
struct Analyzer {
    usage: BindingUsage,
}

impl Analyzer {
    fn cell(&mut self, cell: &Cell, bound: &mut BTreeSet<String>, persistent: bool) {
        for form in &cell.forms {
            self.form(form, bound, persistent);
        }
    }

    fn form(&mut self, form: &Form, bound: &mut BTreeSet<String>, persistent: bool) {
        match &form.node {
            FormKind::Type(_) => {}
            FormKind::Let { pattern, value } => {
                self.expr(value, bound);
                let names = pattern_names(pattern);
                if persistent {
                    self.usage.writes.extend(names.iter().cloned());
                }
                bound.extend(names);
            }
            FormKind::Fun { name, params, body } => {
                let mut local = bound.clone();
                local.insert(name.clone());
                for param in params {
                    local.extend(pattern_names(param));
                }
                self.expr(body, &local);
                if persistent {
                    self.usage.writes.insert(name.clone());
                }
                bound.insert(name.clone());
            }
            FormKind::Expr(expr) => self.expr(expr, bound),
        }
    }

    fn expr(&mut self, expr: &Expr, bound: &BTreeSet<String>) {
        match &expr.node {
            ExprKind::String(value) => self.interpolations(value, bound),
            ExprKind::Name(name) => {
                if !bound.contains(name) {
                    self.usage.reads.insert(name.clone());
                }
            }
            ExprKind::List(values) => {
                for value in values {
                    self.expr(value, bound);
                }
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.expr(value, bound);
                }
            }
            ExprKind::Lambda { params, body } => {
                let mut local = bound.clone();
                for param in params {
                    local.extend(pattern_names(param));
                }
                self.expr(body, &local);
            }
            ExprKind::Apply { function, argument } => {
                self.expr(function, bound);
                self.expr(argument, bound);
            }
            ExprKind::Field { target, .. } | ExprKind::Unary { value: target, .. } => {
                self.expr(target, bound);
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left, bound);
                self.expr(right, bound);
            }
            ExprKind::Pipe { value, target } => {
                self.expr(value, bound);
                self.expr(target, bound);
            }
            ExprKind::If {
                condition,
                then_value,
                else_value,
            } => {
                self.expr(condition, bound);
                self.expr(then_value, bound);
                self.expr(else_value, bound);
            }
            ExprKind::Match { value, arms } | ExprKind::Handle { value, arms } => {
                self.expr(value, bound);
                for arm in arms {
                    let mut local = bound.clone();
                    local.extend(pattern_names(&arm.pattern));
                    self.expr(&arm.value, &local);
                }
            }
            ExprKind::Block(forms) => {
                let mut local = bound.clone();
                for form in forms {
                    self.form(form, &mut local, false);
                }
            }
            ExprKind::Int(_)
            | ExprKind::Decimal(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Uri(_)
            | ExprKind::Constructor(_)
            | ExprKind::Capability(_) => {}
        }
    }

    fn interpolations(&mut self, source: &str, bound: &BTreeSet<String>) {
        let chars: Vec<char> = source.chars().collect();
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
                    let fragment: String = chars[start..end].iter().collect();
                    if let Ok(cell) = parse(&fragment) {
                        let mut local = bound.clone();
                        self.cell(&cell, &mut local, false);
                    }
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
                if !bound.contains(&name) {
                    self.usage.reads.insert(name);
                }
                index = end;
            } else {
                index += 1;
            }
        }
    }
}

fn pattern_names(pattern: &Pattern) -> BTreeSet<String> {
    fn walk(pattern: &Pattern, names: &mut BTreeSet<String>) {
        match &pattern.node {
            PatternKind::Bind(name) => {
                names.insert(name.clone());
            }
            PatternKind::Constructor {
                payload: Some(payload),
                ..
            } => walk(payload, names),
            PatternKind::List(values) => {
                for value in values {
                    walk(value, names);
                }
            }
            PatternKind::Cons { head, tail } => {
                walk(head, names);
                walk(tail, names);
            }
            PatternKind::Record { fields, .. } => {
                for (_, value) in fields {
                    walk(value, names);
                }
            }
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    walk(pattern, &mut names);
    names
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
}
