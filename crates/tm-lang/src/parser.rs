use crate::{
    BinaryOp, Cell, Diagnostic, Expr, ExprKind, Form, FormKind, MatchArm, Pattern, PatternKind,
    Result, Span, Spanned, SpannedToken, Token, TypeDecl, TypeTerm, UnaryOp, VariantDecl,
    lexer::lex_bounded,
};

const DEFAULT_MAX_SOURCE_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_SYNTAX_NODES: usize = 100_000;
const DEFAULT_MAX_PARSE_DEPTH: usize = 256;

pub fn parse(source: &str) -> Result<Cell> {
    parse_bounded(
        source,
        DEFAULT_MAX_SOURCE_BYTES,
        DEFAULT_MAX_SYNTAX_NODES,
        DEFAULT_MAX_PARSE_DEPTH,
    )
}

pub(crate) fn parse_bounded(
    source: &str,
    max_source_bytes: usize,
    max_syntax_nodes: usize,
    max_depth: usize,
) -> Result<Cell> {
    if source.len() > max_source_bytes {
        return Err(Diagnostic::new(
            "TM2019",
            format!(
                "source budget exceeded: {} bytes exceeds {max_source_bytes}",
                source.len()
            ),
            Span::new(0, source.len()),
            source,
        ));
    }
    let tokens = lex_bounded(source, max_syntax_nodes)?;
    Parser {
        source,
        tokens,
        cursor: 0,
        stop_before_lbrace: false,
        depth: 0,
        max_depth,
        last_expr_depth: 0,
        last_form_expr_depth: 0,
    }
    .cell()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<SpannedToken>,
    cursor: usize,
    stop_before_lbrace: bool,
    depth: usize,
    max_depth: usize,
    last_expr_depth: usize,
    last_form_expr_depth: usize,
}

impl<'a> Parser<'a> {
    fn cell(&mut self) -> Result<Cell> {
        let start = self.span().start;
        let mut forms = Vec::new();
        if self.at(&Token::Eof) {
            return Err(self.error("TM2001", "empty cell is unsupported"));
        }
        loop {
            forms.push(self.form(false)?);
            if self.take(&Token::Semicolon).is_some() {
                if self.at(&Token::Eof) {
                    break;
                }
                if self.at(&Token::Semicolon) {
                    return Err(self.error("TM2002", "empty cell form is unsupported"));
                }
                continue;
            }
            break;
        }
        self.expect(Token::Eof, "end of cell")?;
        Ok(Cell {
            forms,
            span: Span::new(start, self.previous_span().end),
        })
    }

    fn form(&mut self, in_block: bool) -> Result<Form> {
        let start = self.span().start;
        let (node, expression_depth) = if self.keyword("type") {
            (FormKind::Type(self.type_decl()?), 0)
        } else if self.keyword("let") {
            self.bump();
            let pattern = self.pattern()?;
            self.expect(Token::Eq, "= after let pattern")?;
            let value = self.expr()?;
            let depth = self.last_expr_depth;
            (FormKind::Let { pattern, value }, depth)
        } else if self.keyword("fun") && self.peek_is_named_function() {
            self.bump();
            let name = self.lower_name("function name")?;
            let mut params = Vec::new();
            while !self.at(&Token::Eq) {
                params.push(self.pattern()?);
            }
            if params.is_empty() {
                return Err(self.error("TM2003", "named function requires at least one parameter"));
            }
            self.bump();
            let body = self.expr()?;
            let depth = self.last_expr_depth;
            (FormKind::Fun { name, params, body }, depth)
        } else {
            let expression = self.expr()?;
            let depth = self.last_expr_depth;
            (FormKind::Expr(expression), depth)
        };
        if !in_block && !self.at(&Token::Semicolon) && !self.at(&Token::Eof) {
            return Err(self.error("TM2004", "top-level forms must be separated by `;`"));
        }
        self.last_form_expr_depth = expression_depth;
        Ok(Spanned::new(
            node,
            Span::new(start, self.previous_span().end),
        ))
    }

    fn type_decl(&mut self) -> Result<TypeDecl> {
        self.expect_keyword("type")?;
        let name = self.upper_name("type name")?;
        self.expect(Token::Eq, "= after type name")?;
        self.expect(Token::Pipe, "| before first variant")?;
        let mut variants = Vec::new();
        loop {
            let variant = self.upper_name("variant name")?;
            let payload = if self.at(&Token::LBrace) {
                Some(TypeTerm::Record(self.schema_fields()?))
            } else if self.starts_type_term() {
                Some(self.type_term()?)
            } else {
                None
            };
            variants.push(VariantDecl {
                name: variant,
                payload,
            });
            if self.take(&Token::Pipe).is_none() {
                break;
            }
        }
        Ok(TypeDecl { name, variants })
    }

