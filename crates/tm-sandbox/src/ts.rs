use deno_ast::{
    DecoratorsTranspileOption, EmitOptions, ImportsNotUsedAsValues, MediaType, ParseParams,
    SourceMapOption, TranspileModuleOptions, TranspileOptions, parse_script,
};
use tm_core::Result;

pub(crate) fn starts_with_top_level_await(code: &str) -> bool {
    code.contains("await ")
}

pub(crate) fn lower_top_level_await(code: &str) -> String {
    if code.contains("await ") {
        wrap_async_cell(code)
    } else {
        code.to_string()
    }
}

fn wrap_async_cell(code: &str) -> String {
    let trimmed = code.trim();
    if !trimmed.contains(';') {
        return format!("(async () => await ({trimmed}))()");
    }
    let mut parts = trimmed.rsplitn(2, ';');
    let tail = parts.next().unwrap_or("").trim();
    let head = parts.next().unwrap_or("").trim_end();
    if tail.is_empty() {
        format!("(async () => {{\n{trimmed}\n}})()")
    } else {
        format!("(async () => {{\n{head};\nreturn ({tail});\n}})()")
    }
}

pub(crate) fn transpile_typescript(code: &str) -> Result<String> {
    let specifier = deno_ast::ModuleSpecifier::parse("file:///cell.ts")
        .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
    let parsed = parse_script(ParseParams {
        specifier,
        text: code.into(),
        media_type: MediaType::TypeScript,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
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
        .map_err(|err| tm_core::Error::Sandbox(err.to_string()))?;
    Ok(transpiled.into_source().text)
}
