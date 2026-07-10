use std::collections::BTreeSet;

use deno_ast::{
    DecoratorsTranspileOption, EmitOptions, ImportsNotUsedAsValues, MediaType, ParseParams,
    ProgramRef, SourceMapOption, TranspileModuleOptions, TranspileOptions, parse_module,
    swc::{
        ast::{
            ArrowExpr, AssignExpr, AssignOp, AssignTarget, AwaitExpr, BindingIdent, BlockStmt,
            BlockStmtOrExpr, CallExpr, Callee, ClassExpr, Decl, Expr, ExprStmt, FnExpr, ForOfStmt,
            Function, GetterProp, Ident, Module, ModuleItem, Param, ParenExpr, Pat, ReturnStmt,
            Script, SetterProp, SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
        },
        codegen::to_code,
        common::{DUMMY_SP, SyntaxContext},
        ecma_visit::{Visit, VisitWith, noop_visit_type},
    },
};
use tm_core::Result;

/// Compile one TypeScript REPL cell into a persistent script.
///
/// Ordinary cells remain ordinary scripts, which lets V8 preserve their top-level bindings and
/// final expression value without changing JavaScript execution semantics. Cells containing a
/// real top-level `await` are lowered through the SWC AST into an async expression. Only simple
/// identifier declarations are hoisted into the script scope; unsupported binding patterns fail
/// closed instead of being rewritten textually.
pub(crate) fn compile_cell(code: &str) -> Result<String> {
    let parsed_typescript = parse(code, MediaType::TypeScript, "file:///cell.ts")?;
    reject_module_declarations(parsed_typescript.program_ref())?;

    let javascript = transpile_typescript(parsed_typescript)?;
    let parsed_javascript = parse(&javascript, MediaType::JavaScript, "file:///cell.js")?;
    reject_module_declarations(parsed_javascript.program_ref())?;
    match parsed_javascript.program_ref() {
        ProgramRef::Module(module) if contains_actual_top_level_await(module) => {
            let statements = module_statements(module);
            validate_lowerable_bindings(&statements)?;
            lower_top_level_await(statements)
        }
        ProgramRef::Script(script) if contains_actual_top_level_await(script) => {
            validate_lowerable_bindings(&script.body)?;
            lower_top_level_await(script.body.clone())
        }
        ProgramRef::Module(_) | ProgramRef::Script(_) => Ok(javascript),
    }
}

#[derive(Default)]
struct TopLevelAwaitDetector {
    found: bool,
}

impl Visit for TopLevelAwaitDetector {
    noop_visit_type!();

    fn visit_stmt(&mut self, statement: &Stmt) {
        if !self.found {
            statement.visit_children_with(self);
        }
    }

    fn visit_param(&mut self, _: &Param) {}

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_getter_prop(&mut self, property: &GetterProp) {
        property.key.visit_with(self);
    }

    fn visit_setter_prop(&mut self, property: &SetterProp) {
        property.key.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        if statement.is_await {
            self.found = true;
        } else {
            statement.visit_children_with(self);
        }
    }

    fn visit_await_expr(&mut self, _: &AwaitExpr) {
        self.found = true;
    }
}

fn contains_actual_top_level_await<V>(node: &V) -> bool
where
    V: VisitWith<TopLevelAwaitDetector>,
{
    let mut detector = TopLevelAwaitDetector::default();
    node.visit_with(&mut detector);
    detector.found
}

fn parse(code: &str, media_type: MediaType, specifier: &str) -> Result<deno_ast::ParsedSource> {
    let specifier = deno_ast::ModuleSpecifier::parse(specifier)
        .map_err(|error| sandbox_error(error.to_string()))?;
    parse_module(ParseParams {
        specifier,
        text: code.into(),
        media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|error| sandbox_error(error.to_string()))
}

fn transpile_typescript(parsed: deno_ast::ParsedSource) -> Result<String> {
    let transpiled = parsed
        .transpile(
            &TranspileOptions {
                imports_not_used_as_values: ImportsNotUsedAsValues::Remove,
                decorators: DecoratorsTranspileOption::Ecma,
                ..TranspileOptions::default()
            },
            &TranspileModuleOptions::default(),
            &EmitOptions {
                source_map: SourceMapOption::None,
                ..EmitOptions::default()
            },
        )
        .map_err(|error| sandbox_error(error.to_string()))?;
    Ok(transpiled.into_source().text)
}

fn reject_module_declarations(program: ProgramRef<'_>) -> Result<()> {
    let ProgramRef::Module(module) = program else {
        return Ok(());
    };
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return Err(sandbox_error(
            "imports and exports are not available in sandbox cells",
        ));
    }
    Ok(())
}

