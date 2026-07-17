use super::*;

impl Parser<'_> {
    pub(super) fn infix_binding(&self) -> Option<(u8, u8, Infix)> {
        let pair = match &self.current().node {
            Token::PipeGt => (1, 2, Infix::Pipe),
            Token::Ident(name) if name == "or" => (3, 4, Infix::Binary(BinaryOp::Or)),
            Token::Ident(name) if name == "and" => (5, 6, Infix::Binary(BinaryOp::And)),
            Token::EqEq => (7, 8, Infix::Binary(BinaryOp::Equal)),
            Token::NotEq => (7, 8, Infix::Binary(BinaryOp::NotEqual)),
            Token::Lt => (7, 8, Infix::Binary(BinaryOp::Less)),
            Token::Le => (7, 8, Infix::Binary(BinaryOp::LessEqual)),
            Token::Gt => (7, 8, Infix::Binary(BinaryOp::Greater)),
            Token::Ge => (7, 8, Infix::Binary(BinaryOp::GreaterEqual)),
            Token::Cons => (9, 9, Infix::Binary(BinaryOp::Cons)),
            Token::Plus => (11, 12, Infix::Binary(BinaryOp::Add)),
            Token::Minus => (11, 12, Infix::Binary(BinaryOp::Subtract)),
            Token::Star => (13, 14, Infix::Binary(BinaryOp::Multiply)),
            Token::Slash => (13, 14, Infix::Binary(BinaryOp::Divide)),
            Token::Percent => (13, 14, Infix::Binary(BinaryOp::Modulo)),
            _ => return None,
        };
        Some(pair)
    }

    pub(super) fn peek_is_named_function(&self) -> bool {
        matches!(
            self.tokens.get(self.cursor + 1).map(|token| &token.node),
            Some(Token::Ident(_))
        ) && !matches!(
            self.tokens.get(self.cursor + 2).map(|token| &token.node),
            Some(Token::Arrow)
        )
    }
    pub(super) fn starts_atom(&self) -> bool {
        if self.stop_before_lbrace && self.at(&Token::LBrace) {
            return false;
        }
        matches!(
            &self.current().node,
            Token::String(_)
                | Token::Int(_)
                | Token::Decimal(_)
                | Token::Uri(_)
                | Token::Ident(_)
                | Token::Upper(_)
                | Token::At
                | Token::LParen
                | Token::LBracket
                | Token::LBrace
        ) && !matches!(&self.current().node, Token::Ident(name) if matches!(name.as_str(), "then" | "else" | "with" | "error" | "and" | "or"))
    }
    pub(super) fn starts_pattern(&self) -> bool {
        matches!(
            &self.current().node,
            Token::Ident(_)
                | Token::Upper(_)
                | Token::String(_)
                | Token::Int(_)
                | Token::LParen
                | Token::LBracket
                | Token::LBrace
        )
    }
    pub(super) fn starts_type_term(&self) -> bool {
        matches!(
            &self.current().node,
            Token::Upper(_) | Token::Ident(_) | Token::LParen
        )
    }
    pub(super) fn keyword(&self, keyword: &str) -> bool {
        matches!(&self.current().node, Token::Ident(name) if name == keyword)
    }
    pub(super) fn upper_is(&self, value: &str) -> bool {
        matches!(&self.current().node, Token::Upper(name) if name == value)
    }
    pub(super) fn expect_keyword(&mut self, keyword: &'static str) -> Result<()> {
        if self.keyword(keyword) {
            self.bump();
            Ok(())
        } else {
            Err(self.error("TM2011", format!("expected {keyword}")))
        }
    }
    pub(super) fn lower_name(&mut self, expected: &'static str) -> Result<String> {
        match self.bump().node.clone() {
            Token::Ident(name) => Ok(name),
            _ => Err(self.error("TM2012", format!("expected {expected}"))),
        }
    }
    pub(super) fn upper_name(&mut self, expected: &'static str) -> Result<String> {
        match self.bump().node.clone() {
            Token::Upper(name) => Ok(name),
            _ => Err(self.error("TM2013", format!("expected {expected}"))),
        }
    }
    pub(super) fn expect(&mut self, token: Token, expected: &'static str) -> Result<Span> {
        self.take(&token)
            .ok_or_else(|| self.error("TM2014", format!("expected {expected}")))
    }
    pub(super) fn take(&mut self, token: &Token) -> Option<Span> {
        self.at(token).then(|| self.bump().span)
    }
    pub(super) fn at(&self, token: &Token) -> bool {
        std::mem::discriminant(&self.current().node) == std::mem::discriminant(token)
    }
    pub(super) fn current(&self) -> &SpannedToken {
        &self.tokens[self.cursor]
    }
    pub(super) fn bump(&mut self) -> &SpannedToken {
        let index = self.cursor;
        if !self.at(&Token::Eof) {
            self.cursor += 1;
        }
        &self.tokens[index]
    }
    pub(super) fn span(&self) -> Span {
        self.current().span
    }
    pub(super) fn previous_span(&self) -> Span {
        self.tokens[self.cursor.saturating_sub(1)].span
    }
    pub(super) fn parent_expr_depth(&self, child_depth: usize) -> Result<usize> {
        let depth = child_depth.saturating_add(1);
        if depth > self.max_depth {
            Err(self.error("TM2021", "parser nesting budget exceeded"))
        } else {
            Ok(depth)
        }
    }
    pub(super) fn error(&self, code: &'static str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, message, self.span(), self.source)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Infix {
    Pipe,
    Binary(BinaryOp),
}