    fn schema_fields(&mut self) -> Result<Vec<(String, TypeTerm)>> {
        self.expect(Token::LBrace, "{")?;
        let mut fields = Vec::new();
        if !self.at(&Token::RBrace) {
            loop {
                let name = self.lower_name("schema field")?;
                self.expect(Token::Colon, ": after schema field")?;
                fields.push((name, self.type_term()?));
                if self.take(&Token::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(Token::RBrace, "} after schema")?;
        Ok(fields)
    }

    fn type_term(&mut self) -> Result<TypeTerm> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.type_term_inner();
        self.depth -= 1;
        result
    }

    fn type_term_inner(&mut self) -> Result<TypeTerm> {
        if self.keyword("List") || self.upper_is("List") {
            self.bump();
            return Ok(TypeTerm::List(Box::new(self.type_atom()?)));
        }
        if self.keyword("Option") || self.upper_is("Option") {
            self.bump();
            return Ok(TypeTerm::Option(Box::new(self.type_atom()?)));
        }
        self.type_atom()
    }

    fn type_atom(&mut self) -> Result<TypeTerm> {
        if self.take(&Token::LParen).is_some() {
            let term = self.type_term()?;
            self.expect(Token::RParen, ") after type")?;
            return Ok(term);
        }
        match self.bump().node.clone() {
            Token::Upper(name) | Token::Ident(name) => Ok(TypeTerm::Named(name)),
            _ => Err(self.error("TM2005", "expected type term")),
        }
    }

    fn expr(&mut self) -> Result<Expr> {
        self.parse_bp(0)
    }

    fn parse_bp(&mut self, min_bp: u8) -> Result<Expr> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.parse_bp_inner(min_bp);
        self.depth -= 1;
        result
    }

    fn parse_bp_inner(&mut self, min_bp: u8) -> Result<Expr> {
        let mut left = if self.keyword("not") {
            let start = self.bump().span;
            let value = self.parse_bp(80)?;
            let depth = self.parent_expr_depth(self.last_expr_depth)?;
            self.last_expr_depth = depth;
            Spanned::new(
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    value: Box::new(value.clone()),
                },
                start.merge(value.span),
            )
        } else if self.take(&Token::Minus).is_some() {
            let start = self.previous_span();
            let value = self.parse_bp(80)?;
            let depth = self.parent_expr_depth(self.last_expr_depth)?;
            self.last_expr_depth = depth;
            Spanned::new(
                ExprKind::Unary {
                    op: UnaryOp::Negate,
                    value: Box::new(value.clone()),
                },
                start.merge(value.span),
            )
        } else {
            self.atom()?
        };
        let mut left_depth = self.last_expr_depth;

        loop {
            if self.at(&Token::Dot) && 100 >= min_bp {
                self.bump();
                let field = self.lower_name("field name")?;
                let span = left.span.merge(self.previous_span());
                left = Spanned::new(
                    ExprKind::Field {
                        target: Box::new(left),
                        field,
                    },
                    span,
                );
                left_depth = self.parent_expr_depth(left_depth)?;
                continue;
            }
            if self.starts_atom() && 90 >= min_bp {
                let argument = self.parse_bp(91)?;
                let argument_depth = self.last_expr_depth;
                let span = left.span.merge(argument.span);
                left = Spanned::new(
                    ExprKind::Apply {
                        function: Box::new(left),
                        argument: Box::new(argument),
                    },
                    span,
                );
                left_depth = self.parent_expr_depth(left_depth.max(argument_depth))?;
                continue;
            }
            let Some((left_bp, right_bp, operator)) = self.infix_binding() else {
                break;
            };
            if left_bp < min_bp {
                break;
            }
            self.bump();
            let right = self.parse_bp(right_bp)?;
            let right_depth = self.last_expr_depth;
            let span = left.span.merge(right.span);
            left = match operator {
                Infix::Pipe => Spanned::new(
                    ExprKind::Pipe {
                        value: Box::new(left),
                        target: Box::new(right),
                    },
                    span,
                ),
                Infix::Binary(op) => Spanned::new(
                    ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                ),
            };
            left_depth = self.parent_expr_depth(left_depth.max(right_depth))?;
        }
        self.last_expr_depth = left_depth;
        Ok(left)
    }