fn module_statements(module: &Module) -> Vec<Stmt> {
    module
        .body
        .iter()
        .map(|item| match item {
            ModuleItem::Stmt(statement) => statement.clone(),
            ModuleItem::ModuleDecl(_) => unreachable!("module declarations were rejected"),
        })
        .collect()
}

fn validate_lowerable_bindings(statements: &[Stmt]) -> Result<()> {
    let mut bindings = BTreeSet::new();
    for statement in statements {
        if let Stmt::Decl(declaration) = statement {
            validate_declaration(declaration, &mut bindings)?;
        }
    }
    if bindings
        .iter()
        .any(|name| name == "globalThis" || name.starts_with("__tm_cell_"))
    {
        return Err(sandbox_error(
            "cell declaration uses a reserved runtime binding",
        ));
    }
    Ok(())
}

fn validate_declaration(declaration: &Decl, bindings: &mut BTreeSet<String>) -> Result<()> {
    match declaration {
        Decl::Class(class) => {
            bindings.insert(class.ident.sym.to_string());
        }
        Decl::Fn(function) => {
            bindings.insert(function.ident.sym.to_string());
        }
        Decl::Var(vars) => {
            for declaration in &vars.decls {
                let Pat::Ident(identifier) = &declaration.name else {
                    return Err(sandbox_error(
                        "top-level destructuring and binding patterns are not available in sandbox cells",
                    ));
                };
                bindings.insert(identifier.id.sym.to_string());
            }
        }
        Decl::Using(_) => {
            return Err(sandbox_error(
                "top-level using declarations are not available in sandbox cells",
            ));
        }
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) | Decl::TsEnum(_) | Decl::TsModule(_) => {}
    }
    Ok(())
}

fn lower_top_level_await(statements: Vec<Stmt>) -> Result<String> {
    let mut bindings = Vec::<BindingIdent>::new();
    let mut body = Vec::<Stmt>::new();
    let body_len = statements.len();

    for (index, statement) in statements.into_iter().enumerate() {
        let is_last = index + 1 == body_len;
        match statement {
            Stmt::Decl(declaration) => {
                lower_declaration(declaration, &mut bindings, &mut body)?;
            }
            Stmt::Expr(expression) if is_last => body.push(Stmt::Return(ReturnStmt {
                span: expression.span,
                arg: Some(expression.expr),
            })),
            other => body.push(other),
        }
    }

    let outer_declaration = (!bindings.is_empty()).then(|| {
        Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            kind: VarDeclKind::Var,
            declare: false,
            decls: bindings
                .into_iter()
                .map(|binding| VarDeclarator {
                    span: DUMMY_SP,
                    name: Pat::Ident(binding),
                    init: None,
                    definite: false,
                })
                .collect(),
        })))
    });
    let async_cell = Expr::Call(CallExpr {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        callee: Callee::Expr(Box::new(Expr::Paren(ParenExpr {
            span: DUMMY_SP,
            expr: Box::new(Expr::Arrow(ArrowExpr {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                params: Vec::new(),
                body: Box::new(BlockStmtOrExpr::BlockStmt(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: SyntaxContext::empty(),
                    stmts: body,
                })),
                is_async: true,
                is_generator: false,
                type_params: None,
                return_type: None,
            })),
        }))),
        args: Vec::new(),
        type_args: None,
    });
    let mut script_body = Vec::with_capacity(2);
    if let Some(declaration) = outer_declaration {
        script_body.push(declaration);
    }
    script_body.push(expression_statement(async_cell));
    Ok(to_code(&Script {
        span: DUMMY_SP,
        body: script_body,
        shebang: None,
    }))
}

fn lower_declaration(
    declaration: Decl,
    bindings: &mut Vec<BindingIdent>,
    body: &mut Vec<Stmt>,
) -> Result<()> {
    match declaration {
        Decl::Var(vars) => {
            for declaration in vars.decls {
                let Pat::Ident(binding) = declaration.name else {
                    return Err(sandbox_error(
                        "top-level destructuring and binding patterns are not available in sandbox cells",
                    ));
                };
                bindings.push(binding.clone());
                let right = declaration.init.unwrap_or_else(|| {
                    Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    )))
                });
                body.push(assignment_statement(binding, right));
            }
        }
        Decl::Fn(function) => {
            let binding = BindingIdent::from(function.ident.clone());
            bindings.push(binding.clone());
            body.push(assignment_statement(
                binding,
                Box::new(Expr::Fn(FnExpr {
                    ident: Some(function.ident),
                    function: function.function,
                })),
            ));
        }
        Decl::Class(class) => {
            let binding = BindingIdent::from(class.ident.clone());
            bindings.push(binding.clone());
            body.push(assignment_statement(
                binding,
                Box::new(Expr::Class(ClassExpr {
                    ident: Some(class.ident),
                    class: class.class,
                })),
            ));
        }
        Decl::Using(_) => {
            return Err(sandbox_error(
                "top-level using declarations are not available in sandbox cells",
            ));
        }
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) | Decl::TsEnum(_) | Decl::TsModule(_) => {
            return Err(sandbox_error(
                "unsupported TypeScript declaration remained after transpilation",
            ));
        }
    }
    Ok(())
}

