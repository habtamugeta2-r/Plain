//! The Plain parser: tokens in, AST out, diagnostics alongside.
//!
//! Plain is indentation-based, so the interesting decisions are not about
//! operator precedence — that part is textbook — but about *what a bare word on
//! a line means*. The same word can be an element, a style property, an HTML
//! attribute, a modifier, a theme token or a value, depending on where it sits.
//! The rules are fixed and ordered, and every one of them is documented at the
//! place it is applied:
//!
//! 1. On an element line and inside an element block, constructs come first:
//!    `when`, `on`, `show`, `bind`, `@breakpoint`, `children`, `slot`.
//! 2. Then control flow and nested UI: `if`, `each`, `let`, `derive`, `fn`,
//!    element names, and Capitalised component names.
//! 3. Then decoration: modifiers (no value), attributes (a value), style
//!    properties (a value).
//! 4. Only then is a bare word an error.
//!
//! Inside a *value list* a word ends the list when it can only start a new
//! item. [`tables::hard_item`] names always end a list; [`tables::soft_item`]
//! names end one only after the property already has a value. That single
//! distinction is what makes both of these read correctly:
//!
//! ```text
//! text-align left            -- one property, one value
//! position absolute left 0   -- three properties
//! ```
//!
//! The parser never panics and never loops forever on malformed input: every
//! loop either consumes a token or reports an error and consumes one. That
//! matters because `plain dev` and the language server keep this parser running
//! while the file is still being typed.

use crate::ast::{
    Assign, AttrProp, BindTarget, Component, DeferredUse, Derive, Each, Element, Expr, FnDecl,
    Handler, If, Let, Modifier, Node, Param, Path, PathStep, Persist, Program, PropDecl,
    PseudoBlock, ResponsiveBlock, Return, Slot, StrPart, StyleProp, Theme, ThemeEntry, ThemeGroup,
    TypeRef, Use,
};
use crate::lexer::{lex, Token, TokenKind};
use crate::tables;
use plain_core::{Diagnostic, FileId, Span};

/// Parses a token stream into a [`Program`].
///
/// Diagnostics from the lexer are *not* included; call [`parse_source`] to get
/// both, or merge them yourself when you already lexed the file.
pub fn parse(file: FileId, tokens: Vec<Token>) -> (Program, Vec<Diagnostic>) {
    let mut parser = Parser::new(file, tokens);
    let program = parser.parse_program();
    (program, parser.errors)
}

/// Lexes and parses `source` in one step, returning every diagnostic found.
pub fn parse_source(file: FileId, source: &str) -> (Program, Vec<Diagnostic>) {
    let (tokens, mut diagnostics) = lex(file, source);
    let (program, parse_errors) = parse(file, tokens);
    diagnostics.extend(parse_errors);
    (program, diagnostics)
}

/// What a block may contain. The parser needs to know this to give the right
/// error for `text "hi"` inside a click handler, or `padding 1rem` inside `if`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// UI and statements: the file body, an element's children, `if`, `each`.
    Nodes,
    /// Statements only: a handler body or a function body.
    Statements,
}

/// Whether the items being read sit on the element's own line or in the
/// indented block below it. Only a block may hold children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Inline,
    Block,
}

/// Everything that can decorate an element or a component invocation.
///
/// One struct for both keeps a single parsing routine; the AST then splits the
/// fields back out, because [`Element`] and [`Use`] are genuinely different
/// things to the semantic pass and to code generation.
#[derive(Debug, Clone, Default)]
struct Decorations {
    props: Vec<StyleProp>,
    attrs: Vec<AttrProp>,
    modifiers: Vec<Modifier>,
    pseudos: Vec<PseudoBlock>,
    responsive: Vec<ResponsiveBlock>,
    handlers: Vec<Handler>,
    bind: Option<BindTarget>,
    show: Option<Expr>,
    children: Vec<Node>,
    slots: Vec<(String, Vec<Node>)>,
    positional: Vec<Expr>,
    /// Named props of a component invocation written in block form.
    props_named: Vec<(Span, String, Expr)>,
}

/// What a style scope — the inside of a `when` or `@` block — collected.
#[derive(Debug, Clone, Default)]
struct StyleScope {
    props: Vec<StyleProp>,
    pseudos: Vec<PseudoBlock>,
    responsive: Vec<ResponsiveBlock>,
    modifiers: Vec<Modifier>,
    show: Option<Expr>,
}

struct Parser {
    file: FileId,
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
    deferred: Vec<DeferredUse>,
}

impl Parser {
    fn new(file: FileId, tokens: Vec<Token>) -> Self {
        Parser { file, tokens, pos: 0, errors: Vec::new(), deferred: Vec::new() }
    }

    // -- token access ------------------------------------------------------

    /// The token `offset` positions ahead, clamped to the final `Eof`.
    fn at(&self, offset: usize) -> &Token {
        let index = self.pos + offset;
        if index < self.tokens.len() {
            &self.tokens[index]
        } else {
            self.tokens.last().expect("the lexer always ends with Eof")
        }
    }

    fn peek(&self) -> &Token {
        self.at(0)
    }

    fn kind(&self) -> TokenKind {
        self.peek().kind
    }

    fn span(&self) -> Span {
        self.peek().span
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn is_name(&self, text: &str) -> bool {
        self.kind() == TokenKind::Name && self.peek().text == text
    }

    fn name_at(&self, offset: usize, text: &str) -> bool {
        self.at(offset).kind == TokenKind::Name && self.at(offset).text == text
    }

    /// Consumes the current token when it matches, reporting otherwise.
    fn expect(&mut self, kind: TokenKind, what: &str) -> Option<Token> {
        if self.kind() == kind {
            return Some(self.advance());
        }
        let found = self.peek().clone();
        self.error("E101", format!("expected {what}, found {}", found.describe()), found.span);
        None
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.kind() == kind {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn at_line_end(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
        )
    }

    fn skip_newlines(&mut self) {
        while self.kind() == TokenKind::Newline {
            self.pos += 1;
        }
    }

    /// Discards the rest of the current line.
    fn skip_line(&mut self) {
        while !self.at_line_end() {
            self.pos += 1;
        }
    }

    /// Discards an indented block, including its `Indent` and `Dedent`.
    fn skip_block(&mut self) {
        if self.kind() != TokenKind::Indent {
            return;
        }
        self.pos += 1;
        let mut depth = 1usize;
        while depth > 0 && self.kind() != TokenKind::Eof {
            match self.kind() {
                TokenKind::Indent => depth += 1,
                TokenKind::Dedent => depth -= 1,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// True when the current line opens with UI, which a statement body cannot
    /// contain.
    fn line_starts_ui(&self) -> bool {
        if self.kind() != TokenKind::Name {
            return false;
        }
        let name = self.peek().text.as_str();
        tables::element(name).is_some() || is_component_name(name)
    }

    /// True when the current line opens with something that only decorates an
    /// element, so it cannot stand on its own.
    fn line_starts_style_only(&self) -> bool {
        if self.kind() != TokenKind::Name {
            return false;
        }
        let name = self.peek().text.as_str();
        if tables::element(name).is_some()
            || is_component_name(name)
            || is_statement_keyword(name)
        {
            return false;
        }
        if self.assignment_ahead() {
            return false;
        }
        if matches!(name, "when" | "on" | "show" | "bind") {
            return true;
        }
        tables::MODIFIERS.contains(&name) || tables::is_property(name)
    }

    /// True when a value can follow the current token on this line.
    ///
    /// Only a *hard* item keyword rules a value out. `text-align left` has a
    /// value even though `left` is itself a property name.
    fn value_follows(&self) -> bool {
        match self.at(1).kind {
            TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof => false,
            TokenKind::Name => !tables::hard_item(&self.at(1).text),
            _ => true,
        }
    }

    // -- diagnostics -------------------------------------------------------

    fn error(&mut self, code: &str, title: impl Into<String>, span: Span) -> &mut Self {
        self.errors
            .push(Diagnostic::error(code, title, self.file).with_label(span, String::new()));
        self
    }

    /// Adds an explanatory paragraph to the diagnostic just pushed.
    fn note(&mut self, text: impl Into<String>) -> &mut Self {
        if let Some(last) = self.errors.last_mut() {
            last.notes.push(text.into());
        }
        self
    }

    /// Adds a concrete fix to the diagnostic just pushed.
    fn suggest(&mut self, text: impl Into<String>) -> &mut Self {
        if let Some(last) = self.errors.last_mut() {
            last.suggestion = Some(text.into());
        }
        self
    }

    /// Reports a construct that is specified but not implemented in v0.1.
    ///
    /// Saying "not yet, here is the plan" is friendlier — and more honest —
    /// than a syntax error that implies the writer made a mistake.
    fn deferred_node(&mut self, keyword: &str, span: Span) -> Node {
        let message = tables::deferred(keyword).unwrap_or("this lands in a later version");
        let found = DeferredUse { span, keyword: keyword.to_string(), message: message.to_string() };
        self.deferred.push(found.clone());
        self.error("E900", format!("`{keyword}` is not implemented in Plain v0.1"), span)
            .note(message.to_string())
            .suggest("See docs/11-roadmap.md for what v0.1 covers.");
        Node::Deferred(found)
    }

    // -- program -----------------------------------------------------------

    fn parse_program(&mut self) -> Program {
        let mut program = Program::new(self.file);
        loop {
            self.skip_newlines();
            match self.kind() {
                TokenKind::Eof => break,
                TokenKind::Indent => {
                    let span = self.span();
                    self.error("E102", "unexpected indentation at the top level", span)
                        .note("Plain uses indentation to nest things inside each other. Nothing is nested here.")
                        .suggest("Remove the leading spaces.");
                    self.pos += 1;
                    self.skip_block();
                }
                TokenKind::Dedent => {
                    self.pos += 1;
                }
                TokenKind::Name if self.is_name("theme") => {
                    if let Some(theme) = self.parse_theme() {
                        if program.theme.is_some() {
                            let span = theme.span;
                            self.error("E112", "a file may declare only one `theme`", span)
                                .suggest("Merge the two blocks into one.");
                        }
                        program.theme = Some(theme);
                    }
                }
                TokenKind::Name if self.is_name("component") => {
                    if let Some(component) = self.parse_component() {
                        program.components.push(component);
                    }
                }
                _ => {
                    if self.line_starts_style_only() {
                        let name = self.peek().text.clone();
                        let span = self.span();
                        self.error(
                            "E179",
                            format!("`{name}` is a style property, which needs an element to style"),
                            span,
                        )
                        .note("Properties live on the element line or in the element's indented block.")
                        .suggest(format!("box {name} ..."));
                        self.skip_line();
                        self.skip_block();
                        continue;
                    }
                    let before = self.pos;
                    if let Some(node) = self.parse_item(BlockKind::Nodes) {
                        program.body.push(node);
                    }
                    if self.pos == before {
                        self.pos += 1;
                    }
                }
            }
        }
        program.deferred = std::mem::take(&mut self.deferred);
        program
    }

    /// Parses nodes until the enclosing block ends.
    fn parse_items(&mut self, kind: BlockKind) -> Vec<Node> {
        let mut nodes = Vec::new();
        loop {
            self.skip_newlines();
            match self.kind() {
                TokenKind::Dedent | TokenKind::Eof => break,
                TokenKind::Indent => {
                    let span = self.span();
                    self.error("E102", "unexpected indentation", span)
                        .note("Only `if`, `each`, `on`, `when`, `@breakpoint`, `component`, `fn` and elements open indented blocks.");
                    self.pos += 1;
                    self.skip_block();
                }
                _ => {
                    let before = self.pos;
                    match kind {
                        BlockKind::Statements if self.line_starts_ui() => {
                            let name = self.peek().text.clone();
                            let span = self.span();
                            self.error(
                                "E183",
                                format!("`{name}` is UI, which cannot appear inside a handler or function body"),
                                span,
                            )
                            .note("Bodies hold statements: assignments, calls, `if` and `each`.")
                            .suggest("Render the element outside the body and drive it with state or `show`.");
                            self.skip_line();
                            self.skip_block();
                        }
                        BlockKind::Nodes if self.line_starts_style_only() => {
                            let name = self.peek().text.clone();
                            let span = self.span();
                            self.error(
                                "E178",
                                format!("`{name}` styles an element, but there is no element here"),
                                span,
                            )
                            .note("Inside `if` and `each` you render elements; the styling goes on them.");
                            self.skip_line();
                            self.skip_block();
                        }
                        _ => {
                            if let Some(node) = self.parse_item(kind) {
                                nodes.push(node);
                            }
                        }
                    }
                    if self.pos == before {
                        self.pos += 1;
                    }
                }
            }
        }
        nodes
    }

    /// Parses one node. `kind` decides whether UI is allowed here.
    fn parse_item(&mut self, kind: BlockKind) -> Option<Node> {
        if self.kind() != TokenKind::Name {
            return self.parse_expression_statement();
        }
        let name = self.peek().text.clone();
        let span = self.span();

        // A type alias looks like `type Name = ...`. `type` is also an HTML
        // attribute, so this shape is checked before anything else.
        if name == "type" && self.at(1).kind == TokenKind::Name && self.at(2).kind == TokenKind::Eq {
            self.skip_line();
            return Some(self.deferred_node("type", span));
        }

        match name.as_str() {
            "let" => self.parse_let(),
            "derive" => self.parse_derive(),
            "fn" => self.parse_fn(),
            "return" => self.parse_return(),
            "if" if !self.conditional_expression_ahead() => self.parse_if(kind),
            "each" => self.parse_each(kind),
            "children" => {
                self.pos += 1;
                self.finish_line("`children`");
                Some(Node::Slot(Slot { span, name: None }))
            }
            "slot" => self.parse_slot(),
            "persist" => {
                self.error(
                    "E187",
                    "`persist` is written at the end of a `let`",
                    span,
                )
                .suggest("let count = 0 persist \"counter\"");
                self.skip_line();
                None
            }
            "theme" => {
                self.error("E113", "`theme` must be at the top level of a file", span);
                self.skip_line();
                self.skip_block();
                None
            }
            "component" => {
                self.error("E110", "`component` must be at the top level of a file", span);
                self.skip_line();
                self.skip_block();
                None
            }
            _ => {
                if tables::deferred(&name).is_some() {
                    self.skip_line();
                    self.skip_block();
                    return Some(self.deferred_node(&name, span));
                }
                if tables::element(&name).is_some() {
                    return self.parse_element_line();
                }
                if is_component_name(&name) {
                    return self.parse_use_line();
                }
                if kind == BlockKind::Nodes && tables::pseudo(&name).is_some() {
                    self.error(
                        "E144",
                        format!("`{name}` is a pseudo-state, which needs `when`"),
                        span,
                    )
                    .suggest(format!("when {name}"));
                    self.skip_line();
                    self.skip_block();
                    return None;
                }
                self.parse_expression_statement()
            }
        }
    }

    // -- declarations ------------------------------------------------------

    fn parse_let(&mut self) -> Option<Node> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a name after `let`") {
            Some(token) => token,
            None => {
                self.skip_line();
                return None;
            }
        };
        self.check_identifier(&name_token);
        let ty = self.parse_optional_type();
        if self.expect(TokenKind::Eq, "`=` and an initial value").is_none() {
            self.skip_line();
            return None;
        }
        let value = match self.parse_expr() {
            Some(value) => value,
            None => {
                self.skip_line();
                return None;
            }
        };
        let persist = self.parse_persist_suffix();
        let span = keyword.span.merge(persist.as_ref().map_or(value.span(), |p| p.span));
        self.finish_line(&format!("`let {}`", name_token.text));
        Some(Node::Let(Let {
            span,
            name: name_token.text,
            ty,
            value,
            persist,
        }))
    }

    fn parse_derive(&mut self) -> Option<Node> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a name after `derive`") {
            Some(token) => token,
            None => {
                self.skip_line();
                return None;
            }
        };
        self.check_identifier(&name_token);
        let ty = self.parse_optional_type();
        if self.expect(TokenKind::Eq, "`=` and the expression to derive").is_none() {
            self.skip_line();
            return None;
        }
        let value = match self.parse_expr() {
            Some(value) => value,
            None => {
                self.skip_line();
                return None;
            }
        };
        let span = keyword.span.merge(value.span());
        self.finish_line(&format!("`derive {}`", name_token.text));
        Some(Node::Derive(Derive { span, name: name_token.text, ty, value }))
    }

    /// `persist "counter"` or `persist session "counter"` after a `let` value.
    fn parse_persist_suffix(&mut self) -> Option<Persist> {
        if !self.is_name("persist") {
            return None;
        }
        let keyword = self.advance();
        let session = self.is_name("session");
        if session {
            self.pos += 1;
        }
        let key = match self.parse_expr() {
            Some(key) => key,
            None => {
                self.error("E188", "`persist` needs a storage key", keyword.span)
                    .suggest("let count = 0 persist \"counter\"");
                return None;
            }
        };
        Some(Persist { span: keyword.span.merge(key.span()), key, session })
    }

    fn parse_fn(&mut self) -> Option<Node> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a name after `fn`") {
            Some(token) => token,
            None => {
                self.skip_line();
                return None;
            }
        };
        self.check_identifier(&name_token);
        let mut params = Vec::new();
        if self.eat(TokenKind::LParen) {
            while self.kind() != TokenKind::RParen && self.kind() != TokenKind::Eof {
                let before = self.pos;
                if let Some(param) = self.parse_param() {
                    params.push(param);
                }
                if self.pos == before {
                    self.pos += 1;
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "`)`");
        }
        self.finish_line(&format!("`fn {}`", name_token.text));
        let body = self.parse_indented_block(BlockKind::Statements, &format!("fn {}", name_token.text), "E181");
        let span = keyword.span.merge(name_token.span);
        Some(Node::Fn(FnDecl { span, name: name_token.text, params, body }))
    }