    fn atom(&mut self) -> Result<Expr> {
        let start = self.span();
        if self.keyword("if") {
            return self.if_expr();
        }
        if self.keyword("match") {
            return self.match_expr(false);
        }
        if self.keyword("handle") {
            return self.match_expr(true);
        }
        if self.keyword("do") {
            return self.block();
        }
        if self.keyword("fun") {
            return self.lambda();
        }
        let token = self.bump().clone();
        let node = match token.node {
            Token::String(value) => ExprKind::String(value),
            Token::Int(value) => ExprKind::Int(value),
            Token::Decimal(value) => ExprKind::Decimal(value),
            Token::Uri(value) => ExprKind::Uri(value),
            Token::Ident(name) if name == "true" => ExprKind::Bool(true),
            Token::Ident(name) if name == "false" => ExprKind::Bool(false),
            Token::Ident(name) if name == "null" => ExprKind::Null,
            Token::Ident(name) => ExprKind::Name(name),
            Token::Upper(name) => ExprKind::Constructor(name),
            Token::At => return self.capability(start.start),
            Token::LParen => {
                let stop_before_lbrace = self.stop_before_lbrace;
                self.stop_before_lbrace = false;
                let value = self.expr();
                self.stop_before_lbrace = stop_before_lbrace;
                let value = value?;
                let depth = self.last_expr_depth;
                self.expect(Token::RParen, ") after expression")?;
                self.last_expr_depth = depth;
                return Ok(Spanned::new(value.node, start.merge(self.previous_span())));
            }
            Token::LBracket => return self.list(start.start),
            Token::LBrace => return self.record(start.start),
            _ => {
                return Err(Diagnostic::new(
                    "TM2006",
                    "expected expression",
                    token.span,
                    self.source,
                ));
            }
        };
        self.last_expr_depth = 1;
        Ok(Spanned::new(node, token.span))
    }

    fn if_expr(&mut self) -> Result<Expr> {
        let start = self.bump().span;
        let condition = self.expr()?;
        let condition_depth = self.last_expr_depth;
        self.expect_keyword("then")?;
        let then_value = self.expr()?;
        let then_depth = self.last_expr_depth;
        self.expect_keyword("else")?;
        let else_value = self.expr()?;
        let else_depth = self.last_expr_depth;
        let span = start.merge(else_value.span);
        self.last_expr_depth =
            self.parent_expr_depth(condition_depth.max(then_depth).max(else_depth))?;
        Ok(Spanned::new(
            ExprKind::If {
                condition: Box::new(condition),
                then_value: Box::new(then_value),
                else_value: Box::new(else_value),
            },
            span,
        ))
    }

    fn match_expr(&mut self, handle: bool) -> Result<Expr> {
        let start = self.bump().span;
        self.stop_before_lbrace = true;
        let value = self.expr()?;
        let mut child_depth = self.last_expr_depth;
        self.stop_before_lbrace = false;
        if handle {
            self.expect_keyword("with")?;
            self.expect_keyword("error")?;
        }
        self.expect(Token::LBrace, "{ before match arms")?;
        let mut arms = Vec::new();
        while !self.at(&Token::RBrace) {
            self.expect(Token::Pipe, "| before match arm")?;
            let pattern = self.pattern()?;
            self.expect(Token::Arrow, "-> in match arm")?;
            let value = self.expr()?;
            child_depth = child_depth.max(self.last_expr_depth);
            arms.push(MatchArm { pattern, value });
        }
        self.expect(Token::RBrace, "} after match arms")?;
        if arms.is_empty() {
            return Err(self.error("TM2007", "match requires at least one arm"));
        }
        let span = start.merge(self.previous_span());
        let node = if handle {
            ExprKind::Handle {
                value: Box::new(value),
                arms,
            }
        } else {
            ExprKind::Match {
                value: Box::new(value),
                arms,
            }
        };
        self.last_expr_depth = self.parent_expr_depth(child_depth)?;
        Ok(Spanned::new(node, span))
    }

