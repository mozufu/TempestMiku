use super::*;

impl Parser<'_> {
    pub(super) fn pattern(&mut self) -> Result<Pattern> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.pattern_inner();
        self.depth -= 1;
        result
    }

    pub(super) fn pattern_inner(&mut self) -> Result<Pattern> {
        let mut left = self.pattern_atom()?;
        if self.take(&Token::Cons).is_some() {
            let right = self.pattern()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(
                PatternKind::Cons {
                    head: Box::new(left),
                    tail: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    pub(super) fn pattern_atom(&mut self) -> Result<Pattern> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.pattern_atom_inner();
        self.depth -= 1;
        result
    }

    pub(super) fn pattern_atom_inner(&mut self) -> Result<Pattern> {
        let token = self.bump().clone();
        let node = match token.node {
            Token::Ident(name) if name == "_" => PatternKind::Wildcard,
            Token::Ident(name) if name == "true" => PatternKind::Bool(true),
            Token::Ident(name) if name == "false" => PatternKind::Bool(false),
            Token::Ident(name) if name == "null" => PatternKind::Null,
            Token::Ident(name) => PatternKind::Bind(name),
            Token::String(value) => PatternKind::String(value),
            Token::Int(value) => PatternKind::Int(value),
            Token::Upper(name) => {
                let payload = self
                    .starts_pattern()
                    .then(|| self.pattern_atom())
                    .transpose()?
                    .map(Box::new);
                PatternKind::Constructor { name, payload }
            }
            Token::LParen => {
                let value = self.pattern()?;
                self.expect(Token::RParen, ") after pattern")?;
                return Ok(Spanned::new(
                    value.node,
                    token.span.merge(self.previous_span()),
                ));
            }
            Token::LBracket => {
                let mut values = Vec::new();
                if !self.at(&Token::RBracket) {
                    loop {
                        values.push(self.pattern()?);
                        if self.take(&Token::Comma).is_none() {
                            break;
                        }
                    }
                }
                self.expect(Token::RBracket, "] after list pattern")?;
                return Ok(Spanned::new(
                    PatternKind::List(values),
                    token.span.merge(self.previous_span()),
                ));
            }
            Token::LBrace => {
                let mut fields = Vec::new();
                let mut rest = false;
                if !self.at(&Token::RBrace) {
                    loop {
                        if self.take(&Token::Ellipsis).is_some() {
                            rest = true;
                            break;
                        }
                        let name = self.lower_name("record pattern field")?;
                        let value = if self.take(&Token::Colon).is_some() {
                            self.pattern()?
                        } else {
                            Spanned::new(PatternKind::Bind(name.clone()), self.previous_span())
                        };
                        fields.push((name, value));
                        if self.take(&Token::Comma).is_none() {
                            break;
                        }
                        if self.take(&Token::Ellipsis).is_some() {
                            rest = true;
                            break;
                        }
                    }
                }
                self.expect(Token::RBrace, "} after record pattern")?;
                return Ok(Spanned::new(
                    PatternKind::Record { fields, rest },
                    token.span.merge(self.previous_span()),
                ));
            }
            _ => {
                return Err(Diagnostic::new(
                    "TM2010",
                    "expected pattern",
                    token.span,
                    self.source,
                ));
            }
        };
        Ok(Spanned::new(node, token.span))
    }
}