    fn parse_param(&mut self) -> Option<Param> {
        let name_token = match self.expect(TokenKind::Name, "a parameter name") {
            Some(token) => token,
            None => return None,
        };
        self.check_identifier(&name_token);
        let ty = self.parse_optional_type();
        let default = if self.eat(TokenKind::Eq) { self.parse_expr() } else { None };
        let span = default.as_ref().map_or(name_token.span, |d| name_token.span.merge(d.span()));
        Some(Param { span, name: name_token.text, ty, default })
    }

    fn parse_return(&mut self) -> Option<Node> {
        let keyword = self.advance();
        let value = if self.at_line_end() { None } else { self.parse_expr() };
        let span = value.as_ref().map_or(keyword.span, |v| keyword.span.merge(v.span()));
        self.finish_line("`return`");
        Some(Node::Return(Return { span, value }))
    }

    fn parse_slot(&mut self) -> Option<Node> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a slot name after `slot`") {
            Some(token) => token,
            None => {
                self.skip_line();
                return None;
            }
        };
        self.finish_line(&format!("`slot {}`", name_token.text));
        Some(Node::Slot(Slot { span: keyword.span.merge(name_token.span), name: Some(name_token.text) }))
    }

    /// True when an `if` on this line is the ternary-like expression form,
    /// `if a then b else c`, rather than a block.
    fn conditional_expression_ahead(&self) -> bool {
        let mut offset = 1usize;
        while !matches!(
            self.at(offset).kind,
            TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
        ) {
            if self.name_at(offset, "then") {
                return true;
            }
            offset += 1;
        }
        false
    }

    fn parse_if(&mut self, kind: BlockKind) -> Option<Node> {
        let keyword = self.advance();
        let condition = match self.parse_expr() {
            Some(condition) => condition,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let body = self.parse_indented_block(kind, "if", "E182");
        let mut branches = vec![(condition, body)];
        let mut else_branch = None;
        loop {
            self.skip_newlines();
            if !self.is_name("else") {
                break;
            }
            self.pos += 1;
            if self.is_name("if") {
                self.pos += 1;
                let condition = match self.parse_expr() {
                    Some(condition) => condition,
                    None => {
                        self.skip_line();
                        self.skip_block();
                        break;
                    }
                };
                let body = self.parse_indented_block(kind, "else if", "E182");
                branches.push((condition, body));
                continue;
            }
            self.finish_line("`else`");
            else_branch = Some(self.parse_indented_block(kind, "else", "E182"));
            break;
        }
        let span = branches
            .iter()
            .fold(keyword.span, |acc, (condition, _)| acc.merge(condition.span()));
        Some(Node::If(If { span, branches, else_branch }))
    }

    fn parse_each(&mut self, kind: BlockKind) -> Option<Node> {
        let keyword = self.advance();
        let item_token = match self.expect(TokenKind::Name, "an item name after `each`") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        self.check_identifier(&item_token);
        let mut index = None;
        if self.eat(TokenKind::Comma) {
            match self.expect(TokenKind::Name, "an index name") {
                Some(token) => {
                    self.check_identifier(&token);
                    index = Some(token.text);
                }
                None => {
                    self.skip_line();
                    self.skip_block();
                    return None;
                }
            }
        }
        if !self.is_name("in") {
            let found = self.peek().clone();
            self.error(
                "E184",
                format!("expected `in` after the item name, found {}", found.describe()),
                found.span,
            )
            .suggest("each todo in todos");
            self.skip_line();
            self.skip_block();
            return None;
        }
        self.pos += 1;
        let iter = match self.parse_expr() {
            Some(iter) => iter,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let mut key = None;
        if self.is_name("key") {
            self.pos += 1;
            key = self.parse_expr();
            if key.is_none() {
                let span = self.span();
                self.error("E186", "`key` needs an expression", span)
                    .suggest("each todo in todos key todo.id");
                self.skip_line();
            }
        }
        let body = self.parse_indented_block(kind, "each", "E180");
        let span = keyword.span.merge(item_token.span);
        Some(Node::Each(Each { span, item: item_token.text, index, iter, key, body }))
    }

    /// Consumes the newline and the indented block that must follow `opener`.
    fn parse_indented_block(&mut self, kind: BlockKind, opener: &str, code: &str) -> Vec<Node> {
        self.finish_line(&format!("`{opener}`"));
        self.skip_newlines();
        if self.kind() != TokenKind::Indent {
            let found = self.peek().clone();
            self.error(
                code,
                format!("`{opener}` needs an indented block underneath it"),
                found.span,
            )
            .note("Plain reads structure from indentation, so the block is required even for one line.")
            .suggest(format!("{opener} ...\n  ..."));
            self.skip_block();
            return Vec::new();
        }
        self.pos += 1;
        let nodes = self.parse_items(kind);
        self.eat(TokenKind::Dedent);
        nodes
    }

    fn parse_expression_statement(&mut self) -> Option<Node> {
        let before = self.pos;
        if self.assignment_ahead() {
            let target = self.parse_path();
            self.pos += 1; // the `=` checked by assignment_ahead
            if let Some(value) = self.parse_expr() {
                let span = target.span.merge(value.span());
                self.finish_line("the assigned value");
                return Some(Node::Assign(Assign { span, target, value }));
            }
            self.skip_line();
            return None;
        }
        let expr = match self.parse_expr() {
            Some(expr) => expr,
            None => {
                if self.pos == before {
                    let found = self.peek().clone();
                    self.error(
                        "E170",
                        format!("expected a value or a statement, found {}", found.describe()),
                        found.span,
                    );
                    self.pos += 1;
                }
                self.skip_line();
                return None;
            }
        };
        self.finish_line("the expression");
        Some(Node::Expr(expr))
    }

    /// True when the current line is an assignment, `a.b[0] = value`.
    ///
    /// Without this lookahead, `color = theme` would parse as a style property
    /// called `color` followed by nonsense.
    fn assignment_ahead(&self) -> bool {
        let mut offset = 0usize;
        if self.at(offset).kind != TokenKind::Name {
            return false;
        }
        offset += 1;
        loop {
            match self.at(offset).kind {
                TokenKind::Dot => {
                    offset += 1;
                    if self.at(offset).kind != TokenKind::Name {
                        return false;
                    }
                    offset += 1;
                }
                TokenKind::LBracket => {
                    let mut depth = 0usize;
                    loop {
                        match self.at(offset).kind {
                            TokenKind::LBracket => depth += 1,
                            TokenKind::RBracket => {
                                depth -= 1;
                                offset += 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            TokenKind::Eof => return false,
                            _ => {}
                        }
                        offset += 1;
                    }
                }
                _ => break,
            }
        }
        self.at(offset).kind == TokenKind::Eq
    }

    fn parse_path(&mut self) -> Path {
        let root_token = self.advance();
        let mut steps = Vec::new();
        loop {
            match self.kind() {
                TokenKind::Dot => {
                    self.pos += 1;
                    match self.expect(TokenKind::Name, "a field name after `.`") {
                        Some(field) => steps.push(PathStep::Field(field.text, field.span)),
                        None => break,
                    }
                }
                TokenKind::LBracket => {
                    let open = self.span();
                    self.pos += 1;
                    match self.parse_expr() {
                        Some(index) => {
                            let close = self.expect(TokenKind::RBracket, "`]`").map_or(index.span(), |t| t.span);
                            steps.push(PathStep::Index(index, open.merge(close)));
                        }
                        None => break,
                    }
                }
                _ => break,
            }
        }
        let span = steps.iter().fold(root_token.span, |acc, step| {
            acc.merge(match step {
                PathStep::Field(_, span) | PathStep::Index(_, span) => *span,
            })
        });
        Path { span, root: root_token.text, steps }
    }

    /// Reports identifiers that cannot work: reserved words, and names starting
    /// with a digit or containing a run of hyphens that reads as subtraction.
    fn check_identifier(&mut self, token: &Token) {
        let name = token.text.as_str();
        if tables::RESERVED.contains(&name) {
            self.error("E190", format!("`{name}` is a reserved word"), token.span)
                .suggest(format!("Pick another name, such as `{name}_value`."));
        }
    }

    /// Reports anything left over at the end of a line.
    fn finish_line(&mut self, context: &str) {
        if self.at_line_end() {
            return;
        }
        let found = self.peek().clone();
        self.error(
            "E104",
            format!("unexpected {} after {}", found.describe(), context),
            found.span,
        )
        .note("A newline ends the item; nothing may follow it on the same line.");
        self.skip_line();
    }

    // -- elements ----------------------------------------------------------

    fn parse_element_line(&mut self) -> Option<Node> {
        let token = self.advance();
        let def = match tables::element(&token.text) {
            Some(def) => def,
            None => {
                self.error("E120", format!("`{}` is not a Plain element", token.text), token.span);
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let name = token.text.clone();
        let mut deco = Decorations::default();
        let mut args = Vec::new();
        let mut span = token.span;
        self.parse_ui_items(&name, false, UiMode::Inline, &mut deco, &mut args, &mut span);

        let mut children = std::mem::take(&mut deco.children);
        if self.kind() == TokenKind::Newline {
            self.pos += 1;
        }
        if self.kind() == TokenKind::Indent {
            self.pos += 1;
            let mut block = Decorations::default();
            let mut block_args = Vec::new();
            self.parse_ui_items(&name, false, UiMode::Block, &mut block, &mut block_args, &mut span);
            self.eat(TokenKind::Dedent);
            args.extend(block_args);
            children.append(&mut block.children);
            merge(&mut deco, block);
        }

        Some(Node::Element(Element {
            span,
            name,
            def,
            args,
            props: deco.props,
            attrs: deco.attrs,
            modifiers: deco.modifiers,
            pseudos: deco.pseudos,
            responsive: deco.responsive,
            handlers: deco.handlers,
            bind: deco.bind,
            show: deco.show,
            children,
            class: None,
        }))
    }

    fn parse_use_line(&mut self) -> Option<Node> {
        let token = self.advance();
        let name = token.text.clone();
        let mut used = Use {
            span: token.span,
            name: name.clone(),
            props: Vec::new(),
            positional: Vec::new(),
            props_style: Vec::new(),
            pseudos: Vec::new(),
            responsive: Vec::new(),
            attrs: Vec::new(),
            modifiers: Vec::new(),
            handlers: Vec::new(),
            show: None,
            children: Vec::new(),
            slots: Vec::new(),
            class: None,
        };

        // Parenthesised props are the unambiguous form and the one the
        // documentation leads with: `Card(title: "Hi", count: 3)`.
        if self.kind() == TokenKind::LParen {
            self.pos += 1;
            self.parse_paren_props(&mut used);
            self.expect(TokenKind::RParen, "`)`");
        } else {
            let mut deco = Decorations::default();
            let mut args = Vec::new();
            self.parse_ui_items(&name, true, UiMode::Inline, &mut deco, &mut args, &mut used.span);
            used.positional.extend(args);
            used.props.append(&mut deco.props_named);
            apply_use_decorations(&mut used, deco);
        }

        if self.kind() == TokenKind::Newline {
            self.pos += 1;
        }
        if self.kind() == TokenKind::Indent {
            self.pos += 1;
            let mut deco = Decorations::default();
            let mut args = Vec::new();
            self.parse_ui_items(&name, true, UiMode::Block, &mut deco, &mut args, &mut used.span);
            self.eat(TokenKind::Dedent);
            used.positional.extend(args);
            used.props.append(&mut deco.props_named);
            used.children.append(&mut deco.children);
            used.slots.append(&mut deco.slots);
            used.positional.append(&mut deco.positional);
            apply_use_decorations(&mut used, deco);
        }
        Some(Node::Use(used))
    }

    /// `title: "Hi", count: 3` inside the parentheses of a component call.
    fn parse_paren_props(&mut self, used: &mut Use) {
        if self.kind() == TokenKind::RParen {
            return;
        }
        loop {
            let before = self.pos;
            if self.kind() == TokenKind::Name && self.at(1).kind == TokenKind::Colon {
                let name_token = self.advance();
                self.pos += 1; // the colon
                match self.parse_expr() {
                    Some(value) => {
                        used.span = used.span.merge(value.span());
                        used.props.push((name_token.span, name_token.text, value));
                    }
                    None => self.skip_to_comma_or_close(),
                }
            } else if let Some(value) = self.parse_expr() {
                // A positional prop: `Card("Hi")` fills the first declared prop.
                used.span = used.span.merge(value.span());
                used.positional.push(value);
            } else if self.pos == before {
                self.pos += 1;
            }
            if !self.eat(TokenKind::Comma) {
                break;
            }
            if self.kind() == TokenKind::RParen {
                break;
            }
        }
    }

    fn skip_to_comma_or_close(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.kind() {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Comma if depth == 0 => break,
                TokenKind::Eof | TokenKind::Newline | TokenKind::Dedent => break,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Reads the items that decorate an element or a component invocation.
    ///
    /// This is the heart of the parser: the ordered dispatch described in the
    /// module documentation. `args` collects positional arguments, which only
    /// elements have meaning for; `span` grows to cover everything read.
    fn parse_ui_items(
        &mut self,
        host: &str,
        component: bool,
        mode: UiMode,
        deco: &mut Decorations,
        args: &mut Vec<Expr>,
        span: &mut Span,
    ) {
        loop {
            let stop = match mode {
                UiMode::Inline => self.at_line_end(),
                UiMode::Block => matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof),
            };
            if stop {
                break;
            }
            if mode == UiMode::Block {
                self.skip_newlines();
                if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                    break;
                }
                if self.kind() == TokenKind::Indent {
                    let found = self.span();
                    self.error("E102", "unexpected indentation", found);
                    self.pos += 1;
                    self.skip_block();
                    continue;
                }
            }

            let before = self.pos;
            match self.kind() {
                TokenKind::Name => {
                    let name = self.peek().text.clone();
                    let name_span = self.span();
                    match name.as_str() {
                        "on" => {
                            if let Some(handler) = self.parse_handler() {
                                *span = span.merge(handler.span);
                                deco.handlers.push(handler);
                            }
                        }
                        "show" => {
                            self.pos += 1;
                            match self.parse_expr() {
                                Some(condition) => {
                                    *span = span.merge(condition.span());
                                    if deco.show.is_some() {
                                        self.error("E140", "an element can only have one `show`", name_span)
                                            .suggest("Combine the conditions: `show a and b`.");
                                    }
                                    deco.show = Some(condition);
                                }
                                None => {
                                    self.error("E141", "`show` needs a condition", name_span)
                                        .suggest("show open");
                                    self.skip_line();
                                }
                            }
                        }
                        "bind" => self.parse_bind(host, component, name_span, deco, span),
                        "children" => {
                            self.pos += 1;
                            self.finish_line("`children`");
                            if component {
                                self.error(
                                    "E162",
                                    "`children` inside a component invocation is redundant",
                                    name_span,
                                )
                                .note("Everything indented under a component invocation is already its children.");
                            }
                        }
                        "slot" if component => {
                            if let Some((slot_name, nodes)) = self.parse_slot_fill() {
                                *span = span.merge(name_span);
                                if deco.slots.iter().any(|(existing, _)| *existing == slot_name) {
                                    self.error(
                                        "E159",
                                        format!("the slot `{slot_name}` is filled twice"),
                                        name_span,
                                    );
                                }
                                deco.slots.push((slot_name, nodes));
                            }
                        }
                        _ if tables::pseudo(&name).is_some() && !self.value_follows() => {
                            self.error(
                                "E144",
                                format!("`{name}` is a pseudo-state, which needs `when`"),
                                name_span,
                            )
                            .suggest(format!("when {name}"));
                            self.skip_line();
                            self.skip_block();
                        }
                        "when" => {
                            if let Some(pseudo) = self.parse_pseudo_block(mode) {
                                *span = span.merge(pseudo.span);
                                deco.pseudos.push(pseudo);
                            }
                        }
                        _ => {
                            self.parse_plain_item(
                                host, component, mode, deco, args, span, &name, name_span,
                            );
                        }
                    }
                }
                TokenKind::At => {
                    if let Some(block) = self.parse_responsive_block(mode) {
                        *span = span.merge(block.span);
                        deco.responsive.push(block);
                    }
                }
                _ => {
                    self.parse_value_item(component, mode, deco, args, span, host);
                }
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
    }

    /// A word that is not a construct: an element, a modifier, an attribute, a
    /// property, a prop, or an error.
    #[allow(clippy::too_many_arguments)]
    fn parse_plain_item(
        &mut self,
        host: &str,
        component: bool,
        mode: UiMode,
        deco: &mut Decorations,
        args: &mut Vec<Expr>,
        span: &mut Span,
        name: &str,
        name_span: Span,
    ) {
        // An assignment wins over every reading of the name, so `color = theme`
        // inside an element is a statement and not a style property.
        if self.assignment_ahead() {
            if let Some(node) = self.parse_item(BlockKind::Nodes) {
                *span = span.merge(node.span());
                deco.children.push(node);
            }
            return;
        }

        let is_element = tables::element(name).is_some();
        let is_use = is_component_name(name);
        let is_control = is_statement_keyword(name);

        if (is_element || is_use || is_control) && mode == UiMode::Inline {
            self.error(
                "E121",
                format!("`{name}` cannot be written on the same line as its parent"),
                name_span,
            )
            .note("Plain nests by indentation, not by writing elements side by side.")
            .suggest(format!("Put `{name}` on the next line, indented."));
            self.skip_line();
            return;
        }
        if is_element || is_use || is_control {
            if let Some(node) = self.parse_item(BlockKind::Nodes) {
                *span = span.merge(node.span());
                deco.children.push(node);
            }
            return;
        }
        if let Some(message) = tables::deferred(name) {
            self.skip_line();
            self.skip_block();
            self.error("E900", format!("`{name}` is not implemented in Plain v0.1"), name_span)
                .note(message.to_string());
            return;
        }

        // Modifiers never take a value, so they are recognised first — but only
        // when nothing follows them. `fill` alone means "take the remaining
        // space"; `fill primary` means "paint it with the primary colour".
        if tables::MODIFIERS.contains(&name) && !self.value_follows() {
            self.pos += 1;
            deco.modifiers.push(Modifier { span: name_span, name: name.to_string() });
            return;
        }
        if !component && tables::is_attribute(host, name) && self.value_follows() {
            let attr = self.parse_attr_prop();
            *span = span.merge(attr.span);
            deco.attrs.push(attr);
            return;
        }
        if !component && tables::is_attribute(host, name) {
            self.error(
                "E145",
                format!("the attribute `{name}` needs a value"),
                name_span,
            )
            .suggest(format!("{name} \"value\""));
            self.pos += 1;
            return;
        }
        if tables::is_property(name) && self.value_follows() {
            let prop = self.parse_style_prop();
            *span = span.merge(prop.span);
            deco.props.push(prop);
            return;
        }
        if tables::is_property(name) {
            self.error("E146", format!("the property `{name}` needs a value"), name_span)
                .suggest(format!("{name} ..."));
            self.pos += 1;
            return;
        }
        if component && self.at(1).kind == TokenKind::Colon {
            let name_token = self.advance();
            self.pos += 1; // the colon
            match self.parse_expr() {
                Some(value) => {
                    *span = span.merge(value.span());
                    deco.props_named.push((name_token.span, name_token.text, value));
                }
                None => self.skip_line(),
            }
            return;
        }
        if component {
            // Block-form prop: `title "Hello"`.
            let name_token = self.advance();
            let values = self.parse_values();
            if values.is_empty() {
                self.error(
                    "E185",
                    format!("the prop `{name}` has no value"),
                    name_span,
                )
                .suggest(format!("Give it a value, or pass it inline: `{host}({name}: ...)`."));
                return;
            }
            let value = join_values(values, name_span);
            *span = span.merge(value.span());
            deco.props_named.push((name_token.span, name_token.text, value));
            return;
        }

        // Nothing matched. Offer the closest thing Plain does know.
        self.pos += 1;
        if let Some(property) = tables::suggest_property(name) {
            self.error("E130", format!("`{name}` is not a property Plain knows", name_span))
                .suggest(format!("Did you mean `{property}`?"));
        } else if let Some(element) = closest_name(name, tables::ELEMENT_NAMES) {
            self.error("E120", format!("`{name}` is not a Plain element", name_span))
                .suggest(format!("Did you mean `{element}`?"));
        } else {
            self.error(
                "E172",
                format!("unexpected `{name}` inside `{host}`"),
                name_span,
            )
            .note("Plain expects an element, a style property, an attribute, a modifier, `on`, `when`, `show`, `bind` or `@breakpoint` here.");
        }
        self.skip_line();
        self.skip_block();
    }

    /// A token that is not a word: a positional argument, or an error.
    fn parse_value_item(
        &mut self,
        component: bool,
        mode: UiMode,
        deco: &mut Decorations,
        args: &mut Vec<Expr>,
        span: &mut Span,
        host: &str,
    ) {
        let found = self.peek().clone();
        if mode == UiMode::Block && !component {
            self.error(
                "E161",
                format!("a value cannot stand alone inside `{host}`"),
                found.span,
            )
            .note("Text lives in an element: `text \"...\"`. A bare string has nowhere to go.");
            self.skip_line();
            return;
        }
        match self.parse_expr() {
            Some(value) => {
                *span = span.merge(value.span());
                if component {
                    deco.positional.push(value);
                } else {
                    args.push(value);
                }
            }
            None => {
                self.error(
                    "E170",
                    format!("expected a value, found {}", found.describe()),
                    found.span,
                );
                self.pos += 1;
                self.skip_line();
            }
        }
    }

    fn parse_bind(
        &mut self,
        host: &str,
        component: bool,
        name_span: Span,
        deco: &mut Decorations,
        span: &mut Span,
    ) {
        self.pos += 1;
        if component {
            self.error("E142", "`bind` belongs on an input element, not a component", name_span)
                .suggest("Bind the `input` inside the component, or pass the value in as a prop.");
            self.skip_line();
            return;
        }
        if deco.bind.is_some() {
            self.error("E155", "an element can only have one `bind`", name_span);
            self.skip_line();
            return;
        }
        if !tables::BINDABLE.contains(&host) {
            self.error(
                "E142",
                format!("`{host}` cannot be bound"),
                name_span,
            )
            .note("`bind` works on input, check, radio, slider, select, toggle and textarea.")
            .suggest("Use `on input` and `value` instead.");
            self.skip_line();
            return;
        }
        if self.kind() != TokenKind::Name {
            let found = self.peek().clone();
            self.error("E141", format!("`bind` needs a name, found {}", found.describe()), found.span)
                .suggest("bind form.email");
            self.skip_line();
            return;
        }
        let path = self.parse_path();
        *span = span.merge(path.span);
        deco.bind = Some(BindTarget { span: path.span, path });
    }

    /// `slot header` followed by an indented block.
    fn parse_slot_fill(&mut self) -> Option<(String, Vec<Node>)> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a slot name after `slot`") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        self.finish_line(&format!("`slot {}`", name_token.text));
        self.skip_newlines();
        let nodes = if self.kind() == TokenKind::Indent {
            self.pos += 1;
            let nodes = self.parse_items(BlockKind::Nodes);
            self.eat(TokenKind::Dedent);
            nodes
        } else {
            self.error(
                "E158",
                format!("`slot {}` needs an indented block underneath it", name_token.text),
                keyword.span.merge(name_token.span),
            );
            Vec::new()
        };
        Some((name_token.text, nodes))
    }

    // -- styling -----------------------------------------------------------

    fn parse_style_prop(&mut self) -> Option<StyleProp> {
        let token = self.advance();
        let values = self.parse_values();
        if values.is_empty() {
            self.error(
                "E146",
                format!("the property `{}` needs a value", token.text),
                token.span,
            )
            .suggest(format!("{} ...", token.text));
        }
        let span = values.iter().fold(token.span, |acc, value| acc.merge(value.span()));
        Some(StyleProp::new(span, token.text, values))
    }

    fn parse_attr_prop(&mut self) -> AttrProp {
        let token = self.advance();
        let values = self.parse_values();
        if values.is_empty() {
            self.error(
                "E145",
                format!("the attribute `{}` needs a value", token.text),
                token.span,
            )
            .suggest(format!("{} \"value\"", token.text));
        }
        let span = values.iter().fold(token.span, |acc, value| acc.merge(value.span()));
        AttrProp::new(span, token.text, values)
    }

    /// Reads the values of one property, stopping where the next item begins.
    fn parse_values(&mut self) -> Vec<Expr> {
        let mut values = Vec::new();
        loop {
            if self.at_line_end() {
                break;
            }
            match self.kind() {
                TokenKind::At => break,
                TokenKind::Comma => {
                    let span = self.span();
                    self.pos += 1;
                    self.error("E103", "commas do not separate style values", span)
                        .note("A property takes space-separated values, like CSS. Commas only appear inside `record( )`, lists and call arguments.")
                        .suggest("Quote the whole value: `font \"Inter, system-ui, sans-serif\"`.");
                }
                TokenKind::Name => {
                    let text = self.peek().text.clone();
                    if tables::hard_item(&text) {
                        break;
                    }
                    // A property, attribute or modifier name only ends the list
                    // once this property has a value.
                    if !values.is_empty() && tables::soft_item(&text) {
                        break;
                    }
                    let before = self.pos;
                    match self.parse_expr() {
                        Some(value) => values.push(value),
                        None => {
                            if self.pos == before {
                                self.pos += 1;
                            }
                            break;
                        }
                    }
                }
                _ => {
                    let before = self.pos;
                    match self.parse_expr() {
                        Some(value) => values.push(value),
                        None => {
                            if self.pos == before {
                                self.pos += 1;
                            }
                            break;
                        }
                    }
                }
            }
        }
        values
    }

    fn parse_pseudo_block(&mut self, mode: UiMode) -> Option<PseudoBlock> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a pseudo-state after `when`") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let name = name_token.text.clone();
        if tables::pseudo(&name).is_none() {
            let span = name_token.span;
            self.error("E157", format!("`{name}` is not a pseudo-state Plain knows", span))
                .note("Plain spells them out: hover, focus, active, disabled, checked, first, last, odd, even, before, after, selection, placeholder.")
                .suggest(match name.as_str() {
                    "hovered" | "mouse-over" => "when hover",
                    "focused" => "when focus",
                    "pressed" => "when active",
                    _ => "when hover",
                });
            self.skip_line();
            self.skip_block();
            return None;
        }
        let span = keyword.span.merge(name_token.span);
        let mut scope = StyleScope::default();
        let allow = Allowed { pseudo: false, responsive: true, modifiers: false, show: false };
        self.parse_style_scope(&mut scope, allow, mode);
        Some(PseudoBlock { span, name, props: scope.props, responsive: scope.responsive })
    }

    /// `@tablet` followed by properties on the same line or in a block below.
    fn parse_responsive_block(&mut self, mode: UiMode) -> Option<ResponsiveBlock> {
        let at = self.advance();
        let (name, query, name_span) = match self.kind() {
            TokenKind::Name => {
                let token = self.advance();
                let query = tables::media_query(&token.text).unwrap_or("").to_string();
                (token.text.clone(), query, token.span)
            }
            TokenKind::Length | TokenKind::Int => {
                let token = self.advance();
                (token.text.clone(), format!("(min-width: {})", token.text), token.span)
            }
            _ => {
                let found = self.peek().clone();
                self.error(
                    "E156",
                    format!("expected a breakpoint name after `@`, found {}", found.describe()),
                    found.span,
                )
                .note("Plain ships mobile, tablet, laptop, desktop and wide, plus reduced-motion, dark, light, print, portrait and landscape.")
                .suggest("@tablet columns 2");
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        // An unresolved name is not an error here: `theme` may declare a custom
        // breakpoint, and only the semantic pass has seen the whole file.
        let span = at.span.merge(name_span);
        let mut scope = StyleScope::default();
        let allow = Allowed { pseudo: true, responsive: false, modifiers: true, show: true };
        self.parse_style_scope(&mut scope, allow, mode);
        Some(ResponsiveBlock {
            span,
            name,
            query,
            props: scope.props,
            pseudos: scope.pseudos,
            modifiers: scope.modifiers,
            show: scope.show,
        })
    }

    /// Reads properties from the rest of the line and from the indented block
    /// below it, which is the body of a `when` or `@breakpoint` block.
    /// `mode` decides who owns the indented block that may follow. On an
    /// element's own line, `box @tablet columns 2` is followed by the element's
    /// children, so the block belongs to the element. On its own line inside an
    /// element, `@tablet` owns the block below it.
    fn parse_style_scope(&mut self, scope: &mut StyleScope, allow: Allowed, mode: UiMode) {
        // Inline: the rest of this line, as in `when hover background red`.
        let mut read_inline = false;
        while !self.at_line_end() {
            let before = self.pos;
            if !self.parse_style_item(scope, allow) {
                break;
            }
            read_inline = true;
            if self.pos == before {
                self.pos += 1;
            }
        }
        if mode == UiMode::Inline || read_inline {
            return;
        }
        if self.kind() != TokenKind::Newline {
            return;
        }
        self.pos += 1;
        if self.kind() != TokenKind::Indent {
            return;
        }
        // Block: everything indented underneath.
        self.pos += 1;
        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            if self.kind() == TokenKind::Indent {
                let found = self.span();
                self.error("E102", "unexpected indentation inside a style block", found);
                self.pos += 1;
                self.skip_block();
                continue;
            }
            let before = self.pos;
            if !self.parse_style_item(scope, allow) {
                self.skip_line();
                self.skip_block();
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        self.eat(TokenKind::Dedent);
    }

    /// Parses one item of a style scope. Returns false when the token cannot
    /// start one, after reporting why.
    fn parse_style_item(&mut self, scope: &mut StyleScope, allow: Allowed) -> bool {
        match self.kind() {
            TokenKind::At => {
                if !allow.responsive {
                    let span = self.span();
                    self.error("E176", "a breakpoint block cannot contain another breakpoint block", span)
                        .suggest("Put both conditions on the outer block, or use two sibling blocks.");
                    self.skip_line();
                    self.skip_block();
                    return true;
                }
                match self.parse_responsive_block(UiMode::Block) {
                    Some(block) => scope.responsive.push(block),
                    None => return true,
                }
                true
            }
            TokenKind::Name => {
                let name = self.peek().text.clone();
                let name_span = self.span();
                if name == "when" {
                    if !allow.pseudo {
                        self.error("E177", "`when` blocks do not nest", name_span)
                            .note("Write the two states as sibling blocks; Plain combines them into one selector.")
                            .suggest("when hover\n  ...\nwhen focus\n  ...");
                        self.skip_line();
                        self.skip_block();
                        return true;
                    }
                    match self.parse_pseudo_block(UiMode::Block) {
                        Some(pseudo) => scope.pseudos.push(pseudo),
                        None => return true,
                    }
                    return true;
                }
                if name == "show" && allow.show {
                    self.pos += 1;
                    if let Some(condition) = self.parse_expr() {
                        scope.show = Some(condition);
                    }
                    return true;
                }
                if name == "on" {
                    self.error("E175", "an event handler belongs on the element, not in a style block", name_span)
                        .suggest("Move `on` up to the element's own indentation.");
                    self.skip_line();
                    self.skip_block();
                    return true;
                }
                if tables::MODIFIERS.contains(&name.as_str()) && !self.value_follows() && allow.modifiers {
                    self.pos += 1;
                    scope.modifiers.push(Modifier { span: name_span, name });
                    return true;
                }
                if tables::is_property(&name) {
                    match self.parse_style_prop() {
                        Some(prop) => scope.props.push(prop),
                        None => return true,
                    }
                    return true;
                }
                if tables::pseudo(&name).is_some() {
                    self.error("E144", format!("`{name}` is a pseudo-state, which needs `when`"), name_span)
                        .suggest(format!("when {name}"));
                } else if let Some(property) = tables::suggest_property(&name) {
                    self.error("E130", format!("`{name}` is not a property Plain knows", name_span))
                        .suggest(format!("Did you mean `{property}`?"));
                } else {
                    self.error("E130", format!("`{name}` is not a property Plain knows", name_span))
                        .note("A style block holds properties such as `color`, `padding`, `font-size`, `gap`.");
                }
                self.skip_line();
                self.skip_block();
                true
            }
            _ => {
                let found = self.peek().clone();
                self.error("E173", format!("expected a style property, found {}", found.describe()), found.span);
                false
            }
        }
    }

    // -- handlers ----------------------------------------------------------

    fn parse_handler(&mut self) -> Option<Handler> {
        let keyword = self.advance();
        let event_token = match self.expect(TokenKind::Name, "an event name after `on`") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let event = event_token.text.clone();
        let mut span = keyword.span.merge(event_token.span);
        let mut suffixes: Vec<(String, Span)> = Vec::new();
        while self.kind() == TokenKind::Dot {
            self.pos += 1;
            match self.expect(TokenKind::Name, "a modifier after `.`") {
                Some(token) => {
                    span = span.merge(token.span);
                    suffixes.push((token.text, token.span));
                }
                None => break,
            }
        }

        if !tables::EVENTS.contains(&event.as_str()) {
            self.error("E150", format!("`{event}` is not an event Plain knows", event_token.span))
                .note("Plain uses DOM event names: click, dblclick, input, change, submit, keydown, keyup, focus, blur, mouseenter, mouseleave, scroll, toggle.")
                .suggest(match event.as_str() {
                    "click" | "tap" => "on click",
                    "pressed" => "on keydown",
                    "hover" => "Style hover states with `when hover`; `on mouseenter` runs code.",
                    _ => "on click",
                });
        }

        let keyboard = matches!(event.as_str(), "keydown" | "keyup" | "keypress");
        let mut keys = Vec::new();
        let mut key_mods = Vec::new();
        let mut modifiers = Vec::new();
        for (suffix, suffix_span) in suffixes {
            if tables::EVENT_MODIFIERS.contains(&suffix.as_str()) {
                modifiers.push(suffix);
            } else if tables::KEY_MODIFIERS.contains(&suffix.as_str()) {
                if !keyboard {
                    self.error(
                        "E153",
                        format!("`.{suffix}` only filters keyboard events, not `{event}`"),
                        suffix_span,
                    )
                    .suggest(format!("on keydown.{suffix}.s"));
                }
                key_mods.push(suffix);
            } else if tables::KEY_FILTERS.iter().any(|(key, _)| *key == suffix) {
                if !keyboard {
                    let note = if suffix == "right" || suffix == "left" {
                        "For a mouse button, Plain uses the dedicated event."
                    } else {
                        "Key filters only make sense on keyboard events."
                    };
                    self.error(
                        "E151",
                        format!("`.{suffix}` only filters keyboard events, not `{event}`"),
                        suffix_span,
                    )
                    .note(note)
                    .suggest(if suffix == "right" {
                        "on contextmenu".to_string()
                    } else {
                        format!("on keydown.{suffix}")
                    });
                }
                keys.push(suffix);
            } else {
                self.error("E154", format!("`.{suffix}` is not a modifier Plain knows", suffix_span))
                    .note("Behaviour modifiers are prevent, stop, self, once, passive and capture. Key filters are enter, escape, tab, space, up, down, left, right, home, end, page-up, page-down, delete, backspace, insert.")
                    .suggest("on click.prevent");
            }
        }

        let body = if self.kind() == TokenKind::Arrow {
            self.pos += 1;
            let before = self.pos;
            let node = if self.assignment_ahead() {
                let target = self.parse_path();
                self.pos += 1;
                self.parse_expr().map(|value| {
                    let span = target.span.merge(value.span());
                    Node::Assign(Assign { span, target, value })
                })
            } else {
                self.parse_expr().map(Node::Expr)
            };
            match node {
                Some(node) => {
                    span = span.merge(node.span());
                    self.finish_line("the handler body");
                    vec![node]
                }
                None => {
                    if self.pos == before {
                        let found = self.peek().clone();
                        self.error(
                            "E171",
                            format!("`=>` needs a statement after it, found {}", found.describe()),
                            found.span,
                        )
                        .suggest("on click => count.set(count() + 1)");
                        self.pos += 1;
                    }
                    self.skip_line();
                    Vec::new()
                }
            }
        } else {
            self.parse_indented_block(BlockKind::Statements, &format!("on {event}"), "E174")
        };

        Some(Handler { span, event, keys, key_mods, modifiers, body })
    }

    // -- components --------------------------------------------------------

    fn parse_component(&mut self) -> Option<Component> {
        let keyword = self.advance();
        let name_token = match self.expect(TokenKind::Name, "a name after `component`") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let name = name_token.text.clone();
        if !is_component_name(&name) {
            self.error(
                "E111",
                format!("component names start with a capital letter, but `{name}` does not"),
                name_token.span,
            )
            .note("A capital first letter is how Plain tells a component from a built-in element.")
            .suggest(format!("component {}", capitalise(&name)));
        }
        let mut span = keyword.span.merge(name_token.span);
        let mut props = Vec::new();
        if self.kind() == TokenKind::LParen {
            self.pos += 1;
            while self.kind() != TokenKind::RParen && self.kind() != TokenKind::Eof {
                let before = self.pos;
                if let Some(prop) = self.parse_prop_decl() {
                    span = span.merge(prop.span);
                    props.push(prop);
                }
                if self.pos == before {
                    self.pos += 1;
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "`)`");
        }
        self.finish_line(&format!("`component {name}`"));

        let mut body = Vec::new();
        self.skip_newlines();
        if self.kind() == TokenKind::Indent {
            self.pos += 1;
            let mut in_props = true;
            loop {
                self.skip_newlines();
                if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                    break;
                }
                if self.kind() == TokenKind::Indent {
                    let found = self.span();
                    self.error("E102", "unexpected indentation", found);
                    self.pos += 1;
                    self.skip_block();
                    continue;
                }
                let before = self.pos;
                // Prop declarations come first, and the section ends at the
                // first line that is not one. That is what makes the block form
                // unambiguous without a separator.
                if in_props && self.prop_decl_ahead() {
                    if let Some(prop) = self.parse_prop_decl() {
                        span = span.merge(prop.span);
                        if props.iter().any(|existing| existing.name == prop.name) {
                            self.error(
                                "E114",
                                format!("the prop `{}` is declared twice", prop.name),
                                prop.span,
                            );
                        }
                        props.push(prop);
                    }
                } else {
                    in_props = false;
                    if self.is_name("children") || self.is_name("slot") {
                        if let Some(node) = self.parse_item(BlockKind::Nodes) {
                            span = span.merge(node.span());
                            body.push(node);
                        }
                    } else if let Some(node) = self.parse_item(BlockKind::Nodes) {
                        span = span.merge(node.span());
                        body.push(node);
                    }
                }
                if self.pos == before {
                    self.pos += 1;
                }
            }
            self.eat(TokenKind::Dedent);
        } else if props.is_empty() {
            let found = self.peek().clone();
            self.error(
                "E115",
                format!("`component {name}` has no body"),
                found.span,
            )
            .note("A component renders something, so it needs an indented block.")
            .suggest(format!("component {name}\n  text \"...\""));
        }

        Some(Component { span, name, props, body, doc: None })
    }

    /// True when the current line is `name: type`, `name = default` or both.
    fn prop_decl_ahead(&self) -> bool {
        self.kind() == TokenKind::Name
            && matches!(self.at(1).kind, TokenKind::Colon | TokenKind::Eq)
    }

    fn parse_prop_decl(&mut self) -> Option<PropDecl> {
        let name_token = match self.expect(TokenKind::Name, "a prop name") {
            Some(token) => token,
            None => return None,
        };
        self.check_identifier(&name_token);
        let ty = self.parse_optional_type();
        let default = if self.eat(TokenKind::Eq) {
            match self.parse_expr() {
                Some(value) => Some(value),
                None => {
                    self.skip_line();
                    None
                }
            }
        } else {
            None
        };
        let span = default.as_ref().map_or(name_token.span, |value| name_token.span.merge(value.span()));
        Some(PropDecl { span, name: name_token.text, ty, default })
    }

    // -- theme -------------------------------------------------------------

    fn parse_theme(&mut self) -> Option<Theme> {
        let keyword = self.advance();
        let mut span = keyword.span;
        let mut groups = Vec::new();
        self.finish_line("`theme`");
        self.skip_newlines();
        if self.kind() != TokenKind::Indent {
            let found = self.peek().clone();
            self.error("E116", "`theme` needs an indented block of categories", found.span)
                .note("Categories are colors, fonts, sizes, spacing, radius, shadows and breakpoints.")
                .suggest("theme\n  colors\n    primary #5dade2");
            self.skip_block();
            return Some(Theme { span, groups });
        }
        self.pos += 1;
        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            if self.kind() == TokenKind::Indent {
                let found = self.span();
                self.error("E102", "unexpected indentation inside `theme`", found)
                    .note("A category name comes first, then its tokens indented under it.");
                self.pos += 1;
                self.skip_block();
                continue;
            }
            let before = self.pos;
            if let Some(group) = self.parse_theme_group() {
                span = span.merge(group.span);
                groups.push(group);
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        self.eat(TokenKind::Dedent);
        Some(Theme { span, groups })
    }

    fn parse_theme_group(&mut self) -> Option<ThemeGroup> {
        let name_token = match self.expect(TokenKind::Name, "a theme category") {
            Some(token) => token,
            None => {
                self.skip_line();
                self.skip_block();
                return None;
            }
        };
        let name = name_token.text.clone();
        if !tables::THEME_CATEGORIES.contains(&name.as_str()) {
            self.error(
                "E117",
                format!("`{name}` is not a theme category"),
                name_token.span,
            )
            .note("Plain v0.1 knows colors, fonts, sizes, spacing, radius, shadows and breakpoints.")
            .suggest(
                closest_name(&name, tables::THEME_CATEGORIES).unwrap_or("colors").to_string(),
            );
            self.skip_line();
            self.skip_block();
            return None;
        }
        let mut group = ThemeGroup {
            span: name_token.span,
            name,
            entries: Vec::new(),
            dark: Vec::new(),
            breakpoints: Vec::new(),
        };
        self.finish_line(&format!("the category `{}`", group.name));
        self.skip_newlines();
        if self.kind() != TokenKind::Indent {
            let found = self.peek().clone();
            self.error(
                "E118",
                format!("the category `{}` needs tokens indented underneath it", group.name),
                found.span,
            )
            .suggest(format!("{}\n  md 1rem", group.name));
            return Some(group);
        }
        self.pos += 1;
        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            if self.kind() == TokenKind::Indent {
                let found = self.span();
                self.error("E102", "unexpected indentation inside a theme category", found)
                    .note("Only `dark` opens a nested block inside a category.");
                self.pos += 1;
                self.skip_block();
                continue;
            }
            let before = self.pos;
            if self.is_name("dark") {
                let dark_token = self.advance();
                self.finish_line("`dark`");
                self.skip_newlines();
                if self.kind() == TokenKind::Indent {
                    self.pos += 1;
                    while let Some(entry) = self.parse_theme_entry(&group.name) {
                        group.span = group.span.merge(entry.span);
                        group.dark.push(entry);
                        self.skip_newlines();
                        if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                            break;
                        }
                    }
                    self.eat(TokenKind::Dedent);
                } else {
                    self.error(
                        "E119",
                        "`dark` needs the overridden tokens indented underneath it",
                        dark_token.span,
                    )
                    .suggest(format!("dark\n  {} ...", group.name));
                }
            } else if let Some(entry) = self.parse_theme_entry(&group.name) {
                group.span = group.span.merge(entry.span);
                if group.name == "breakpoints" {
                    let query = match entry.values.first() {
                        Some(Expr::Length(value, unit, _)) => {
                            format!("(min-width: {}{})", trim_number(*value), unit)
                        }
                        Some(Expr::Str(parts, _)) if parts.len() == 1 => match &parts[0] {
                            StrPart::Text(text) => text.clone(),
                            StrPart::Expr(_) => String::new(),
                        },
                        Some(other) => {
                            self.error(
                                "E122",
                                "a custom breakpoint needs a width such as `1800px`",
                                other.span(),
                            )
                            .suggest("huge 1800px");
                            String::new()
                        }
                        None => {
                            let span = entry.span;
                            self.error("E122", "a custom breakpoint needs a width such as `1800px`", span)
                                .suggest("huge 1800px");
                            String::new()
                        }
                    };
                    group.breakpoints.push((entry.name, query, entry.span));
                } else {
                    group.entries.push(entry);
                }
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        self.eat(TokenKind::Dedent);
        Some(group)
    }

    /// One `name value...` line inside a theme category. Returns `None` at the
    /// end of the block, which is how the caller's loop stops.
    fn parse_theme_entry(&mut self, category: &str) -> Option<ThemeEntry> {
        self.skip_newlines();
        if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
            return None;
        }
        if self.kind() != TokenKind::Name {
            let found = self.peek().clone();
            self.error(
                "E123",
                format!("expected a token name inside `{category}`, found {}", found.describe()),
                found.span,
            );
            self.skip_line();
            return None;
        }
        let name_token = self.advance();
        let values = self.parse_values();
        if values.is_empty() {
            self.error(
                "E114",
                format!("the {category} token `{}` needs a value", name_token.text),
                name_token.span,
            )
            .suggest(format!("{} ...", name_token.text));
        }
        self.finish_line(&format!("the token `{}`", name_token.text));
        let span = values.iter().fold(name_token.span, |acc, value| acc.merge(value.span()));
        Some(ThemeEntry { span, name: name_token.text, values })
    }

    // -- types -------------------------------------------------------------

    /// `: string`, `: list(todo)`, `: string?`, `: record(name: string)`.
    fn parse_optional_type(&mut self) -> Option<TypeRef> {
        if self.kind() != TokenKind::Colon {
            return None;
        }
        self.pos += 1;
        self.parse_type()
    }

    fn parse_type(&mut self) -> Option<TypeRef> {
        let mut ty = self.parse_type_atom()?;
        while self.kind() == TokenKind::Question {
            let question = self.advance();
            let span = ty.span().merge(question.span);
            ty = TypeRef::Optional(Box::new(ty), span);
        }
        // A union is written `string | number`.
        while self.kind() == TokenKind::Pipe {
            let pipe = self.advance();
            let mut options = vec![ty];
            loop {
                match self.parse_type_atom() {
                    Some(next) => options.push(next),
                    None => break,
                }
                if self.kind() != TokenKind::Pipe {
                    break;
                }
                self.pos += 1;
            }
            let span = options.iter().fold(pipe.span, |acc, option| acc.merge(option.span()));
            ty = TypeRef::Union(options, span);
        }
        Some(ty)
    }

    fn parse_type_atom(&mut self) -> Option<TypeRef> {
        match self.kind() {
            TokenKind::Name => {
                let token = self.advance();
                let name = token.text.clone();
                if self.kind() == TokenKind::LParen {
                    self.pos += 1;
                    let mut args = Vec::new();
                    while self.kind() != TokenKind::RParen && self.kind() != TokenKind::Eof {
                        // `record(name: string)` carries field names.
                        if self.kind() == TokenKind::Name && self.at(1).kind == TokenKind::Colon {
                            let field = self.advance();
                            self.pos += 1;
                            match self.parse_type() {
                                Some(ty) => args.push((field.text, ty)),
                                None => break,
                            }
                        } else {
                            match self.parse_type() {
                                Some(ty) => args.push((String::new(), ty)),
                                None => break,
                            }
                        }
                        if !self.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    let close = self.expect(TokenKind::RParen, "`)`").map_or(token.span, |t| t.span);
                    let span = token.span.merge(close);
                    if name == "record" {
                        return Some(TypeRef::Record(args, span));
                    }
                    let types: Vec<TypeRef> = args.into_iter().map(|(_, ty)| ty).collect();
                    if name == "list" && types.len() == 1 {
                        return Some(TypeRef::List(Box::new(types[0].clone()), span));
                    }
                    return Some(TypeRef::Generic(name, types, span));
                }
                if name == "list" && self.kind() == TokenKind::LBracket {
                    self.pos += 1;
                    let inner = self.parse_type();
                    let close = self.expect(TokenKind::RBracket, "`]`").map_or(token.span, |t| t.span);
                    let span = token.span.merge(close);
                    return Some(match inner {
                        Some(inner) => TypeRef::List(Box::new(inner), span),
                        None => TypeRef::Named(name, span),
                    });
                }
                Some(TypeRef::Named(name, token.span))
            }
            TokenKind::LBracket => {
                let open = self.advance();
                let inner = self.parse_type();
                let close = self.expect(TokenKind::RBracket, "`]`").map_or(open.span, |t| t.span);
                let span = open.span.merge(close);
                Some(match inner {
                    Some(inner) => TypeRef::List(Box::new(inner), span),
                    None => TypeRef::Named("list".to_string(), span),
                })
            }
            TokenKind::StrStart => {
                // A quoted type is a literal type; v0.1 keeps it as text.
                let text = self.string_literal_text();
                Some(TypeRef::Named(text.0, text.1))
            }
            _ => {
                let found = self.peek().clone();
                self.error("E124", format!("expected a type, found {}", found.describe()), found.span)
                    .note("Types are string, number, bool, list(x), record(field: x), or a component's own name.")
                    .suggest(": string");
                None
            }
        }
    }

    // -- expressions -------------------------------------------------------

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_lambda()
    }

    fn parse_lambda(&mut self) -> Option<Expr> {
        if let Some(params) = self.lambda_ahead() {
            let start = self.span();
            let parenthesised = self.kind() == TokenKind::LParen;
            if parenthesised {
                self.pos += 1;
            }
            for (index, _) in params.iter().enumerate() {
                self.pos += 1; // the parameter name
                if index + 1 < params.len() {
                    self.eat(TokenKind::Comma);
                }
            }
            if parenthesised {
                self.expect(TokenKind::RParen, "`)`");
            }
            let arrow = self.expect(TokenKind::Arrow, "`=>`");
            let body = match self.parse_expr() {
                Some(body) => body,
                None => {
                    self.skip_line();
                    return None;
                }
            };
            let span = arrow.map_or_else(|| start.merge(body.span()), |a| start.merge(a.span).merge(body.span()));
            return Some(Expr::Lambda(params, Box::new(body), span));
        }
        self.parse_conditional()
    }

    /// Recognises `it => e`, `(a, b) => e` and `() => e` by lookahead, so that
    /// an ordinary parenthesised expression is not mistaken for a parameter
    /// list.
    fn lambda_ahead(&self) -> Option<Vec<String>> {
        if self.kind() == TokenKind::Name && self.at(1).kind == TokenKind::Arrow {
            let name = self.peek().text.clone();
            if tables::RESERVED.contains(&name.as_str()) {
                return None;
            }
            return Some(vec![name]);
        }
        if self.kind() != TokenKind::LParen {
            return None;
        }
        let mut offset = 1usize;
        let mut params = Vec::new();
        if self.at(offset).kind == TokenKind::RParen {
            offset += 1;
            return (self.at(offset).kind == TokenKind::Arrow).then_some(params);
        }
        loop {
            if self.at(offset).kind != TokenKind::Name {
                return None;
            }
            params.push(self.at(offset).text.clone());
            offset += 1;
            match self.at(offset).kind {
                TokenKind::Comma => offset += 1,
                TokenKind::RParen => {
                    offset += 1;
                    break;
                }
                _ => return None,
            }
        }
        (self.at(offset).kind == TokenKind::Arrow).then_some(params)
    }

    fn parse_conditional(&mut self) -> Option<Expr> {
        if !self.is_name("if") {
            return self.parse_pipe();
        }
        let keyword = self.advance();
        let condition = self.parse_or()?;
        if !self.is_name("then") {
            let found = self.peek().clone();
            self.error(
                "E125",
                format!("an expression `if` needs `then`, found {}", found.describe()),
                found.span,
            )
            .note("`if a then b else c` produces a value. A block `if` renders elements and needs no `then`.")
            .suggest("if a then b else c");
            self.skip_line();
            return None;
        }
        self.pos += 1;
        let then = self.parse_conditional()?;
        if !self.is_name("else") {
            self.error("E126", "an expression `if` needs an `else`", keyword.span.merge(then.span()))
                .note("Because it produces a value, both branches are required.")
                .suggest("if a then b else c");
            return Some(Expr::Conditional(
                Box::new(condition),
                Box::new(then),
                Box::new(Expr::None(self.span())),
                keyword.span.merge(then.span()),
            ));
        }
        self.pos += 1;
        let otherwise = self.parse_conditional()?;
        let span = keyword.span.merge(otherwise.span());
        Some(Expr::Conditional(Box::new(condition), Box::new(then), Box::new(otherwise), span))
    }

    /// `value | filter(f) | count()` — the pipeline reads left to right.
    fn parse_pipe(&mut self) -> Option<Expr> {
        let mut left = self.parse_or()?;
        while self.kind() == TokenKind::Pipe {
            let pipe = self.advance();
            match self.parse_or() {
                Some(right) => {
                    let span = left.span().merge(pipe.span).merge(right.span());
                    left = Expr::Pipe(Box::new(left), Box::new(right), span);
                }
                None => return Some(left),
            }
        }
        Some(left)
    }

    fn parse_or(&mut self) -> Option<Expr> {
        let mut left = self.parse_and()?;
        while self.is_name("or") {
            let operator = self.advance();
            match self.parse_and() {
                Some(right) => left = binary("or", left, operator.span, right),
                None => return Some(left),
            }
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<Expr> {
        let mut left = self.parse_equality()?;
        while self.is_name("and") {
            let operator = self.advance();
            match self.parse_equality() {
                Some(right) => left = binary("and", left, operator.span, right),
                None => return Some(left),
            }
        }
        Some(left)
    }

    fn parse_equality(&mut self) -> Option<Expr> {
        let mut left = self.parse_comparison()?;
        loop {
            let operator = match self.kind() {
                TokenKind::EqEq => "==",
                TokenKind::BangEq => "!=",
                _ => break,
            };
            let span = self.advance().span;
            match self.parse_comparison() {
                Some(right) => left = binary(operator, left, span, right),
                None => break,
            }
        }
        Some(left)
    }

    fn parse_comparison(&mut self) -> Option<Expr> {
        let mut left = self.parse_range()?;
        loop {
            let operator = match self.kind() {
                TokenKind::Lt => "<",
                TokenKind::LtEq => "<=",
                TokenKind::Gt => ">",
                TokenKind::GtEq => ">=",
                _ => break,
            };
            let span = self.advance().span;
            match self.parse_range() {
                Some(right) => left = binary(operator, left, span, right),
                None => break,
            }
        }
        Some(left)
    }

    fn parse_range(&mut self) -> Option<Expr> {
        let left = self.parse_additive()?;
        if self.kind() != TokenKind::DotDot {
            return Some(left);
        }
        let dots = self.advance();
        match self.parse_additive() {
            Some(right) => {
                let span = left.span().merge(dots.span).merge(right.span());
                Some(Expr::Range(Box::new(left), Box::new(right), span))
            }
            None => Some(left),
        }
    }

    fn parse_additive(&mut self) -> Option<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let operator = match self.kind() {
                TokenKind::Plus => "+",
                TokenKind::Minus => "-",
                _ => break,
            };
            let span = self.advance().span;
            match self.parse_multiplicative() {
                Some(right) => left = binary(operator, left, span, right),
                None => break,
            }
        }
        Some(left)
    }

    fn parse_multiplicative(&mut self) -> Option<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let operator = match self.kind() {
                TokenKind::Star => "*",
                TokenKind::Slash => "/",
                TokenKind::Percent => "%",
                _ => break,
            };
            let span = self.advance().span;
            match self.parse_unary() {
                Some(right) => left = binary(operator, left, span, right),
                None => break,
            }
        }
        Some(left)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        if self.is_name("not") {
            let operator = self.advance();
            let operand = self.parse_unary()?;
            let span = operator.span.merge(operand.span());
            return Some(Expr::Unary("not".to_string(), Box::new(operand), span));
        }
        if self.kind() == TokenKind::Minus {
            let operator = self.advance();
            let operand = self.parse_unary()?;
            let span = operator.span.merge(operand.span());
            return Some(Expr::Unary("-".to_string(), Box::new(operand), span));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut base = self.parse_primary()?;
        loop {
            match self.kind() {
                TokenKind::Dot => {
                    self.pos += 1;
                    let field = match self.expect(TokenKind::Name, "a field name after `.`") {
                        Some(field) => field,
                        None => break,
                    };
                    if self.kind() == TokenKind::LParen {
                        self.pos += 1;
                        let args = self.parse_arguments();
                        let close = self.expect(TokenKind::RParen, "`)`").map_or(field.span, |t| t.span);
                        let span = base.span().merge(close);
                        base = Expr::Method(Box::new(base), field.text, args, span);
                    } else {
                        let span = base.span().merge(field.span);
                        base = Expr::Member(Box::new(base), field.text, span);
                    }
                }
                TokenKind::LBracket => {
                    let open = self.advance();
                    match self.parse_expr() {
                        Some(index) => {
                            let close = self.expect(TokenKind::RBracket, "`]`").map_or(index.span(), |t| t.span);
                            let span = base.span().merge(open.span).merge(close);
                            base = Expr::Index(Box::new(base), Box::new(index), span);
                        }
                        None => break,
                    }
                }
                TokenKind::LParen => {
                    self.pos += 1;
                    let args = self.parse_arguments();
                    let close = self.expect(TokenKind::RParen, "`)`").map_or(base.span(), |t| t.span);
                    let span = base.span().merge(close);
                    base = Expr::Call(Box::new(base), args, span);
                }
                _ => break,
            }
        }
        Some(base)
    }

    fn parse_arguments(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        if self.kind() == TokenKind::RParen {
            return args;
        }
        loop {
            if self.kind() == TokenKind::Name && self.at(1).kind == TokenKind::Colon {
                let name = self.peek().text.clone();
                let span = self.span();
                self.error(
                    "E127",
                    format!("`{name}:` is a named argument, which only component invocations take"),
                    span,
                )
                .note("Functions take positional arguments. Named props belong on a Capitalised component.")
                .suggest(format!("Pass the value on its own: `f({name})`."));
                self.pos += 2;
            }
            let before = self.pos;
            match self.parse_expr() {
                Some(arg) => args.push(arg),
                None => {
                    if self.pos == before {
                        self.pos += 1;
                    }
                    break;
                }
            }
            if !self.eat(TokenKind::Comma) {
                break;
            }
            if self.kind() == TokenKind::RParen || self.at_line_end() {
                break;
            }
        }
        args
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Int => {
                self.pos += 1;
                Some(Expr::Int(token.value as i64, token.span))
            }
            TokenKind::Float => {
                self.pos += 1;
                Some(Expr::Float(token.value, token.span))
            }
            TokenKind::Length => {
                self.pos += 1;
                Some(Expr::Length(token.value, token.unit.clone(), token.span))
            }
            TokenKind::Color => {
                self.pos += 1;
                Some(Expr::Color(token.text.clone(), token.span))
            }
            TokenKind::StrStart => Some(self.parse_string()),
            TokenKind::Name => {
                self.pos += 1;
                match token.text.as_str() {
                    "true" => Some(Expr::Bool(true, token.span)),
                    "false" => Some(Expr::Bool(false, token.span)),
                    "none" => Some(Expr::None(token.span)),
                    "record" if self.kind() == TokenKind::LParen => {
                        self.pos += 1;
                        self.parse_record(token.span)
                    }
                    _ => Some(Expr::Name(token.text.clone(), token.span)),
                }
            }
            TokenKind::LParen => {
                self.pos += 1;
                let inner = self.parse_expr();
                // Parentheses only group; they do not build a node of their own.
                self.expect(TokenKind::RParen, "`)`");
                inner
            }
            TokenKind::LBracket => {
                self.pos += 1;
                self.parse_list(token.span)
            }
            TokenKind::Ellipsis => {
                self.pos += 1;
                match self.parse_expr() {
                    Some(inner) => {
                        let span = token.span.merge(inner.span());
                        Some(Expr::Spread(Box::new(inner), span))
                    }
                    None => None,
                }
            }
            TokenKind::Arrow => {
                self.error("E128", "`=>` has no parameters before it", token.span)
                    .note("A lambda is `name => expression` or `(a, b) => expression`.")
                    .suggest("it => it.name");
                self.pos += 1;
                None
            }
            TokenKind::Colon => {
                self.error("E129", "a `:` here has nothing to annotate", token.span)
                    .note("Types follow a name in a `let`, a parameter or a prop declaration.")
                    .suggest("let name: string = ...");
                self.pos += 1;
                None
            }
            TokenKind::InterpStart | TokenKind::InterpEnd | TokenKind::StrText | TokenKind::StrEnd => {
                self.error("E172", "interpolation only appears inside a string literal", token.span)
                    .suggest("Write `\"~(value)\"` inside a string.");
                self.pos += 1;
                None
            }
            _ => {
                self.error(
                    "E170",
                    format!("expected a value, found {}", token.describe()),
                    token.span,
                );
                None
            }
        }
    }

    fn parse_list(&mut self, open: Span) -> Option<Expr> {
        let mut items = Vec::new();
        if self.kind() != TokenKind::RBracket {
            loop {
                let before = self.pos;
                match self.parse_expr() {
                    Some(item) => items.push(item),
                    None => {
                        if self.pos == before {
                            self.pos += 1;
                        }
                        break;
                    }
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                if self.kind() == TokenKind::RBracket || self.at_line_end() {
                    break;
                }
            }
        }
        let close = self.expect(TokenKind::RBracket, "`]`").map_or(open, |token| token.span);
        Some(Expr::List(items, open.merge(close)))
    }

    fn parse_record(&mut self, open: Span) -> Option<Expr> {
        let mut fields = Vec::new();
        if self.kind() != TokenKind::RParen {
            loop {
                if self.kind() == TokenKind::Ellipsis {
                    let dots = self.advance();
                    match self.parse_expr() {
                        Some(inner) => {
                            let span = dots.span.merge(inner.span());
                            fields.push((String::new(), Expr::Spread(Box::new(inner), span)));
                        }
                        None => break,
                    }
                } else if self.kind() == TokenKind::Name && self.at(1).kind == TokenKind::Colon {
                    let field = self.advance();
                    self.pos += 1;
                    match self.parse_expr() {
                        Some(value) => fields.push((field.text, value)),
                        None => break,
                    }
                } else {
                    let found = self.peek().clone();
                    self.error(
                        "E131",
                        format!("a record field is `name: value`, found {}", found.describe()),
                        found.span,
                    )
                    .suggest("record(name: \"Alice\", age: 30)");
                    self.skip_to_comma_or_close();
                    if self.kind() != TokenKind::Comma {
                        break;
                    }
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                if self.kind() == TokenKind::RParen || self.at_line_end() {
                    break;
                }
            }
        }
        let close = self.expect(TokenKind::RParen, "`)`").map_or(open, |token| token.span);
        Some(Expr::Record(fields, open.merge(close)))
    }

    /// A string literal, whose pieces are already inline in the token stream so
    /// that interpolations keep real spans into the source file.
    fn parse_string(&mut self) -> Expr {
        let open = self.advance(); // StrStart
        let mut parts = Vec::new();
        let mut span = open.span;
        loop {
            match self.kind() {
                TokenKind::StrText => {
                    let token = self.advance();
                    span = span.merge(token.span);
                    parts.push(StrPart::Text(token.text));
                }
                TokenKind::InterpStart => {
                    let start = self.advance();
                    match self.parse_expr() {
                        Some(value) => {
                            span = span.merge(value.span());
                            parts.push(StrPart::Expr(value));
                        }
                        None => {
                            span = span.merge(start.span);
                        }
                    }
                    match self.expect(TokenKind::InterpEnd, "`)` to close the interpolation") {
                        Some(close) => span = span.merge(close.span),
                        None => {
                            self.skip_line();
                            break;
                        }
                    }
                }
                TokenKind::StrEnd => {
                    let close = self.advance();
                    span = span.merge(close.span);
                    break;
                }
                TokenKind::Eof => {
                    self.error("E132", "the string literal is never closed", span)
                        .suggest("Add the missing `\"`.");
                    break;
                }
                _ => {
                    let found = self.peek().clone();
                    self.error(
                        "E132",
                        format!("the string literal is never closed; found {}", found.describe()),
                        span,
                    )
                    .suggest("Add the missing `\"`.");
                    self.skip_line();
                    break;
                }
            }
        }
        Expr::Str(parts, span)
    }

    /// Reads a string literal and flattens it to text, for type annotations.
    fn string_literal_text(&mut self) -> (String, Span) {
        let expr = self.parse_string();
        match expr {
            Expr::Str(parts, span) => {
                let mut text = String::new();
                for part in parts {
                    match part {
                        StrPart::Text(value) => text.push_str(&value),
                        StrPart::Expr(_) => text.push_str("..."),
                    }
                }
                (text, span)
            }
            other => (String::new(), other.span()),
        }
    }
}

/// Which decorations a style scope accepts.
#[derive(Debug, Clone, Copy)]
struct Allowed {
    pseudo: bool,
    responsive: bool,
    modifiers: bool,
    show: bool,
}

/// Folds the decorations of a component invocation into its AST node.
fn apply_use_decorations(used: &mut Use, deco: Decorations) {
    used.props_style.extend(deco.props);
    used.modifiers.extend(deco.modifiers);
    used.pseudos.extend(deco.pseudos);
    used.responsive.extend(deco.responsive);
    used.handlers.extend(deco.handlers);
    used.attrs.extend(deco.attrs);
    if let Some(show) = deco.show {
        used.show = Some(show);
    }
}

/// Moves the block-level decorations into the ones read from the element line.
fn merge(target: &mut Decorations, source: Decorations) {
    target.props.extend(source.props);
    target.attrs.extend(source.attrs);
    target.modifiers.extend(source.modifiers);
    target.pseudos.extend(source.pseudos);
    target.responsive.extend(source.responsive);
    target.handlers.extend(source.handlers);
    target.positional.extend(source.positional);
    target.slots.extend(source.slots);
    target.props_named.extend(source.props_named);
    if target.bind.is_none() {
        target.bind = source.bind;
    }
    if target.show.is_none() {
        target.show = source.show;
    }
}

fn binary(operator: &str, left: Expr, operator_span: Span, right: Expr) -> Expr {
    let span = left.span().merge(operator_span).merge(right.span());
    Expr::Binary(operator.to_string(), Box::new(left), Box::new(right), span)
}

/// Joins the values of one property into a single expression. A property with
/// several values is kept as a list so the CSS pass can emit them in order.
fn join_values(mut values: Vec<Expr>, span: Span) -> Expr {
    if values.len() == 1 {
        return values.remove(0);
    }
    let span = values.iter().fold(span, |acc, value| acc.merge(value.span()));
    Expr::List(values, span)
}

/// True when `name` starts with a capital letter, which is how Plain
/// distinguishes a component from a built-in element.
fn is_component_name(name: &str) -> bool {
    name.chars().next().is_some_and(|first| first.is_ascii_uppercase())
}

/// Keywords that introduce a statement or a UI construct.
///
/// This list deliberately excludes anything that is also a property or
/// attribute name (`type`, `transition`, `animation`): those are styling, and
/// deferred keywords are reported by [`Parser::parse_item`].
fn is_statement_keyword(name: &str) -> bool {
    matches!(
        name,
        "let" | "derive" | "fn" | "if" | "each" | "children" | "slot" | "return"
    )
}

fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The nearest name in a vocabulary, for "did you mean" suggestions. A
/// distance above two is not worth suggesting: a confident wrong guess is worse
/// than no guess at all.
fn closest_name(target: &str, candidates: &'static [&'static str]) -> Option<&'static str> {
    let target: Vec<char> = target.chars().collect();
    let mut best: Option<(&'static str, usize)> = None;
    for candidate in candidates {
        let chars: Vec<char> = candidate.chars().collect();
        let distance = tables::edit_distance(&target, &chars);
        let better = match best {
            Some((_, best_distance)) => distance < best_distance,
            None => true,
        };
        if distance <= 2 && better {
            best = Some((candidate, distance));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// Renders a number without a trailing `.0`, for media queries.
fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> (Program, Vec<Diagnostic>) {
        parse_source(FileId(0), src)
    }

    /// Parses and asserts nothing was reported, so a test fails loudly instead
    /// of quietly producing a smaller tree.
    fn ok(src: &str) -> Program {
        let (program, diagnostics) = parse(src);
        assert!(diagnostics.is_empty(), "unexpected diagnostics for:\n{src}\n{diagnostics:#?}");
        program
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    fn first_element(program: &Program) -> &Element {
        match program.body.first() {
            Some(Node::Element(element)) => element,
            other => panic!("expected an element, got {other:?}"),
        }
    }

    fn prop_names(element: &Element) -> Vec<&str> {
        element.props.iter().map(|p| p.name.as_str()).collect()
    }

    /// Renders an expression back to readable text. Matching every variant is
    /// deliberate: adding an [`Expr`] case breaks this function, which is the
    /// reminder the tests need.
    fn text_of(expr: &Expr) -> String {
        match expr {
            Expr::Int(value, _) => value.to_string(),
            Expr::Float(value, _) => format!("{value}"),
            Expr::Length(value, unit, _) => format!("{}{}", trim_number(*value), unit),
            Expr::Color(text, _) => text.clone(),
            Expr::Bool(value, _) => value.to_string(),
            Expr::None(_) => "none".to_string(),
            Expr::Name(name, _) => name.clone(),
            Expr::Str(parts, _) => parts
                .iter()
                .map(|part| match part {
                    StrPart::Text(text) => text.clone(),
                    StrPart::Expr(_) => "~()".to_string(),
                })
                .collect(),
            Expr::Member(target, field, _) => format!("{}.{}", text_of(target), field),
            Expr::Index(target, index, _) => format!("{}[{}]", text_of(target), text_of(index)),
            Expr::Call(callee, args, _) => format!("{}({})", text_of(callee), join(args)),
            Expr::Method(target, method, args, _) => {
                format!("{}.{}({})", text_of(target), method, join(args))
            }
            Expr::Unary(operator, operand, _) => format!("{operator} {}", text_of(operand)),
            Expr::Binary(operator, left, right, _) => {
                format!("{} {} {}", text_of(left), operator, text_of(right))
            }
            Expr::Pipe(left, right, _) => format!("{} | {}", text_of(left), text_of(right)),
            Expr::Conditional(condition, then, otherwise, _) => format!(
                "if {} then {} else {}",
                text_of(condition),
                text_of(then),
                text_of(otherwise)
            ),
            Expr::Lambda(params, body, _) => format!("{} => {}", params.join(", "), text_of(body)),
            Expr::List(items, _) => join(items),
            Expr::Record(fields, _) => fields
                .iter()
                .map(|(key, value)| format!("{key}: {}", text_of(value)))
                .collect::<Vec<_>>()
                .join(", "),
            Expr::Spread(inner, _) => format!("...{}", text_of(inner)),
            Expr::Range(from, to, _) => format!("{}..{}", text_of(from), text_of(to)),
        }
    }

    fn join(items: &[Expr]) -> String {
        items.iter().map(text_of).collect::<Vec<_>>().join(", ")
    }

    fn values(prop: &StyleProp) -> String {
        prop.values.iter().map(text_of).collect::<Vec<_>>().join(" ")
    }

    // -- structure ---------------------------------------------------------

    #[test]
    fn parses_a_text_element_with_a_static_string() {
        let program = ok("text \"Hello, world\"\n");
        let element = first_element(&program);
        assert_eq!(element.name, "text");
        assert_eq!(element.args.len(), 1);
        assert_eq!(text_of(&element.args[0]), "Hello, world");
    }

    #[test]
    fn parses_nested_children_by_indentation() {
        let program = ok("box\n  text \"inner\"\n");
        let element = first_element(&program);
        assert_eq!(element.name, "box");
        assert!(element.args.is_empty());
        assert_eq!(element.children.len(), 1);
        assert!(matches!(&element.children[0], Node::Element(child) if child.name == "text"));
    }

    #[test]
    fn siblings_at_the_same_indentation_are_not_children() {
        let program = ok("text \"a\"\ntext \"b\"\n");
        assert_eq!(program.body.len(), 2);
        assert!(first_element(&program).children.is_empty());
    }

    #[test]
    fn an_element_cannot_be_written_inline_after_its_parent() {
        let (_, diagnostics) = parse("box text \"inner\"\n");
        assert!(codes(&diagnostics).contains(&"E121"));
    }

    // -- styling -----------------------------------------------------------

    #[test]
    fn inline_properties_stay_on_the_element_line() {
        let program = ok("button \"Save\" fill primary size lg\n");
        let element = first_element(&program);
        assert_eq!(element.args.len(), 1);
        assert_eq!(prop_names(element), vec!["fill", "size"]);
        assert_eq!(values(&element.props[0]), "primary");
    }

    #[test]
    fn a_value_is_not_mistaken_for_the_next_property() {
        let program = ok("box text-align left\n");
        let element = first_element(&program);
        assert_eq!(prop_names(element), vec!["text-align"]);
        assert_eq!(values(&element.props[0]), "left");

        // ... but once a property has a value, the next name starts a new item.
        let program = ok("box position absolute left 0 top 0\n");
        let element = first_element(&program);
        assert_eq!(prop_names(element), vec!["position", "left", "top"]);
    }

    #[test]
    fn display_flex_is_one_property_not_two() {
        let program = ok("box display flex\n");
        let element = first_element(&program);
        assert_eq!(prop_names(element), vec!["display"]);
        assert_eq!(values(&element.props[0]), "flex");
    }

    #[test]
    fn flex_direction_row_is_a_value_not_a_child_element() {
        let program = ok("box flex-direction row\n");
        let element = first_element(&program);
        assert_eq!(prop_names(element), vec!["flex-direction"]);
        assert_eq!(values(&element.props[0]), "row");
        assert!(element.children.is_empty());
    }

    #[test]
    fn visibility_hidden_is_a_value_even_though_hidden_is_a_modifier() {
        let program = ok("box visibility hidden\n");
        let element = first_element(&program);
        assert_eq!(prop_names(element), vec!["visibility"]);
        assert_eq!(values(&element.props[0]), "hidden");
        assert!(element.modifiers.is_empty());
    }

    #[test]
    fn fill_alone_is_a_modifier_and_fill_with_a_colour_is_a_property() {
        let modifier = first_element(&ok("box fill\n"));
        assert_eq!(modifier.modifiers.len(), 1);
        assert_eq!(modifier.modifiers[0].name, "fill");
        assert!(modifier.props.is_empty());

        let property = first_element(&ok("box fill primary\n"));
        assert!(property.modifiers.is_empty());
        assert_eq!(prop_names(&property), vec!["fill"]);
    }

    #[test]
    fn modifiers_need_no_value() {
        let program = ok("box hidden\nimage \"/a.png\" decorative\n");
        let element = first_element(&program);
        assert_eq!(element.modifiers[0].name, "hidden");
        match &program.body[1] {
            Node::Element(image) => assert_eq!(image.modifiers[0].name, "decorative"),
            other => panic!("expected an image, got {other:?}"),
        }
    }

    #[test]
    fn attributes_carry_values() {
        let program = ok("input type email placeholder \"you@example.com\"\n");
        let element = first_element(&program);
        assert_eq!(element.attrs.len(), 2);
        assert_eq!(element.attrs[0].name, "type");
        assert_eq!(element.attrs[1].name, "placeholder");
        assert!(element.props.is_empty(), "`type` is an attribute, not a property");
    }

    #[test]
    fn an_attribute_without_a_value_is_an_error() {
        let (_, diagnostics) = parse("box id\n");
        assert!(codes(&diagnostics).contains(&"E145"));
    }

    #[test]
    fn a_bare_value_on_its_own_line_explains_itself() {
        let (_, diagnostics) = parse("box\n  \"stray\"\n");
        assert!(codes(&diagnostics).contains(&"E161"));
    }

    #[test]
    fn a_style_property_at_the_top_level_explains_itself() {
        let (_, diagnostics) = parse("color red\n");
        assert!(codes(&diagnostics).contains(&"E179"));
    }

    #[test]
    fn an_unknown_element_is_reported_with_a_suggestion() {
        let (_, diagnostics) = parse("buton \"Save\"\n");
        assert!(codes(&diagnostics).contains(&"E120"));
        assert!(diagnostics[0].suggestion.as_deref().unwrap().contains("button"));
    }

    #[test]
    fn a_comma_in_a_style_list_points_at_the_quoting_fix() {
        let (_, diagnostics) = parse("box font Inter, sans-serif\n");
        assert!(codes(&diagnostics).contains(&"E103"));
        assert!(diagnostics[0]
            .suggestion
            .as_deref()
            .unwrap()
            .contains("\"Inter, system-ui, sans-serif\""));
    }

    // -- pseudo-states and breakpoints -------------------------------------

    #[test]
    fn when_blocks_attach_to_the_enclosing_element() {
        let program = ok("button \"x\"\n  when hover\n    background accent\n");
        let element = first_element(&program);
        assert_eq!(element.pseudos.len(), 1);
        assert_eq!(element.pseudos[0].name, "hover");
        assert_eq!(element.pseudos[0].props.len(), 1);
        assert!(element.children.is_empty());
    }

    #[test]
    fn a_when_block_can_be_written_on_one_line() {
        let program = ok("button \"x\"\n  when hover background accent\n");
        let element = first_element(&program);
        assert_eq!(element.pseudos[0].props.len(), 1);
    }

    #[test]
    fn an_inline_breakpoint_does_not_swallow_the_child_block() {
        let program = ok("stack gap 1rem @tablet gap 2rem\n  text \"child\"\n");
        let element = first_element(&program);
        assert_eq!(element.responsive.len(), 1);
        assert_eq!(element.responsive[0].name, "tablet");
        assert_eq!(element.responsive[0].query, "(min-width: 640px)");
        assert_eq!(element.children.len(), 1, "the indented line is still a child");
    }

    #[test]
    fn a_breakpoint_block_below_its_element_owns_its_indentation() {
        let program = ok("stack\n  @desktop\n    columns 4\n    when hover\n      background red\n");
        let element = first_element(&program);
        assert_eq!(element.responsive.len(), 1);
        assert_eq!(element.responsive[0].props.len(), 1);
        assert_eq!(element.responsive[0].pseudos.len(), 1);
        assert!(element.children.is_empty());
    }

    #[test]
    fn a_bare_pseudo_state_is_reported() {
        let (_, diagnostics) = parse("button \"x\"\n  hover\n    background red\n");
        assert!(codes(&diagnostics).contains(&"E144"));
    }

    #[test]
    fn an_unknown_pseudo_state_is_reported() {
        let (_, diagnostics) = parse("button \"x\"\n  when hovered\n    background red\n");
        assert!(codes(&diagnostics).contains(&"E157"));
        assert!(diagnostics[0].suggestion.as_deref().unwrap().contains("when hover"));
    }

    #[test]
    fn media_features_resolve_to_a_query() {
        let program = ok("box\n  @reduced-motion\n    transition none\n");
        let element = first_element(&program);
        assert_eq!(element.responsive[0].query, "(prefers-reduced-motion: reduce)");
    }

    #[test]
    fn an_unknown_breakpoint_is_left_for_the_semantic_pass() {
        // `theme` may declare it, and the theme may appear later in the file.
        let program = ok("box @huge columns 3\n");
        let element = first_element(&program);
        assert_eq!(element.responsive[0].name, "huge");
        assert_eq!(element.responsive[0].query, "");
    }

    // -- events and binding ------------------------------------------------

    #[test]
    fn handlers_accept_an_arrow_body_and_a_block_body() {
        let program = ok("button \"x\"\n  on click => count.set(count() + 1)\n  on keydown.enter => go()\n");
        let element = first_element(&program);
        assert_eq!(element.handlers.len(), 2);
        assert_eq!(element.handlers[0].event, "click");
        assert_eq!(element.handlers[0].body.len(), 1);
        assert_eq!(element.handlers[1].keys, vec!["enter".to_string()]);

        let program = ok("button \"x\"\n  on click\n    count.set(1)\n");
        let element = first_element(&program);
        assert_eq!(element.handlers[0].body.len(), 1);
    }

    #[test]
    fn handler_behaviour_modifiers_parse() {
        let program = ok("form\n  on submit.prevent.stop => go()\n");
        let element = first_element(&program);
        assert_eq!(element.handlers[0].modifiers, vec!["prevent".to_string(), "stop".to_string()]);
    }

    #[test]
    fn a_key_filter_on_a_mouse_event_is_reported() {
        let (_, diagnostics) = parse("button \"x\"\n  on click.right => menu()\n");
        assert!(codes(&diagnostics).contains(&"E151"));
        assert!(diagnostics[0].suggestion.as_deref().unwrap().contains("contextmenu"));
    }

    #[test]
    fn an_unknown_event_is_reported() {
        let (_, diagnostics) = parse("button \"x\"\n  on tap => go()\n");
        assert!(codes(&diagnostics).contains(&"E150"));
    }

    #[test]
    fn bind_targets_a_path() {
        let program = ok("input bind form.email\n");
        let element = first_element(&program);
        let bind = element.bind.as_ref().expect("a bind target");
        assert_eq!(bind.path.root, "form");
        assert_eq!(bind.path.steps.len(), 1);
    }

    #[test]
    fn bind_on_a_non_input_element_is_reported() {
        let (_, diagnostics) = parse("box bind value\n");
        assert!(codes(&diagnostics).contains(&"E142"));
    }

    #[test]
    fn show_takes_a_condition() {
        let program = ok("box show open\n");
        let element = first_element(&program);
        assert_eq!(text_of(element.show.as_ref().unwrap()), "open");
    }

    // -- declarations ------------------------------------------------------

    #[test]
    fn let_derive_and_persist_parse() {
        let program = ok("let count = 0\nlet name = \"a\" persist \"user\"\nderive double = count * 2\n");
        match &program.body[0] {
            Node::Let(decl) => {
                assert_eq!(decl.name, "count");
                assert!(decl.persist.is_none());
            }
            other => panic!("expected a let, got {other:?}"),
        }
        match &program.body[1] {
            Node::Let(decl) => {
                let persist = decl.persist.as_ref().expect("a persist suffix");
                assert_eq!(text_of(&persist.key), "user");
                assert!(!persist.session);
            }
            other => panic!("expected a let, got {other:?}"),
        }
        assert!(matches!(&program.body[2], Node::Derive(decl) if decl.name == "double"));
    }

    #[test]
    fn a_standalone_persist_points_at_the_let_form() {
        let (_, diagnostics) = parse("persist count\n");
        assert!(codes(&diagnostics).contains(&"E187"));
    }

    #[test]
    fn type_annotations_parse_into_the_right_shape() {
        let program = ok("let a: string = \"\"\nlet b: list(todo) = []\nlet c: string? = none\n");
        let types: Vec<String> = program
            .body
            .iter()
            .map(|node| match node {
                Node::Let(decl) => decl.ty.as_ref().map(TypeRef::describe).unwrap_or_default(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(types, vec!["string", "[todo]", "string?"]);
    }

    #[test]
    fn functions_take_typed_parameters_with_defaults() {
        let program = ok("fn add(a: number, b = 2)\n  return a + b\n");
        match &program.body[0] {
            Node::Fn(decl) => {
                assert_eq!(decl.name, "add");
                assert_eq!(decl.params.len(), 2);
                assert!(decl.params[0].ty.is_some());
                assert!(decl.params[1].default.is_some());
                assert_eq!(decl.body.len(), 1);
            }
            other => panic!("expected a function, got {other:?}"),
        }
    }

    #[test]
    fn if_chains_parse() {
        let program = ok("if a > 1\n  text \"big\"\nelse if a > 0\n  text \"small\"\nelse\n  text \"none\"\n");
        match &program.body[0] {
            Node::If(branch) => {
                assert_eq!(branch.branches.len(), 2);
                assert!(branch.else_branch.is_some());
                assert_eq!(branch.branches[1].1.len(), 1);
            }
            other => panic!("expected an if, got {other:?}"),
        }
    }

    #[test]
    fn if_inside_an_element_renders_ui() {
        let program = ok("box\n  if open\n    text \"hi\"\n");
        let element = first_element(&program);
        assert!(matches!(&element.children[0], Node::If(_)));
    }

    #[test]
    fn each_parses_index_and_key() {
        let program = ok("each todo, i in todos key todo.id\n  text todo.title\n");
        match &program.body[0] {
            Node::Each(each) => {
                assert_eq!(each.item, "todo");
                assert_eq!(each.index.as_deref(), Some("i"));
                assert_eq!(text_of(each.key.as_ref().unwrap()), "todo.id");
                assert_eq!(text_of(&each.iter), "todos");
                assert_eq!(each.body.len(), 1);
            }
            other => panic!("expected an each, got {other:?}"),
        }
    }

    #[test]
    fn each_over_a_range_parses() {
        let program = ok("each i in 1..5\n  text \"~(i)\"\n");
        match &program.body[0] {
            Node::Each(each) => assert_eq!(text_of(&each.iter), "1..5"),
            other => panic!("expected an each, got {other:?}"),
        }
    }

    #[test]
    fn each_without_in_is_reported() {
        let (_, diagnostics) = parse("each todo todos\n");
        assert!(codes(&diagnostics).contains(&"E184"));
    }

    #[test]
    fn a_missing_indented_block_is_reported() {
        assert!(codes(&parse("if a\n").1).contains(&"E182"));
        assert!(codes(&parse("each a in b\n").1).contains(&"E180"));
        assert!(codes(&parse("fn f()\n").1).contains(&"E181"));
    }

    // -- components --------------------------------------------------------

    #[test]
    fn components_declare_props_in_parentheses() {
        let program = ok("component Card(title: string, count = 0)\n  text \"~(title)\"\n");
        let component = &program.components[0];
        assert_eq!(component.name, "Card");
        assert_eq!(component.props.len(), 2);
        assert!(component.props[0].ty.is_some());
        assert!(component.props[1].default.is_some());
        assert_eq!(component.body.len(), 1);
    }

    #[test]
    fn components_declare_props_in_a_block() {
        let program = ok("component Card\n  title: string\n  count = 0\n  children\n  slot footer\n  text \"body\"\n");
        let component = &program.components[0];
        assert_eq!(component.props.len(), 2);
        assert!(matches!(&component.body[0], Node::Slot(slot) if slot.name.is_none()));
        assert!(matches!(&component.body[1], Node::Slot(slot) if slot.name.as_deref() == Some("footer")));
        assert!(matches!(&component.body[2], Node::Element(_)));
    }

    #[test]
    fn a_lowercase_component_name_is_reported() {
        let (_, diagnostics) = parse("component card\n  text \"x\"\n");
        assert!(codes(&diagnostics).contains(&"E111"));
    }

    #[test]
    fn component_uses_accept_both_prop_forms() {
        let program = ok("Card(title: \"Hi\", count: 3)\nCard\n  title \"Hi\"\n");
        match &program.body[0] {
            Node::Use(used) => {
                assert_eq!(used.props.len(), 2);
                assert_eq!(used.props[0].1, "title");
                assert_eq!(text_of(&used.props[0].2), "Hi");
            }
            other => panic!("expected a use, got {other:?}"),
        }
        match &program.body[1] {
            Node::Use(used) => {
                assert_eq!(used.props.len(), 1);
                assert_eq!(used.props[0].1, "title");
            }
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn a_component_use_can_be_styled_and_filled() {
        let program = ok("Card(title: \"Hi\")\n  gap 1rem\n  text \"body\"\n  slot header\n    heading \"H\"\n");
        match &program.body[0] {
            Node::Use(used) => {
                assert_eq!(used.props_style.len(), 1);
                assert_eq!(used.children.len(), 1);
                assert_eq!(used.slots.len(), 1);
                assert_eq!(used.slots[0].0, "header");
                assert_eq!(used.slots[0].1.len(), 1);
            }
            other => panic!("expected a use, got {other:?}"),
        }
    }

    #[test]
    fn a_prop_with_no_value_is_reported() {
        let (_, diagnostics) = parse("Card\n  title\n");
        assert!(codes(&diagnostics).contains(&"E185"));
    }

    // -- theme -------------------------------------------------------------

    #[test]
    fn theme_groups_and_dark_overrides_parse() {
        let program = ok("theme\n  colors\n    primary #5dade2\n    dark\n      primary #2e86c1\n  spacing\n    md 1rem\n");
        let theme = program.theme.as_ref().expect("a theme");
        assert_eq!(theme.groups.len(), 2);
        assert_eq!(theme.groups[0].name, "colors");
        assert_eq!(theme.groups[0].entries.len(), 1);
        assert_eq!(text_of(&theme.groups[0].entries[0].values[0]), "#5dade2");
        assert_eq!(theme.groups[0].dark.len(), 1);
        assert_eq!(theme.groups[1].name, "spacing");
    }

    #[test]
    fn a_theme_token_is_not_parsed_as_a_group() {
        let program = ok("theme\n  spacing\n    md 1rem\n");
        let theme = program.theme.as_ref().expect("a theme");
        assert_eq!(theme.groups.len(), 1, "`md 1rem` is an entry, not a group");
        assert_eq!(theme.groups[0].entries[0].name, "md");
    }

    #[test]
    fn custom_breakpoints_become_media_queries() {
        let program = ok("theme\n  breakpoints\n    huge 1800px\n");
        let theme = program.theme.as_ref().expect("a theme");
        let group = &theme.groups[0];
        assert_eq!(group.breakpoints.len(), 1);
        assert_eq!(group.breakpoints[0].0, "huge");
        assert_eq!(group.breakpoints[0].1, "(min-width: 1800px)");
    }

    #[test]
    fn an_unknown_theme_category_is_reported() {
        let (_, diagnostics) = parse("theme\n  colours\n    primary red\n");
        assert!(codes(&diagnostics).contains(&"E117"));
    }

    #[test]
    fn only_one_theme_per_file() {
        let (_, diagnostics) = parse("theme\n  colors\n    a red\ntheme\n  colors\n    b blue\n");
        assert!(codes(&diagnostics).contains(&"E112"));
    }

    // -- expressions -------------------------------------------------------

    fn let_value(program: &Program, index: usize) -> String {
        match &program.body[index] {
            Node::Let(decl) => text_of(&decl.value),
            other => panic!("expected a let, got {other:?}"),
        }
    }

    #[test]
    fn precedence_follows_the_usual_order() {
        let program = ok("let a = 1 + 2 * 3\nlet b = (1 + 2) * 3\n");
        assert_eq!(let_value(&program, 0), "1 + 2 * 3");
        assert_eq!(let_value(&program, 1), "1 + 2 * 3");
        assert!(matches!(
            match &program.body[0] { Node::Let(d) => &*d.value, _ => unreachable!() },
            Expr::Binary(op, _, _, _) if op == "+"
        ));
        assert!(matches!(
            match &program.body[1] { Node::Let(d) => &*d.value, _ => unreachable!() },
            Expr::Binary(op, _, _, _) if op == "*"
        ));
    }

    #[test]
    fn comparison_and_logic_operators_parse() {
        let program = ok("let a = x == 1\nlet b = x != 1 and y <= 2 or not z\n");
        assert_eq!(let_value(&program, 0), "x == 1");
        assert_eq!(let_value(&program, 1), "x != 1 and y <= 2 or not z");
    }

    #[test]
    fn unary_minus_attaches_to_numbers_only() {
        let program = ok("let a = -3\nlet b = 5 - 3\n");
        assert_eq!(let_value(&program, 0), "- 3");
        assert_eq!(let_value(&program, 1), "5 - 3");
    }

    #[test]
    fn pipes_chain_left_to_right() {
        let program = ok("let a = items | filter(it => it.done) | count()\n");
        assert_eq!(let_value(&program, 0), "items | filter(it => it.done) | count()");
    }

    #[test]
    fn lambdas_come_in_three_shapes() {
        let program = ok("let a = it => it.name\nlet b = (x, y) => x + y\nlet c = () => 1\n");
        assert_eq!(let_value(&program, 0), "it => it.name");
        assert_eq!(let_value(&program, 1), "x, y => x + y");
        assert_eq!(let_value(&program, 2), " => 1");
    }

    #[test]
    fn records_lists_and_spreads_parse() {
        let program = ok("let a = record(name: \"Alice\", age: 30)\nlet b = [1, 2, ...rest]\n");
        assert_eq!(let_value(&program, 0), "name: Alice, age: 30");
        assert_eq!(let_value(&program, 1), "1, 2, ...rest");
    }

    #[test]
    fn member_index_and_method_access_parse() {
        let program = ok("let a = user.name\nlet b = items[0]\nlet c = items.map(it => it.id)\n");
        assert_eq!(let_value(&program, 0), "user.name");
        assert_eq!(let_value(&program, 1), "items[0]");
        assert_eq!(let_value(&program, 2), "items.map(it => it.id)");
    }

    #[test]
    fn an_expression_if_produces_a_value() {
        let program = ok("let a = if n > 0 then \"pos\" else \"neg\"\n");
        assert_eq!(let_value(&program, 0), "if n > 0 then pos else neg");
    }

    #[test]
    fn an_expression_if_without_then_is_reported() {
        let (_, diagnostics) = parse("let a = if n > 0 \"pos\" else \"neg\"\n");
        assert!(codes(&diagnostics).contains(&"E125"));
    }

    #[test]
    fn lengths_colours_and_numbers_keep_their_kind() {
        let program = ok("let a = 12px\nlet b = 1.5rem\nlet c = #4f46e5\nlet d = 100%\n");
        assert_eq!(let_value(&program, 0), "12px");
        assert_eq!(let_value(&program, 1), "1.5rem");
        assert_eq!(let_value(&program, 2), "#4f46e5");
        assert_eq!(let_value(&program, 3), "100%");
    }

    #[test]
    fn interpolation_builds_a_string_with_expressions() {
        let program = ok("text \"Count: ~(count + 1) of ~(total)\"\n");
        let element = first_element(&program);
        match &element.args[0] {
            Expr::Str(parts, _) => {
                assert_eq!(parts.len(), 4);
                assert!(matches!(&parts[0], StrPart::Text(text) if text == "Count: "));
                assert!(matches!(&parts[1], StrPart::Expr(_)));
            }
            other => panic!("expected a string, got {other:?}"),
        }
        assert_eq!(text_of(&element.args[0]), "Count: ~() of ~()");
    }

    #[test]
    fn an_unterminated_interpolation_is_reported() {
        let (_, diagnostics) = parse("text \"~(count\"\n");
        assert!(!diagnostics.is_empty());
    }

    #[test]
    fn assignment_targets_can_be_paths() {
        let program = ok("form.email = \"a\"\nitems[0].done = true\n");
        match &program.body[0] {
            Node::Assign(assign) => {
                assert_eq!(assign.target.root, "form");
                assert_eq!(assign.target.steps.len(), 1);
                assert_eq!(text_of(&assign.value), "a");
            }
            other => panic!("expected an assignment, got {other:?}"),
        }
        match &program.body[1] {
            Node::Assign(assign) => assert_eq!(assign.target.steps.len(), 2),
            other => panic!("expected an assignment, got {other:?}"),
        }
    }

    #[test]
    fn a_property_named_like_a_variable_is_still_an_assignment() {
        // `color = theme` must not read as a style property called `color`.
        let program = ok("color = theme\n");
        assert!(matches!(&program.body[0], Node::Assign(_)));
    }

    // -- errors and recovery -----------------------------------------------

    #[test]
    fn curly_braces_and_semicolons_are_rejected_by_the_lexer() {
        assert!(codes(&parse("box { text \"x\" }\n").1).contains(&"E006"));
        assert!(codes(&parse("text \"x\";\n").1).contains(&"E007"));
        assert!(codes(&parse("<div>x</div>\n").1).contains(&"E008"));
    }

    #[test]
    fn deferred_keywords_report_the_roadmap_not_a_syntax_error() {
        for keyword in ["route", "fetch", "validate", "match", "import", "global"] {
            let (program, diagnostics) = parse(&format!("{keyword} \"/x\"\n"));
            assert!(codes(&diagnostics).contains(&"E900"), "{keyword} should be deferred");
            assert_eq!(program.deferred.len(), 1);
            assert_eq!(program.deferred[0].keyword, keyword);
        }
    }

    #[test]
    fn a_type_alias_is_deferred_but_the_type_attribute_is_not() {
        assert!(codes(&parse("type User = record(name: string)\n").1).contains(&"E900"));
        let program = ok("input type email\n");
        assert_eq!(first_element(&program).attrs[0].name, "type");
    }

    #[test]
    fn ui_inside_a_handler_is_reported() {
        let (_, diagnostics) = parse("button \"x\"\n  on click\n    text \"hi\"\n");
        assert!(codes(&diagnostics).contains(&"E183"));
    }

    #[test]
    fn a_style_property_inside_an_if_is_reported() {
        let (_, diagnostics) = parse("box\n  if open\n    color red\n");
        assert!(codes(&diagnostics).contains(&"E178"));
    }

    #[test]
    fn a_reserved_word_cannot_be_a_variable_name() {
        let (_, diagnostics) = parse("let each = 1\n");
        assert!(codes(&diagnostics).contains(&"E190"));
    }

    #[test]
    fn trailing_junk_on_a_line_is_reported() {
        let (_, diagnostics) = parse("let a = 1 2\n");
        assert!(codes(&diagnostics).contains(&"E104"));
    }

    #[test]
    fn recovery_terminates_on_garbage_without_panicking() {
        // The parser runs inside `plain dev` and the language server, so it must
        // never loop or panic on half-typed input.
        for source in [
            ")))",
            "@ @ @",
            "box\n    text \"a\"\n  text \"b\"\n",
            "let = = =",
            "text \"unterminated\n",
            "component\n",
            "on\n",
            "each\n",
            ".. .. ...",
            "1 2 3",
            "box\n\ttext \"tab\"\n",
            "when\n",
            "theme\n",
            "Card(\n",
            "text \"a\" \"~(\"\n",
            "fn\n",
            "slot\n",
            "bind\n",
            "show\n",
            "if\n",
            "derive\n",
            "let a: = 1\n",
            "record(\n",
            "[1, 2\n",
            "a.\n",
            "a[\n",
            "=> 1\n",
            ": string\n",
        ] {
            let (_, diagnostics) = parse(source);
            assert!(!diagnostics.is_empty(), "expected an error for {source:?}");
        }
    }

    #[test]
    fn a_full_small_app_parses_without_errors() {
        let program = ok(
            "theme
  colors
    primary #5dade2
    dark
      primary #2e86c1
  spacing
    md 1rem

let count = 0 persist \"count\"
derive label = if count == 1 then \"click\" else \"clicks\"

fn reset()
  count = 0

component Counter(title: string)
  stack align center gap 2
    heading \"~(title): ~(count) ~(label)\"
      level 2
    row gap 1rem
      button \"Add one\"
        fill primary
        size lg
        radius md
        when hover
          background accent
        on click => count.set(count() + 1)
      button \"Reset\"
        on click => reset()

main
  section
    Counter(title: \"Clicks\")
      gap 1rem @tablet gap 2rem
    list
      each item, index in items key item.id
        item \"~(index). ~(item.name)\"
          show item.visible
    form
      input bind draft placeholder \"Search\"
        label \"Search the list\"
      check bind exact label \"Match whole words\"
      button \"Go\"
        type submit
        on submit.prevent => search()
",
        );
        assert_eq!(program.components.len(), 1);
        assert!(program.theme.is_some());
        assert_eq!(program.body.len(), 4);
        assert!(program.deferred.is_empty());
    }
}
