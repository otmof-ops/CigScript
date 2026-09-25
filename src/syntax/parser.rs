// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Recursive-descent parser with precedence climbing for expressions.
//!
//! Statements end at a newline, a `;`, a `}` or end of file. Newlines are
//! ignored inside parentheses, brackets and map braces, and directly after
//! a binary operator or a comma, so long expressions can wrap.

use super::ast::*;
use super::lexer::{tokenize, Lexer};
use super::span::Span;
use super::token::{StrPart, Token, TokenKind};
use crate::diagnostics::{syntax, Diagnostic};
use std::rc::Rc;

pub fn parse(src: &str) -> Result<Program, Diagnostic> {
    let tokens = tokenize(src)?;
    Parser::new(tokens).program()
}

/// Parse a single expression (used by `cig eval` and string interpolation).
pub fn parse_expr_src(src: &str, origin: Span) -> Result<Expr, Diagnostic> {
    let tokens = Lexer::with_origin(src, origin).run()?;
    let mut p = Parser::new(tokens);
    p.skip_newlines();
    let expr = p.expr()?;
    p.skip_newlines();
    if p.peek() != &TokenKind::Eof {
        return Err(syntax(
            format!("unexpected {} after expression", p.peek().describe()),
            p.span(),
        ));
    }
    Ok(expr)
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ----- token helpers -------------------------------------------------

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<Token, Diagnostic> {
        if self.at(&kind) {
            Ok(self.advance())
        } else {
            Err(syntax(
                format!("expected {what}, found {}", self.peek().describe()),
                self.span(),
            ))
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) {
            self.advance();
        }
    }

    fn ident(&mut self, what: &str) -> Result<(String, Span), Diagnostic> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let t = self.advance();
                Ok((name, t.span))
            }
            other => Err(syntax(
                format!("expected {what}, found {}", other.describe()),
                self.span(),
            )),
        }
    }

    /// A member name after `.`: an identifier, or a keyword used as a name.
    fn member_name(&mut self) -> Result<(String, Span), Diagnostic> {
        if let Some(word) = self.peek().keyword_text() {
            let t = self.advance();
            return Ok((word.to_string(), t.span));
        }
        self.ident("a member name after `.`")
    }

    /// A statement must be followed by a terminator.
    fn end_stmt(&mut self) -> Result<(), Diagnostic> {
        match self.peek() {
            TokenKind::Newline | TokenKind::Semicolon => {
                self.advance();
                self.skip_newlines();
                Ok(())
            }
            TokenKind::RBrace | TokenKind::Eof => Ok(()),
            other => Err(syntax(
                format!("expected end of statement, found {}", other.describe()),
                self.span(),
            )
            .code("E203")
            .with_hint("put each statement on its own line, or separate them with `;`")),
        }
    }

    /// `exhael x`: a lone identifier followed by more tokens on the same line
    /// is almost always a misspelled keyword.
    fn misspelled_keyword(&self, expr: &Expr) -> Option<Diagnostic> {
        let ExprKind::Ident(word) = &expr.kind else {
            return None;
        };
        if self.stmt_ended()
            || matches!(
                self.peek(),
                TokenKind::Assign
                    | TokenKind::PlusAssign
                    | TokenKind::MinusAssign
                    | TokenKind::StarAssign
                    | TokenKind::SlashAssign
            )
        {
            return None;
        }
        const KEYWORDS: &[&str] = &[
            "roll", "stick", "pull", "chain", "snuff", "exhale", "cough", "try", "burn", "if",
            "else", "while", "for", "break", "continue",
        ];
        let close = crate::interp::closest(word, KEYWORDS.iter().copied())?;
        Some(
            syntax(format!("`{word}` is not a statement"), expr.span)
                .code("E204")
                .with_hint(format!("did you mean `{close}`?")),
        )
    }

    // ----- program and statements ------------------------------------------

    pub fn program(&mut self) -> Result<Program, Diagnostic> {
        let mut body = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::RBrace) {
                return Err(syntax("unexpected `}` with no open block", self.span()));
            }
            body.push(self.stmt()?);
            self.skip_newlines();
        }
        Ok(Program { body })
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        let open = self.expect(TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(syntax("this `{` is never closed", open.span)
                    .code("E202")
                    .with_hint("add a matching `}`"));
            }
            stmts.push(self.stmt()?);
            self.skip_newlines();
        }
        let close = self.advance();
        Ok(Block {
            stmts,
            span: open.span.to(close.span),
        })
    }

    fn stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.span();
        match self.peek().clone() {
            TokenKind::Roll | TokenKind::Stick => {
                let mutable = matches!(self.advance().kind, TokenKind::Roll);
                let (name, _) = self.ident("a name")?;
                if !self.eat(&TokenKind::Assign) {
                    return Err(syntax(
                        format!(
                            "expected `=` after `{name}`, found {}",
                            self.peek().describe()
                        ),
                        self.span(),
                    )
                    .with_hint(if mutable {
                        "roll x = value"
                    } else {
                        "stick x = value"
                    }));
                }
                self.skip_newlines();
                let value = self.expr()?;
                self.end_stmt()?;
                Ok(Stmt::Declare {
                    name,
                    mutable,
                    value,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Pull => {
                self.advance();
                let (name, _) = self.ident("a function name after `pull`")?;
                let params = self.params()?;
                let body = self.block()?;
                Ok(Stmt::Pull(FnDecl {
                    name,
                    params,
                    body: Rc::new(body),
                    span: start.to(self.prev_span()),
                }))
            }
            TokenKind::Chain => {
                self.advance();
                let (name, _) = self.ident("a chain name after `chain`")?;
                let open = self.expect(TokenKind::LBrace, "`{`")?;
                let mut steps = Vec::new();
                self.skip_newlines();
                while !self.at(&TokenKind::RBrace) {
                    if self.at(&TokenKind::Eof) {
                        return Err(syntax("this `{` is never closed", open.span)
                            .code("E202")
                            .with_hint("add a matching `}`"));
                    }
                    steps.push(self.expr()?);
                    match self.peek() {
                        TokenKind::Comma | TokenKind::Newline | TokenKind::Semicolon => {
                            self.advance();
                            self.skip_newlines();
                        }
                        TokenKind::RBrace => {}
                        other => {
                            return Err(syntax(
                                format!(
                                    "expected a comma or a new line between chain steps, found {}",
                                    other.describe()
                                ),
                                self.span(),
                            ))
                        }
                    }
                }
                self.advance();
                if steps.is_empty() {
                    return Err(syntax(
                        format!("chain `{name}` has no steps"),
                        start.to(self.prev_span()),
                    )
                    .code("E207")
                    .with_hint(
                        "list the sticks to light, one per line: chain name { fetch, build }",
                    ));
                }
                Ok(Stmt::Chain(ChainDecl {
                    name,
                    steps,
                    span: start.to(self.prev_span()),
                }))
            }
            TokenKind::Snuff => {
                self.advance();
                let value = if self.stmt_ended() {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.end_stmt()?;
                Ok(Stmt::Snuff {
                    value,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Exhale => {
                self.advance();
                let mut values = Vec::new();
                if !self.stmt_ended() {
                    values.push(self.expr()?);
                    while self.eat(&TokenKind::Comma) {
                        self.skip_newlines();
                        values.push(self.expr()?);
                    }
                }
                self.end_stmt()?;
                Ok(Stmt::Exhale {
                    values,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Cough => {
                self.advance();
                if self.stmt_ended() {
                    return Err(syntax("`cough` needs a value to raise", start)
                        .with_hint("cough \"what went wrong\""));
                }
                let value = self.expr()?;
                self.end_stmt()?;
                Ok(Stmt::Cough {
                    value,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::If => self.if_stmt(),
            TokenKind::While => {
                self.advance();
                let cond = self.expr()?;
                let body = self.block()?;
                Ok(Stmt::While {
                    cond,
                    body,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::For => {
                self.advance();
                let (var, _) = self.ident("a loop variable after `for`")?;
                self.expect(TokenKind::In, "`in`")?;
                let iter = self.expr()?;
                let body = self.block()?;
                Ok(Stmt::For {
                    var,
                    iter,
                    body,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Break => {
                self.advance();
                self.end_stmt()?;
                Ok(Stmt::Break(start))
            }
            TokenKind::Continue => {
                self.advance();
                self.end_stmt()?;
                Ok(Stmt::Continue(start))
            }
            TokenKind::Try => {
                self.advance();
                let body = self.block()?;
                self.skip_newlines();
                if !self.eat(&TokenKind::Ashtray) {
                    return Err(syntax(
                        format!(
                            "expected `ashtray` after the `try` block, found {}",
                            self.peek().describe()
                        ),
                        self.span(),
                    )
                    .with_hint("try { ... } ashtray err { ... }"));
                }
                let catch_var = match self.peek().clone() {
                    TokenKind::Ident(name) => {
                        self.advance();
                        Some(name)
                    }
                    _ => None,
                };
                let handler = self.block()?;
                Ok(Stmt::Try {
                    body,
                    catch_var,
                    handler,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Burn => {
                self.advance();
                let class = if self.eat(&TokenKind::Unlit) {
                    BurnClass::Unlit
                } else {
                    BurnClass::Normal
                };
                if !self.at(&TokenKind::LBrace) {
                    return Err(syntax(
                        format!(
                            "expected `{{` after `burn`, found {}",
                            self.peek().describe()
                        ),
                        self.span(),
                    )
                    .with_hint("burn { ... } or burn unlit { ... }"));
                }
                let body = self.block()?;
                Ok(Stmt::Burn {
                    class,
                    body,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Else => Err(syntax("`else` without a preceding `if`", start)),
            TokenKind::Ashtray => Err(syntax("`ashtray` without a preceding `try`", start)),
            TokenKind::LBrace => Err(syntax("a bare `{` cannot start a statement", start)
                .with_hint(
                    "did you mean `burn { ... }`? Map literals are only valid in expressions",
                )),
            _ => self.expr_or_assign_stmt(),
        }
    }

    fn stmt_ended(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
        )
    }

    fn if_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.span();
        self.expect(TokenKind::If, "`if`")?;
        let mut branches = Vec::new();
        let cond = self.expr()?;
        let body = self.block()?;
        branches.push((cond, body));
        let mut otherwise = None;
        loop {
            // `else` may sit on the next line.
            let save = self.pos;
            self.skip_newlines();
            if !self.eat(&TokenKind::Else) {
                self.pos = save;
                break;
            }
            if self.eat(&TokenKind::If) {
                let cond = self.expr()?;
                let body = self.block()?;
                branches.push((cond, body));
            } else {
                otherwise = Some(self.block()?);
                break;
            }
        }
        Ok(Stmt::If {
            branches,
            otherwise,
            span: start.to(self.prev_span()),
        })
    }

    fn expr_or_assign_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.span();
        let expr = self.expr()?;
        let op = match self.peek() {
            TokenKind::Assign => Some(AssignOp::Set),
            TokenKind::PlusAssign => Some(AssignOp::Add),
            TokenKind::MinusAssign => Some(AssignOp::Sub),
            TokenKind::StarAssign => Some(AssignOp::Mul),
            TokenKind::SlashAssign => Some(AssignOp::Div),
            _ => None,
        };
        if op.is_none() {
            if let Some(d) = self.misspelled_keyword(&expr) {
                return Err(d);
            }
        }
        if let Some(op) = op {
            let op_span = self.span();
            self.advance();
            let target = match expr.kind {
                ExprKind::Ident(name) => AssignTarget::Var(name),
                ExprKind::Index { object, index } => AssignTarget::Index {
                    object: *object,
                    index: *index,
                },
                ExprKind::Member {
                    object,
                    name,
                    safe: false,
                } => AssignTarget::Member {
                    object: *object,
                    name,
                },
                _ => {
                    return Err(
                        syntax("this is not something that can be assigned to", expr.span)
                            .code("E205")
                            .with_hint("assign to a name, `list[i]` or `map.key`"),
                    )
                }
            };
            self.skip_newlines();
            let value = self.expr()?;
            self.end_stmt()?;
            let _ = op_span;
            return Ok(Stmt::Assign {
                target,
                op,
                value,
                span: start.to(self.prev_span()),
            });
        }
        self.end_stmt()?;
        Ok(Stmt::Expr(expr))
    }

    fn params(&mut self) -> Result<Vec<String>, Diagnostic> {
        self.expect(TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RParen) {
            let (name, span) = self.ident("a parameter name")?;
            if params.contains(&name) {
                return Err(syntax(format!("duplicate parameter `{name}`"), span));
            }
            params.push(name);
            self.skip_newlines();
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(TokenKind::RParen, "`)` to close the parameter list")?;
        Ok(params)
    }

    // ----- expressions -------------------------------------------------------

    pub fn expr(&mut self) -> Result<Expr, Diagnostic> {
        self.pipe()
    }

    fn pipe(&mut self) -> Result<Expr, Diagnostic> {
        let mut lhs = self.or()?;
        while self.at(&TokenKind::Pipe) {
            self.advance();
            self.skip_newlines();
            let rhs = self.or()?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr::new(
                ExprKind::Pipe {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Ok(lhs)
    }

    fn binary_left<F>(&mut self, ops: &[(TokenKind, BinOp)], next: F) -> Result<Expr, Diagnostic>
    where
        F: Fn(&mut Self) -> Result<Expr, Diagnostic>,
    {
        let mut lhs = next(self)?;
        'outer: loop {
            for (tok, op) in ops {
                if self.at(tok) {
                    self.advance();
                    self.skip_newlines();
                    let rhs = next(self)?;
                    let span = lhs.span.to(rhs.span);
                    lhs = Expr::new(
                        ExprKind::Binary {
                            op: *op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        span,
                    );
                    continue 'outer;
                }
            }
            break;
        }
        Ok(lhs)
    }

    fn or(&mut self) -> Result<Expr, Diagnostic> {
        self.binary_left(&[(TokenKind::Or, BinOp::Or)], Self::and)
    }

    fn and(&mut self) -> Result<Expr, Diagnostic> {
        self.binary_left(&[(TokenKind::And, BinOp::And)], Self::not)
    }

    fn not(&mut self) -> Result<Expr, Diagnostic> {
        if self.at(&TokenKind::Not) {
            let start = self.span();
            self.advance();
            let expr = self.not()?;
            let span = start.to(expr.span);
            return Ok(Expr::new(
                ExprKind::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                },
                span,
            ));
        }
        self.coalesce()
    }

    fn coalesce(&mut self) -> Result<Expr, Diagnostic> {
        self.binary_left(&[(TokenKind::Coalesce, BinOp::Coalesce)], Self::comparison)
    }

    fn comparison(&mut self) -> Result<Expr, Diagnostic> {
        let lhs = self.range()?;
        let op = match self.peek() {
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::NotEq => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::LtEq => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::GtEq => BinOp::Ge,
            _ => return Ok(lhs),
        };
        self.advance();
        self.skip_newlines();
        let rhs = self.range()?;
        let span = lhs.span.to(rhs.span);
        let expr = Expr::new(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            span,
        );
        if matches!(
            self.peek(),
            TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::LtEq
                | TokenKind::Gt
                | TokenKind::GtEq
        ) {
            return Err(syntax("comparisons cannot be chained", self.span())
                .code("E206")
                .with_hint("write `a < b and b < c`"));
        }
        Ok(expr)
    }

    fn range(&mut self) -> Result<Expr, Diagnostic> {
        let lhs = self.additive()?;
        if self.at(&TokenKind::DotDot) {
            self.advance();
            self.skip_newlines();
            let rhs = self.additive()?;
            let span = lhs.span.to(rhs.span);
            return Ok(Expr::new(
                ExprKind::Binary {
                    op: BinOp::Range,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            ));
        }
        Ok(lhs)
    }

    fn additive(&mut self) -> Result<Expr, Diagnostic> {
        self.binary_left(
            &[
                (TokenKind::Plus, BinOp::Add),
                (TokenKind::Minus, BinOp::Sub),
            ],
            Self::multiplicative,
        )
    }

    fn multiplicative(&mut self) -> Result<Expr, Diagnostic> {
        self.binary_left(
            &[
                (TokenKind::Star, BinOp::Mul),
                (TokenKind::Slash, BinOp::Div),
                (TokenKind::Percent, BinOp::Rem),
            ],
            Self::unary,
        )
    }

    fn unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.at(&TokenKind::Minus) {
            let start = self.span();
            self.advance();
            let expr = self.unary()?;
            let span = start.to(expr.span);
            return Ok(Expr::new(
                ExprKind::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                },
                span,
            ));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.primary()?;
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.advance();
                    let args = self.args(TokenKind::RParen, "`)`")?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                TokenKind::Dot | TokenKind::SafeDot => {
                    let safe = matches!(self.advance().kind, TokenKind::SafeDot);
                    let (name, _) = self.member_name()?;
                    if self.at(&TokenKind::LParen) {
                        self.advance();
                        let args = self.args(TokenKind::RParen, "`)`")?;
                        let span = expr.span.to(self.prev_span());
                        expr = Expr::new(
                            ExprKind::Method {
                                receiver: Box::new(expr),
                                name,
                                args,
                                safe,
                            },
                            span,
                        );
                    } else {
                        let span = expr.span.to(self.prev_span());
                        expr = Expr::new(
                            ExprKind::Member {
                                object: Box::new(expr),
                                name,
                                safe,
                            },
                            span,
                        );
                    }
                }
                TokenKind::LBracket => {
                    self.advance();
                    self.skip_newlines();
                    let index = self.expr()?;
                    self.skip_newlines();
                    self.expect(TokenKind::RBracket, "`]`")?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr::new(
                        ExprKind::Index {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// Parse a comma-separated list up to `close` (already past the opener).
    fn args(&mut self, close: TokenKind, what: &str) -> Result<Vec<Expr>, Diagnostic> {
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&close) {
            if self.at(&TokenKind::Eof) {
                return Err(syntax(format!("expected {what}"), self.span()));
            }
            items.push(self.expr()?);
            self.skip_newlines();
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(close, what)?;
        Ok(items)
    }

    fn primary(&mut self) -> Result<Expr, Diagnostic> {
        let tok = self.advance();
        let span = tok.span;
        let kind = match tok.kind {
            TokenKind::Null => ExprKind::Null,
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Int(v) => ExprKind::Int(v),
            TokenKind::Float(v) => ExprKind::Float(v),
            TokenKind::Str(parts) => ExprKind::Str(self.string_pieces(parts)?),
            TokenKind::Ident(name) => ExprKind::Ident(name),
            TokenKind::LParen => {
                self.skip_newlines();
                let inner = self.expr()?;
                self.skip_newlines();
                self.expect(TokenKind::RParen, "`)`")?;
                return Ok(Expr::new(inner.kind, span.to(self.prev_span())));
            }
            TokenKind::LBracket => {
                let items = self.args(TokenKind::RBracket, "`]`")?;
                ExprKind::List(items)
            }
            TokenKind::LBrace => ExprKind::Map(self.map_entries()?),
            TokenKind::Pack => {
                let params = self.params()?;
                let body = if self.eat(&TokenKind::FatArrow) {
                    self.skip_newlines();
                    LambdaBody::Expr(Rc::new(self.expr()?))
                } else if self.at(&TokenKind::LBrace) {
                    LambdaBody::Block(Rc::new(self.block()?))
                } else {
                    return Err(syntax(
                        format!(
                            "expected `=>` or `{{` after pack parameters, found {}",
                            self.peek().describe()
                        ),
                        self.span(),
                    )
                    .with_hint("pack(x) => x + 1   or   pack(x) { snuff x + 1 }"));
                };
                ExprKind::Lambda { params, body }
            }
            TokenKind::Newline | TokenKind::Eof => {
                return Err(syntax("expected an expression, found end of line", span).code("E201"))
            }
            other => {
                return Err(syntax(
                    format!("expected an expression, found {}", other.describe()),
                    span,
                )
                .code("E201"))
            }
        };
        Ok(Expr::new(kind, span.to(self.prev_span())))
    }

    fn map_entries(&mut self) -> Result<Vec<(MapKey, Expr)>, Diagnostic> {
        let mut entries = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let key = match self.peek().clone() {
                TokenKind::Ident(name) => {
                    self.advance();
                    MapKey::Static(name)
                }
                kw if kw.keyword_text().is_some()
                    && self.tokens[self.pos + 1].kind == TokenKind::Colon =>
                {
                    self.advance();
                    MapKey::Static(kw.keyword_text().unwrap_or_default().to_string())
                }
                TokenKind::Str(parts) => {
                    let t = self.advance();
                    let pieces = self.string_pieces(parts)?;
                    if pieces.iter().all(|p| matches!(p, StrPiece::Lit(_))) {
                        let text = pieces
                            .into_iter()
                            .map(|p| match p {
                                StrPiece::Lit(s) => s,
                                StrPiece::Expr(_) => unreachable!(),
                            })
                            .collect::<String>();
                        MapKey::Static(text)
                    } else {
                        MapKey::Dynamic(Expr::new(ExprKind::Str(pieces), t.span))
                    }
                }
                TokenKind::LBracket => {
                    self.advance();
                    self.skip_newlines();
                    let e = self.expr()?;
                    self.skip_newlines();
                    self.expect(TokenKind::RBracket, "`]`")?;
                    MapKey::Dynamic(e)
                }
                TokenKind::Int(v) => {
                    self.advance();
                    MapKey::Static(v.to_string())
                }
                other => {
                    return Err(syntax(
                        format!("expected a map key, found {}", other.describe()),
                        self.span(),
                    )
                    .with_hint(
                        "keys look like `name: value`, `\"text\": value` or `[expr]: value`",
                    ))
                }
            };
            self.expect(TokenKind::Colon, "`:` after the map key")?;
            self.skip_newlines();
            let value = self.expr()?;
            entries.push((key, value));
            self.skip_newlines();
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.skip_newlines();
        self.expect(TokenKind::RBrace, "`}` to close the map")?;
        Ok(entries)
    }

    fn string_pieces(&mut self, parts: Vec<StrPart>) -> Result<Vec<StrPiece>, Diagnostic> {
        let mut pieces = Vec::with_capacity(parts.len());
        for part in parts {
            pieces.push(match part {
                StrPart::Lit(s) => StrPiece::Lit(s),
                StrPart::Interp { src, span } => StrPiece::Expr(parse_expr_src(&src, span)?),
            });
        }
        Ok(pieces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Program {
        match parse(src) {
            Ok(p) => p,
            Err(e) => panic!("{}", e.render(None, Some(src))),
        }
    }

    fn err(src: &str) -> Diagnostic {
        parse(src).expect_err("expected a parse error")
    }

    #[test]
    fn parses_declarations_and_functions() {
        let p = ok(
            "roll a = 1\nstick b = a + 2\npull add(x, y) {\n  snuff x + y\n}\nexhale add(a, b)\n",
        );
        assert_eq!(p.body.len(), 4);
        assert!(matches!(p.body[0], Stmt::Declare { mutable: true, .. }));
        assert!(matches!(p.body[1], Stmt::Declare { mutable: false, .. }));
        assert!(matches!(p.body[2], Stmt::Pull(_)));
    }

    #[test]
    fn precedence_is_sane() {
        let p = ok("roll v = 1 + 2 * 3 == 7 and not false");
        let Stmt::Declare { value, .. } = &p.body[0] else {
            panic!()
        };
        // and( ==( +(1, *(2,3)), 7 ), not(false) )
        let ExprKind::Binary {
            op: BinOp::And,
            lhs,
            ..
        } = &value.kind
        else {
            panic!("top should be `and`, got {:?}", value.kind)
        };
        let ExprKind::Binary {
            op: BinOp::Eq,
            lhs: sum,
            ..
        } = &lhs.kind
        else {
            panic!("expected `==`")
        };
        assert!(matches!(sum.kind, ExprKind::Binary { op: BinOp::Add, .. }));
    }

    #[test]
    fn pipe_is_lowest() {
        let p = ok("roll v = a + 1 >> f(2) >> g");
        let Stmt::Declare { value, .. } = &p.body[0] else {
            panic!()
        };
        let ExprKind::Pipe { rhs, lhs } = &value.kind else {
            panic!()
        };
        assert!(matches!(rhs.kind, ExprKind::Ident(_)));
        assert!(matches!(lhs.kind, ExprKind::Pipe { .. }));
    }

    #[test]
    fn statements_need_terminators() {
        let e = err("roll a = 1 roll b = 2");
        assert!(e.message.contains("end of statement"));
        ok("roll a = 1; roll b = 2");
    }

    #[test]
    fn multiline_calls_lists_and_maps() {
        ok("roll m = {\n  name: \"x\",\n  \"with space\": [1,\n 2,\n 3],\n  [1 + 1]: null,\n}\nroll r = f(\n  1,\n  2,\n)\n");
    }

    #[test]
    fn if_else_chains_and_else_on_next_line() {
        let p = ok("if a {\n exhale 1\n} else if b {\n exhale 2\n}\nelse {\n exhale 3\n}\n");
        let Stmt::If {
            branches,
            otherwise,
            ..
        } = &p.body[0]
        else {
            panic!()
        };
        assert_eq!(branches.len(), 2);
        assert!(otherwise.is_some());
    }

    #[test]
    fn burn_try_and_lambdas() {
        let p = ok("burn unlit {\n fs.rm(\"x\")\n}\ntry {\n cough \"boom\"\n} ashtray e {\n exhale e.message\n}\nroll inc = pack(x) => x + 1\nroll blk = pack(x) { snuff x }\n");
        assert!(matches!(
            p.body[0],
            Stmt::Burn {
                class: BurnClass::Unlit,
                ..
            }
        ));
        assert!(matches!(&p.body[1], Stmt::Try { catch_var: Some(v), .. } if v == "e"));
    }

    #[test]
    fn assignment_targets() {
        ok("a = 1\na += 2\nm.k = 3\nl[0] = 4\n");
        let e = err("f() = 1");
        assert!(e.message.contains("assigned"));
    }

    #[test]
    fn helpful_errors() {
        assert!(err("roll = 1").message.contains("expected a name"));
        assert!(err("if x {\n exhale 1\n").message.contains("never closed"));
        assert!(err("a < b < c").hint.is_some());
        assert!(err("else {}").message.contains("`else`"));
    }

    #[test]
    fn interpolation_parses_expressions() {
        let p = ok("exhale \"n=${n + 1} ok\"");
        let Stmt::Exhale { values, .. } = &p.body[0] else {
            panic!()
        };
        let ExprKind::Str(pieces) = &values[0].kind else {
            panic!()
        };
        assert!(matches!(pieces[1], StrPiece::Expr(_)));
    }
}
