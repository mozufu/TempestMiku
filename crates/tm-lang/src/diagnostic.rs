use crate::Span;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code} at {line}:{column}: {message}")]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub line: usize,
    pub column: usize,
}

impl Diagnostic {
    pub fn new(code: &'static str, message: impl Into<String>, span: Span, source: &str) -> Self {
        let prefix = &source[..span.start.min(source.len())];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix
            .rsplit('\n')
            .next()
            .map_or(1, |tail| tail.chars().count() + 1);
        Self {
            code,
            message: message.into(),
            span,
            line,
            column,
        }
    }
}

pub type Result<T> = std::result::Result<T, Diagnostic>;