fn assignment_statement(binding: BindingIdent, right: Box<Expr>) -> Stmt {
    expression_statement(Expr::Assign(AssignExpr {
        span: DUMMY_SP,
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Ident(binding)),
        right,
    }))
}

fn expression_statement(expression: Expr) -> Stmt {
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(expression),
    })
}

fn sandbox_error(message: impl Into<String>) -> tm_core::Error {
    tm_core::Error::Sandbox(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_actual_top_level_await() {
        for code in [
            "'await Promise.resolve(1)'",
            "// await Promise.resolve(1)\n41 + 1",
            "async function nested() { await Promise.resolve(1); }\nnested",
            "const nested = async () => await Promise.resolve(1);\nnested",
            "const nested = { async method() { await Promise.resolve(1); } };\nnested",
        ] {
            let compiled = compile_cell(code).unwrap();
            assert!(!is_lowered_async_wrapper(&compiled), "{compiled}");
        }

        let compiled = compile_cell("await Promise.resolve(42)").unwrap();
        assert!(is_lowered_async_wrapper(&compiled));
        assert!(compiled.contains("await Promise.resolve(42)"));

        let compiled =
            compile_cell("const object = { value: await Promise.resolve(42) }; object.value")
                .unwrap();
        assert!(is_lowered_async_wrapper(&compiled));
    }

    #[test]
    fn top_level_await_hoists_supported_declarations_and_returns_final_expression() {
        let compiled = compile_cell(
            "const value = await Promise.resolve(40);\n\
             let offset = 2;\n\
             function add(a: number, b: number) { return a + b; }\n\
             class Holder { static value = value; }\n\
             add(Holder.value, offset)",
        )
        .unwrap();

        assert!(
            compiled.contains("var value, offset, add, Holder"),
            "{compiled}"
        );
        assert!(compiled.contains("value = await Promise.resolve(40)"));
        assert!(compiled.contains("add = function add"), "{compiled}");
        assert!(compiled.contains("Holder = class Holder"), "{compiled}");
        assert!(compiled.contains("return add(Holder.value, offset)"));
    }

    #[test]
    fn non_await_cells_remain_scripts_with_persistent_declarations() {
        let compiled = compile_cell(
            "const value = 1; let count = value; function read() { return count; } class Boxed {} read()",
        )
        .unwrap();
        assert!(!compiled.contains("async"), "{compiled}");
        assert!(compiled.contains("const value = 1"));
        assert!(compiled.contains("function read()"));
        assert!(compiled.contains("class Boxed"));
    }

    #[test]
    fn unsupported_module_syntax_and_bindings_fail_closed() {
        for code in [
            "import { value } from './x.ts'; value",
            "export const value = 1",
            "const { value } = { value: await Promise.resolve(1) }; value",
            "const [value] = [await Promise.resolve(1)]; value",
            "using resource = await acquire(); resource",
        ] {
            let compiled = compile_cell(code);
            assert!(
                compiled.is_err(),
                "{code:?} unexpectedly compiled as {compiled:?}"
            );
        }
    }

    #[test]
    fn ordinary_scripts_keep_supported_javascript_binding_patterns() {
        for code in [
            "const { value } = { value: 1 }; value",
            "const [value] = [1]; value",
        ] {
            let compiled = compile_cell(code).unwrap();
            assert!(!is_lowered_async_wrapper(&compiled), "{compiled}");
        }
    }

    fn is_lowered_async_wrapper(code: &str) -> bool {
        let parsed = parse(code, MediaType::JavaScript, "file:///compiled-cell.js").unwrap();
        let statement = match parsed.program_ref() {
            ProgramRef::Module(module) => {
                let Some(ModuleItem::Stmt(Stmt::Expr(statement))) = module.body.last() else {
                    return false;
                };
                statement
            }
            ProgramRef::Script(script) => {
                let Some(Stmt::Expr(statement)) = script.body.last() else {
                    return false;
                };
                statement
            }
        };
        let Expr::Call(call) = statement.expr.as_ref() else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        matches!(
            callee.as_ref(),
            Expr::Paren(paren)
                if matches!(paren.expr.as_ref(), Expr::Arrow(arrow) if arrow.is_async)
        )
    }
}