    fn block(&mut self) -> Result<Expr> {
        let start = self.bump().span;
        self.expect(Token::LBrace, "{ after do")?;
        let mut forms = Vec::new();
        let mut child_depth = 0;
        if !self.at(&Token::RBrace) {
            loop {
                forms.push(self.form(true)?);
                child_depth = child_depth.max(self.last_form_expr_depth);
                if self.take(&Token::Semicolon).is_none() {
                    break;
                }
                if self.at(&Token::RBrace) {
                    break;
                }
            }
        }
        self.expect(Token::RBrace, "} after do block")?;
        if forms.is_empty() {
            return Err(self.error("TM2008", "do block cannot be empty"));
        }
        self.last_expr_depth = self.parent_expr_depth(child_depth)?;
        Ok(Spanned::new(
            ExprKind::Block(forms),
            start.merge(self.previous_span()),
        ))
    }

    fn lambda(&mut self) -> Result<Expr> {
        let start = self.bump().span;
        let mut params = Vec::new();
        while !self.at(&Token::Arrow) {
            params.push(self.pattern()?);
        }
        if params.is_empty() {
            return Err(self.error("TM2009", "lambda requires at least one parameter"));
        }
        self.bump();
        let body = self.expr()?;
        let body_depth = self.last_expr_depth;
        let span = start.merge(body.span);
        self.last_expr_depth = self.parent_expr_depth(body_depth)?;
        Ok(Spanned::new(
            ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            span,
        ))
    }

    fn capability(&mut self, start: usize) -> Result<Expr> {
        let mut parts = vec![self.lower_name("capability name")?];
        while self.take(&Token::Dot).is_some() {
            parts.push(self.lower_name("capability segment")?);
        }
        self.last_expr_depth = 1;
        Ok(Spanned::new(
            ExprKind::Capability(parts.join(".")),
            Span::new(start, self.previous_span().end),
        ))
    }

