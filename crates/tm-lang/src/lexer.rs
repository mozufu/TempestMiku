use crate::{Diagnostic, Result, Span, Spanned, SpannedToken, Token};

pub fn lex(source: &str) -> Result<Vec<SpannedToken>> {
    lex_bounded(source, usize::MAX)
}

pub(crate) fn lex_bounded(source: &str, max_tokens: usize) -> Result<Vec<SpannedToken>> {
    Lexer {
        source,
        offset: 0,
        max_tokens,
    }
    .run()
}

struct Lexer<'a> {
    source: &'a str,
    offset: usize,
    max_tokens: usize,
}

impl<'a> Lexer<'a> {
    fn run(mut self) -> Result<Vec<SpannedToken>> {
        let mut tokens = Vec::new();
        let mut delimiters = Vec::new();
        while self.offset < self.source.len() {
            self.skip_space();
            if self.offset >= self.source.len() {
                break;
            }
            if self.legacy_separator_here() {
                return Err(self.error(
                    "TM1006",
                    "legacy `---` separator was removed; separate cell forms with `;`",
                    Span::new(self.offset, self.offset + 3),
                ));
            }
            if self.rest().starts_with("--") {
                self.offset += self.rest().find('\n').unwrap_or(self.rest().len());
                continue;
            }
            let start = self.offset;
            if self.peek() == Some('"') {
                self.bump();
                let token = self.string(start)?;
                self.push_token(&mut tokens, token)?;
                continue;
            }
            let record_field_position = delimiters.last() == Some(&'{')
                && tokens.last().is_some_and(|token: &SpannedToken| {
                    matches!(token.node, Token::LBrace | Token::Comma)
                });
            let token = match self.bump().expect("offset checked") {
                '(' => Token::LParen,
                ')' => Token::RParen,
                '{' => Token::LBrace,
                '}' => Token::RBrace,
                '[' => Token::LBracket,
                ']' => Token::RBracket,
                ',' => Token::Comma,
                ';' => Token::Semicolon,
                '@' => Token::At,
                '+' => Token::Plus,
                '*' => Token::Star,
                '/' => Token::Slash,
                '%' => Token::Percent,
                '.' if self.rest().starts_with("..") => {
                    self.offset += 2;
                    Token::Ellipsis
                }
                '.' => Token::Dot,
                ':' if self.rest().starts_with(':') => {
                    self.offset += 1;
                    Token::Cons
                }
                ':' => Token::Colon,
                '|' if self.rest().starts_with('>') => {
                    self.offset += 1;
                    Token::PipeGt
                }
                '|' => Token::Pipe,
                '=' if self.rest().starts_with('=') => {
                    self.offset += 1;
                    Token::EqEq
                }
                '=' => Token::Eq,
                '!' if self.rest().starts_with('=') => {
                    self.offset += 1;
                    Token::NotEq
                }
                '<' if self.rest().starts_with('=') => {
                    self.offset += 1;
                    Token::Le
                }
                '<' => Token::Lt,
                '>' if self.rest().starts_with('=') => {
                    self.offset += 1;
                    Token::Ge
                }
                '>' => Token::Gt,
                '-' if self.rest().starts_with('>') => {
                    self.offset += 1;
                    Token::Arrow
                }
                '-' => Token::Minus,
                ch if ch.is_ascii_digit() => self.number_or_uri(start, ch)?,
                ch if is_ident_start(ch) => self.ident_or_uri(start, ch, record_field_position),
                ch => {
                    return Err(self.error(
                        "TM1001",
                        format!("unsupported character {ch:?}"),
                        Span::new(start, self.offset),
                    ));
                }
            };
            match &token {
                Token::LParen => delimiters.push('('),
                Token::LBrace => delimiters.push('{'),
                Token::LBracket => delimiters.push('['),
                Token::RParen if delimiters.last() == Some(&'(') => {
                    delimiters.pop();
                }
                Token::RBrace if delimiters.last() == Some(&'{') => {
                    delimiters.pop();
                }
                Token::RBracket if delimiters.last() == Some(&'[') => {
                    delimiters.pop();
                }
                _ => {}
            }
            let token = Spanned::new(token, Span::new(start, self.offset));
            self.push_token(&mut tokens, token)?;
        }
        self.finish(tokens)
    }

    fn finish(&self, mut tokens: Vec<SpannedToken>) -> Result<Vec<SpannedToken>> {
        let eof = Spanned::new(Token::Eof, Span::new(self.source.len(), self.source.len()));
        self.push_token(&mut tokens, eof)?;
        Ok(tokens)
    }

    fn push_token(&self, tokens: &mut Vec<SpannedToken>, token: SpannedToken) -> Result<()> {
        if tokens.len() >= self.max_tokens {
            return Err(self.error(
                "TM1007",
                format!(
                    "syntax budget exceeded: more than {} tokens",
                    self.max_tokens
                ),
                token.span,
            ));
        }
        tokens.push(token);
        Ok(())
    }