    fn list(&mut self, start: usize) -> Result<Expr> {
        let mut values = Vec::new();
        let mut child_depth = 0;
        if !self.at(&Token::RBracket) {
            loop {
                values.push(self.expr()?);
                child_depth = child_depth.max(self.last_expr_depth);
                if self.take(&Token::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(Token::RBracket, "] after list")?;
        self.last_expr_depth = self.parent_expr_depth(child_depth)?;
        Ok(Spanned::new(
            ExprKind::List(values),
            Span::new(start, self.previous_span().end),
        ))
    }

    fn record(&mut self, start: usize) -> Result<Expr> {
        let mut fields = Vec::new();
        let mut child_depth = 0;
        if !self.at(&Token::RBrace) {
            loop {
                let name = self.lower_name("record field")?;
                let value = if self.take(&Token::Colon).is_some() {
                    let value = self.expr()?;
                    child_depth = child_depth.max(self.last_expr_depth);
                    value
                } else {
                    child_depth = child_depth.max(1);
                    Spanned::new(ExprKind::Name(name.clone()), self.previous_span())
                };
                fields.push((name, value));
                if self.take(&Token::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(Token::RBrace, "} after record")?;
        self.last_expr_depth = self.parent_expr_depth(child_depth)?;
        Ok(Spanned::new(
            ExprKind::Record(fields),
            Span::new(start, self.previous_span().end),
        ))
    }

    fn pattern(&mut self) -> Result<Pattern> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.pattern_inner();
        self.depth -= 1;
        result
    }

    fn pattern_inner(&mut self) -> Result<Pattern> {
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

    fn pattern_atom(&mut self) -> Result<Pattern> {
        if self.depth >= self.max_depth {
            return Err(self.error("TM2021", "parser nesting budget exceeded"));
        }
        self.depth += 1;
        let result = self.pattern_atom_inner();
        self.depth -= 1;
        result
    }

    fn pattern_atom_inner(&mut self) -> Result<Pattern> {
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

    fn infix_binding(&self) -> Option<(u8, u8, Infix)> {
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

    fn peek_is_named_function(&self) -> bool {
        matches!(
            self.tokens.get(self.cursor + 1).map(|token| &token.node),
            Some(Token::Ident(_))
        ) && !matches!(
            self.tokens.get(self.cursor + 2).map(|token| &token.node),
            Some(Token::Arrow)
        )
    }
    fn starts_atom(&self) -> bool {
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
    fn starts_pattern(&self) -> bool {
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
    fn starts_type_term(&self) -> bool {
        matches!(
            &self.current().node,
            Token::Upper(_) | Token::Ident(_) | Token::LParen
        )
    }
    fn keyword(&self, keyword: &str) -> bool {
        matches!(&self.current().node, Token::Ident(name) if name == keyword)
    }
    fn upper_is(&self, value: &str) -> bool {
        matches!(&self.current().node, Token::Upper(name) if name == value)
    }
    fn expect_keyword(&mut self, keyword: &'static str) -> Result<()> {
        if self.keyword(keyword) {
            self.bump();
            Ok(())
        } else {
            Err(self.error("TM2011", format!("expected {keyword}")))
        }
    }
    fn lower_name(&mut self, expected: &'static str) -> Result<String> {
        match self.bump().node.clone() {
            Token::Ident(name) => Ok(name),
            _ => Err(self.error("TM2012", format!("expected {expected}"))),
        }
    }
    fn upper_name(&mut self, expected: &'static str) -> Result<String> {
        match self.bump().node.clone() {
            Token::Upper(name) => Ok(name),
            _ => Err(self.error("TM2013", format!("expected {expected}"))),
        }
    }
    fn expect(&mut self, token: Token, expected: &'static str) -> Result<Span> {
        self.take(&token)
            .ok_or_else(|| self.error("TM2014", format!("expected {expected}")))
    }
    fn take(&mut self, token: &Token) -> Option<Span> {
        self.at(token).then(|| self.bump().span)
    }
    fn at(&self, token: &Token) -> bool {
        std::mem::discriminant(&self.current().node) == std::mem::discriminant(token)
    }
    fn current(&self) -> &SpannedToken {
        &self.tokens[self.cursor]
    }
    fn bump(&mut self) -> &SpannedToken {
        let index = self.cursor;
        if !self.at(&Token::Eof) {
            self.cursor += 1;
        }
        &self.tokens[index]
    }
    fn span(&self) -> Span {
        self.current().span
    }
    fn previous_span(&self) -> Span {
        self.tokens[self.cursor.saturating_sub(1)].span
    }
    fn parent_expr_depth(&self, child_depth: usize) -> Result<usize> {
        let depth = child_depth.saturating_add(1);
        if depth > self.max_depth {
            Err(self.error("TM2021", "parser nesting budget exceeded"))
        } else {
            Ok(depth)
        }
    }
    fn error(&self, code: &'static str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, message, self.span(), self.source)
    }
}

#[derive(Clone, Copy)]
enum Infix {
    Pipe,
    Binary(BinaryOp),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_and_pipe_follow_precedence() {
        let cell = parse("text |> split \",\" |> filter predicate").unwrap();
        assert!(matches!(
            cell.forms[0].node,
            FormKind::Expr(Expr {
                node: ExprKind::Pipe { .. },
                ..
            })
        ));
    }

    #[test]
    fn parses_local_sum_match_and_effect() {
        let source = "do { type Finding = | Hit {file: String, line: Int} | Ignored Path; let x = Hit {file: \"a\", line: 1}; match x { | Hit {file, line} -> @fs.read workspace:a | Ignored _ -> null } }";
        parse(source).unwrap();
    }

    #[test]
    fn parses_handled_effect_with_record_arguments() {
        parse(
            r#"handle (@proc.run {cmd: "cargo", args: ["clean"], cwd: "repo:"}) with error { | error -> error }"#,
        )
        .unwrap();
    }

    #[test]
    fn rejects_implicit_top_level_sequence() {
        let error = parse("let x = 1 let y = 2").unwrap_err();
        assert_eq!(error.code, "TM2004");
    }

    #[test]
    fn nesting_budget_covers_every_recursive_pattern_form() {
        let nested_parens = format!("let {}x{} = null", "(".repeat(16), ")".repeat(16));
        let nested_lists = format!("let {}x{} = null", "[".repeat(16), "]".repeat(16));
        let nested_constructors = format!("let {}x = null", "Some ".repeat(16));
        let nested_cons = format!("let {}x = null", "x :: ".repeat(16));

        for source in [
            nested_parens,
            nested_lists,
            nested_constructors,
            nested_cons,
        ] {
            let error = parse_bounded(&source, 4096, 4096, 8).unwrap_err();
            assert_eq!(error.code, "TM2021", "{source}");
        }
    }

    #[test]
    fn nesting_budget_covers_flat_left_associative_expression_chains() {
        let fields = format!("value{}", ".field".repeat(32));
        let applications = format!("function{}", " argument".repeat(32));
        let infix = std::iter::repeat_n("1", 32).collect::<Vec<_>>().join(" + ");
        let pipes = std::iter::repeat_n("value", 32)
            .collect::<Vec<_>>()
            .join(" |> ");

        for source in [fields, applications, infix, pipes] {
            let error = parse_bounded(&source, 4096, 4096, 8).unwrap_err();
            assert_eq!(error.code, "TM2021", "{source}");
        }
    }
}