    fn string(&mut self, start: usize) -> Result<SpannedToken> {
        let mut raw = String::from("\"");
        let mut escaped = false;
        while let Some(ch) = self.bump() {
            raw.push(ch);
            if ch == '"' && !escaped {
                let json_raw = raw.replace("\\#", "\\\\#");
                let value: String = serde_json::from_str(&json_raw).map_err(|error| {
                    self.error("TM1002", error.to_string(), Span::new(start, self.offset))
                })?;
                return Ok(Spanned::new(
                    Token::String(value),
                    Span::new(start, self.offset),
                ));
            }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' {
                escaped = false;
            }
        }
        Err(self.error(
            "TM1003",
            "unterminated string",
            Span::new(start, self.offset),
        ))
    }

    fn number_or_uri(&mut self, start: usize, first: char) -> Result<Token> {
        let mut text = first.to_string();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                text.push(ch);
                self.bump();
            } else {
                break;
            }
        }
        if self.peek() == Some('.')
            && self
                .rest()
                .chars()
                .nth(1)
                .is_some_and(|ch| ch.is_ascii_digit())
        {
            text.push('.');
            self.bump();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    text.push(ch);
                    self.bump();
                } else {
                    break;
                }
            }
            return text.parse().map(Token::Decimal).map_err(|_| {
                self.error("TM1004", "invalid decimal", Span::new(start, self.offset))
            });
        }
        text.parse()
            .map(Token::Int)
            .map_err(|_| self.error("TM1005", "invalid integer", Span::new(start, self.offset)))
    }

    fn ident_or_uri(&mut self, _start: usize, first: char, record_field_position: bool) -> Token {
        let mut text = first.to_string();
        while let Some(ch) = self.peek() {
            if is_ident_continue(ch) {
                text.push(ch);
                self.bump();
            } else {
                break;
            }
        }
        if !record_field_position && let Some(bytes) = self.hyphenated_scheme_suffix_bytes() {
            text.push_str(&self.rest()[..bytes]);
            self.offset += bytes;
        }
        if !record_field_position && self.peek() == Some(':') {
            text.push(':');
            self.bump();
            while let Some(ch) = self.peek() {
                if ch.is_whitespace() || matches!(ch, ',' | ';' | ')' | ']' | '}') {
                    break;
                }
                text.push(ch);
                self.bump();
            }
            Token::Uri(text)
        } else if first.is_uppercase() {
            Token::Upper(text)
        } else {
            Token::Ident(text)
        }
    }

    fn hyphenated_scheme_suffix_bytes(&self) -> Option<usize> {
        let rest = self.rest();
        if !rest.starts_with('-') {
            return None;
        }
        let mut bytes = 0;
        for ch in rest.chars() {
            if ch == ':' {
                return Some(bytes);
            }
            if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
                return None;
            }
            bytes += ch.len_utf8();
        }
        None
    }

    fn legacy_separator_here(&self) -> bool {
        if !self.rest().starts_with("---") {
            return false;
        }
        let before = &self.source[..self.offset];
        if before
            .rsplit('\n')
            .next()
            .is_some_and(|line| !line.trim().is_empty())
        {
            return false;
        }
        let tail = &self.rest()[3..];
        let line = tail.split('\n').next().unwrap_or(tail);
        line.trim().is_empty() || line.trim_start().starts_with("--")
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    fn rest(&self) -> &'a str {
        &self.source[self.offset..]
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }
    fn error(&self, code: &'static str, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(code, message, span, self.source)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}
fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolon_separates_forms_and_uri_beats_colon() {
        let tokens = lex("let 路徑 = workspace:src/main.rs;\n{name: \"miku\"}").unwrap();
        assert!(tokens.iter().any(|token| token.node == Token::Semicolon));
        assert!(
            tokens
                .iter()
                .any(|token| token.node == Token::Uri("workspace:src/main.rs".into()))
        );
        assert!(tokens.iter().any(|token| token.node == Token::Colon));
    }

    #[test]
    fn record_fields_without_spaces_do_not_become_uris() {
        let tokens = lex("{line:42,nested:{value:Int},uri:workspace:a}").unwrap();
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.node == Token::Colon)
                .count(),
            4
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.node == Token::Uri("workspace:a".into()))
        );
        assert!(!tokens.iter().any(|token| {
            matches!(&token.node, Token::Uri(uri) if uri == "line:42" || uri == "value:Int")
        }));
    }

    #[test]
    fn hyphenated_and_root_alias_uris_preserve_subtraction() {
        let tokens = lex("my-repo:src/lib.rs my-repo:; a - b").unwrap();
        assert!(
            tokens
                .iter()
                .any(|token| token.node == Token::Uri("my-repo:src/lib.rs".into()))
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.node == Token::Uri("my-repo:".into()))
        );
        assert!(tokens.iter().any(|token| token.node == Token::Minus));
    }

    #[test]
    fn token_budget_stops_lexing_before_collecting_the_whole_source() {
        let source = std::iter::repeat_n("x", 10_000)
            .collect::<Vec<_>>()
            .join(" ");
        let error = lex_bounded(&source, 8).unwrap_err();
        assert_eq!(error.code, "TM1007");
    }

    #[test]
    fn diagnostics_are_stable_and_spanned() {
        let error = lex("let x = \"oops").unwrap_err();
        assert_eq!(error.code, "TM1003");
        assert_eq!((error.line, error.column), (1, 9));
    }

    #[test]
    fn legacy_separator_fails_instead_of_becoming_a_comment() {
        let error = lex("let x = 1\n--- -- legacy boundary\nx").unwrap_err();
        assert_eq!(error.code, "TM1006");
        assert_eq!((error.line, error.column), (2, 1));
        assert_eq!(
            error.message,
            "legacy `---` separator was removed; separate cell forms with `;`"
        );
    }
}
