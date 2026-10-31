//! The semantic pass.
//!
//! The parser knows what the words mean; this pass knows what the program
//! means. It answers the questions that need the whole file — is that name
//! declared, does that component exist, does that control have a name a screen
//! reader can read — and it decides one thing the generators need: which values
//! are not known until the program runs.
//!
//! That last job is why [`check`] runs before [`css::collect`](crate::css::collect).
//! `color primary` is a theme token and belongs in the stylesheet; `color brand`
//! where `brand` is a `let` is a value that changes, and belongs in an inline
//! style effect. The rule is one line and it decides both: **a bare word is a
//! variable if and only if it is declared.**
//!
//! The pass never panics and never stops at the first problem. A file with ten
//! mistakes reports ten mistakes, because fixing them one build at a time is
//! how a compiler teaches someone to dislike it.

use std::collections::{HashMap, HashSet};

use plain_core::{Diagnostic, FileId, Span};

use crate::ast::{
    Assign, Component, Derive, Each, Element, Expr, FnDecl, Handler, If, Let, Node, Program,
    Return, Slot, StyleProp, Use,
};
use crate::expr::{self, Binding, Scope, NAMESPACES};
use crate::tables::{self, ArgRole};

/// JavaScript a Plain program may reach. v0.1 has no `import`, and these are the
/// honest escape hatches; anything else that is not declared is a mistake, and
/// saying so at build time is cheaper than a blank page at run time.
const GLOBALS: &[&str] = &[
    "AbortController",
    "Array",
    "Blob",
    "Boolean",
    "CustomEvent",
    "Date",
    "Element",
    "Error",
    "Event",
    "File",
    "FormData",
    "HTMLElement",
    "Headers",
    "Infinity",
    "Intl",
    "IntersectionObserver",
    "JSON",
    "KeyboardEvent",
    "Map",
    "Math",
    "MouseEvent",
    "MutationObserver",
    "NaN",
    "Node",
    "Number",
    "Object",
    "Promise",
    "RegExp",
    "Request",
    "ResizeObserver",
    "Response",
    "Set",
    "String",
    "Symbol",
    "TextDecoder",
    "TextEncoder",
    "URL",
    "URLSearchParams",
    "WeakMap",
    "WeakSet",
    "alert",
    "atob",
    "btoa",
    "cancelAnimationFrame",
    "clearInterval",
    "clearTimeout",
    "confirm",
    "console",
    "crypto",
    "decodeURI",
    "decodeURIComponent",
    "document",
    "encodeURI",
    "encodeURIComponent",
    "event",
    "getComputedStyle",
    "globalThis",
    "history",
    "isFinite",
    "isNaN",
    "localStorage",
    "location",
    "matchMedia",
    "navigator",
    "parseFloat",
    "parseInt",
    "performance",
    "prompt",
    "queueMicrotask",
    "requestAnimationFrame",
    "screen",
    "sessionStorage",
    "setInterval",
    "setTimeout",
    "structuredClone",
    "undefined",
    "window",
];

/// Elements that are only valid inside another element. Getting this wrong does
/// not produce a diagnostic in a browser; it produces a table that is not a
/// table, silently.
const NESTING: &[(&str, &[&str])] = &[
    ("item", &["list", "numbered-list"]),
    ("option", &["select"]),
    ("table-row", &["table"]),
    ("table-cell", &["table-row"]),
    ("summary", &["details"]),
    ("caption", &["figure"]),
];

/// Attributes that give a control an accessible name.
const NAME_ATTRIBUTES: &[&str] = &["label", "aria-label", "aria-labelledby", "alt"];

/// What a component declares, copied out of the AST so the checker can hold it
/// while the program is being modified.
#[derive(Clone)]
struct ComponentInfo {
    props: Vec<PropInfo>,
    /// The named slots the body reads, so a caller can be told when it fills one
    /// that nothing will ever render.
    slots: Vec<String>,
}

#[derive(Clone)]
struct PropInfo {
    name: String,
    /// True when the declaration supplies a default, which is what makes a prop
    /// optional.
    optional: bool,
}

/// Walks a program and collects everything worth reporting.
struct Checker {
    file: FileId,
    diagnostics: Vec<Diagnostic>,
    components: HashMap<String, ComponentInfo>,
    scope: Scope,
    /// Names declared in the block being walked, so a duplicate is caught where
    /// it happens rather than as a confusing redefinition later.
    declared: Vec<HashSet<String>>,
    /// `derive` names, which may be read but not assigned.
    computed: HashSet<String>,
    ids: HashSet<String>,
    /// Where each top-level heading is, so the report about there being several
    /// can point at the second one rather than at the whole file.
    headings: Vec<Span>,
    /// The elements enclosing the one being checked, outermost first.
    parents: Vec<String>,
    /// True inside a component body, where `children` and `slot` mean something.
    in_component: bool,
}

/// Checks a program and marks every value that is not known until it runs.
///
/// Takes the program by mutable reference because the `dynamic` flags the
/// generators read are decided here.
pub fn check(program: &mut Program) -> Vec<Diagnostic> {
    let mut checker = Checker {
        file: program.file,
        diagnostics: Vec::new(),
        components: HashMap::new(),
        scope: Scope::new(),
        declared: vec![HashSet::new()],
        computed: HashSet::new(),
        ids: HashSet::new(),
        headings: Vec::new(),
        parents: Vec::new(),
        in_component: false,
    };

    // Every component is visible to every other, including itself, so the
    // declarations are gathered before any body is walked.
    for component in &program.components {
        checker.register(component);
    }
    for component in &mut program.components {
        checker.component(component);
    }
    checker.in_component = false;
    checker.reset_scope();
    checker.nodes(&mut program.body);
    checker.finish();

    // Diagnostics stay in the order they were found, which is the order the
    // file reads in. Sorting them by code would put the last mistake first.
    checker.diagnostics
}

impl Checker {
    fn reset_scope(&mut self) {
        self.scope = Scope::new();
        self.computed = HashSet::new();
        self.declared = vec![HashSet::new()];
    }

    fn register(&mut self, component: &Component) {
        if self.components.contains_key(&component.name) {
            self.error(
                "E206",
                format!("`{}` is declared twice", component.name),
                component.span,
            )
            .note("Two components cannot share a name, because an invocation would not know which one it means.")
            .suggest("Rename one of them.");
            return;
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut props = Vec::new();
        for prop in &component.props {
            if !seen.insert(prop.name.clone()) {
                self.error(
                    "E206",
                    format!("`{}` declares `{}` twice", component.name, prop.name),
                    prop.span,
                )
                .suggest("A prop is looked up by name, so two of them cannot both be reached.");
            }
            props.push(PropInfo {
                name: prop.name.clone(),
                optional: prop.default.is_some(),
            });
        }
        let mut slots = Vec::new();
        collect_slots(&component.body, &mut slots);
        self.components
            .insert(component.name.clone(), ComponentInfo { props, slots });
    }

    fn component(&mut self, component: &mut Component) {
        self.reset_scope();
        self.in_component = true;
        self.parents.clear();
        for prop in &component.props {
            self.declare_name(&prop.name, Binding::Accessor, prop.span);
            if let Some(default) = &prop.default {
                self.expression(default);
            }
        }
        self.nodes(&mut component.body);
        self.in_component = false;
    }

    fn finish(&mut self) {
        if self.headings.len() > 1 {
            let second = self.headings[1];
            // Not a fatal problem, and not one the writer can always avoid, so it
            // is a warning: a page with several `h1`s is harder to skim with a
            // screen reader's heading list, but it still works.
            self.warning(
                "W101",
                format!(
                    "this page has {} `heading`s at level 1",
                    self.headings.len()
                ),
                second,
            )
            .note("A page usually wants one top-level heading. Assistive technology lists headings, and several at the same level make that list harder to skim.")
            .suggest("Give the others a level: `heading \"About\"` and then `level 2`.");
        }
    }

    // --- diagnostics ------------------------------------------------------

    fn error(&mut self, code: &str, title: impl Into<String>, span: Span) -> &mut Self {
        self.diagnostics
            .push(Diagnostic::error(code, title, self.file).with_label(span, String::new()));
        self
    }

    fn warning(&mut self, code: &str, title: impl Into<String>, span: Span) -> &mut Self {
        self.diagnostics
            .push(Diagnostic::warning(code, title, self.file).with_label(span, String::new()));
        self
    }

    /// Adds an explanatory paragraph to the diagnostic just pushed.
    fn note(&mut self, text: impl Into<String>) -> &mut Self {
        if let Some(last) = self.diagnostics.last_mut() {
            last.notes.push(text.into());
        }
        self
    }

    /// Adds a concrete fix to the diagnostic just pushed.
    fn suggest(&mut self, text: impl Into<String>) -> &mut Self {
        if let Some(last) = self.diagnostics.last_mut() {
            last.suggestion = Some(text.into());
        }
        self
    }

    // --- names ------------------------------------------------------------

    fn declare_name(&mut self, name: &str, binding: Binding, span: Span) {
        let top = self.declared.last_mut().expect("a block is always open");
        if !top.insert(name.to_string()) {
            self.error(
                "E206",
                format!("`{name}` is declared twice in the same block"),
                span,
            )
            .note("The second declaration would hide the first, and one of them is a mistake.")
            .suggest("Rename one, or assign to the existing name instead of declaring a new one.");
        }
        self.scope.bind(name, binding);
    }

    fn known(&self, name: &str) -> bool {
        self.scope.contains(name)
            || tables::builtin(name).is_some()
            || NAMESPACES.contains(&name)
            || GLOBALS.contains(&name)
    }

    /// The nearest declared name or built-in, close enough to be worth offering.
    fn closest(&self, name: &str) -> Option<String> {
        let letters: Vec<char> = name.chars().collect();
        let mut candidates: Vec<String> =
            self.scope.names().iter().map(|n| n.to_string()).collect();
        candidates.extend(tables::BUILTINS.iter().map(|(plain, _)| plain.to_string()));
        closest_of(name, &candidates).map(|found| found.to_string())
    }

    // --- nodes ------------------------------------------------------------

    fn nodes(&mut self, nodes: &mut [Node]) {
        // Declarations are gathered first, so a handler written above the `let`
        // it uses still resolves — and so the duplicate check sees the whole
        // block at once.
        self.declared.push(HashSet::new());
        for node in nodes.iter() {
            match node {
                Node::Let(declaration) => {
                    self.declare_name(&declaration.name, Binding::Accessor, declaration.span)
                }
                Node::Derive(declaration) => {
                    self.declare_name(&declaration.name, Binding::Accessor, declaration.span);
                    self.computed.insert(declaration.name.clone());
                }
                Node::Fn(function) => {
                    self.declare_name(&function.name, Binding::Function, function.span)
                }
                _ => {}
            }
        }
        for node in nodes.iter_mut() {
            self.node(node);
        }
        self.declared.pop();
    }

    fn node(&mut self, node: &mut Node) {
        match node {
            Node::Let(declaration) => self.let_declaration(declaration),
            Node::Derive(declaration) => self.derive_declaration(declaration),
            Node::Fn(function) => self.function(function),
            Node::If(condition) => self.condition(condition),
            Node::Each(each) => self.each(each),
            Node::Assign(assignment) => self.assignment(assignment),
            Node::Expr(value) => self.expression(value),
            Node::Return(returned) => self.return_statement(returned),
            Node::Slot(slot) => self.slot(slot),
            Node::Element(element) => self.element(element),
            Node::Use(invocation) => self.invocation(invocation),
            Node::Deferred(_) => {}
        }
    }

    fn let_declaration(&mut self, declaration: &mut Let) {
        self.expression(&declaration.value);
        if let Some(persist) = &declaration.persist {
            match &persist.key {
                Expr::Str(parts, _)
                    if parts
                        .iter()
                        .all(|part| matches!(part, crate::ast::StrPart::Text(_))) => {}
                other => {
                    let span = other.span();
                    self.error(
                        "E214",
                        "a storage key has to be a literal string".to_string(),
                        span,
                    )
                    .note("The key is how the value is found again on the next visit, so it cannot depend on anything that changes.")
                    .suggest("Write `persist \"todos\"`.");
                }
            }
        }
    }

    fn derive_declaration(&mut self, declaration: &mut Derive) {
        self.expression(&declaration.value);
    }

    fn function(&mut self, function: &mut FnDecl) {
        self.declared.push(HashSet::new());
        for param in &function.params {
            self.declare_name(&param.name, Binding::Value, param.span);
            if let Some(default) = &param.default {
                self.expression(default);
            }
        }
        self.nodes(&mut function.body);
        self.declared.pop();
    }

    fn condition(&mut self, condition: &mut If) {
        for (test, body) in condition.branches.iter_mut() {
            self.expression(test);
            self.nodes(body);
        }
        if let Some(body) = &mut condition.else_branch {
            self.nodes(body);
        }
    }

    fn each(&mut self, each: &mut Each) {
        self.expression(&each.iter);
        self.declared.push(HashSet::new());
        self.declare_name(&each.item, Binding::Accessor, each.span);
        if let Some(index) = &each.index {
            self.declare_name(index, Binding::Accessor, each.span);
        }
        self.nodes(&mut each.body);
        self.declared.pop();
        // The key is read from the raw item, not the row's accessor.
        if let Some(key) = &mut each.key {
            self.declared.push(HashSet::new());
            self.declare_name(&each.item, Binding::Value, each.span);
            if let Some(index) = &each.index {
                self.declare_name(index, Binding::Value, each.span);
            }
            self.expression(key);
            self.declared.pop();
        }
    }

    fn assignment(&mut self, assignment: &mut Assign) {
        self.expression(&assignment.value);
        let root = &assignment.target.root;
        if self.computed.contains(root) {
            self.error(
                "E205",
                format!("`{root}` is derived, so it cannot be assigned"),
                assignment.span,
            )
            .note("A `derive` is worked out from other state. Writing to it would be overwritten the moment any of that state changed.")
            .suggest("Assign to the state it is derived from, or make it a `let`.");
            return;
        }
        match self.scope.binding(root) {
            Some(Binding::Accessor) => {}
            Some(_) => {
                self.error(
                    "E205",
                    format!("`{root}` is not state, so it cannot be assigned"),
                    assignment.span,
                )
                .note("Only a `let` holds a value that can be written.");
            }
            None if !self.known(root) => self.unknown_name(root, assignment.span),
            None => {}
        }
        for step in &assignment.target.steps {
            if let crate::ast::PathStep::Index(index, _) = step {
                self.expression(index);
            }
        }
    }

    fn return_statement(&mut self, returned: &mut Return) {
        if let Some(value) = &mut returned.value {
            self.expression(value);
        }
    }

    fn slot(&mut self, slot: &Slot) {
        if !self.in_component {
            self.error(
                "E211",
                match &slot.name {
                    Some(name) => format!("`slot {name}` outside a component"),
                    None => "`children` outside a component".to_string(),
                },
                slot.span,
            )
            .note("A slot is the hole a caller fills. Only a component has callers.")
            .suggest("Put it inside a `component`, or write the elements directly.");
        }
    }

    // --- expressions ------------------------------------------------------

    /// Walks an expression, reporting names nothing declares.
    fn expression(&mut self, value: &Expr) {
        match value {
            Expr::Name(name, span) => {
                if !self.known(name) {
                    self.unknown_name(name, *span);
                }
            }
            Expr::Member(target, _, _) | Expr::Spread(target, _) | Expr::Unary(_, target, _) => {
                self.expression(target)
            }
            Expr::Index(target, index, _)
            | Expr::Binary(_, target, index, _)
            | Expr::Pipe(target, index, _)
            | Expr::Range(target, index, _) => {
                self.expression(target);
                self.expression(index);
            }
            Expr::Conditional(test, then, otherwise, _) => {
                self.expression(test);
                self.expression(then);
                self.expression(otherwise);
            }
            Expr::Call(callee, args, _) => {
                self.expression(callee);
                for arg in args {
                    self.expression(arg);
                }
            }
            Expr::Method(target, _, args, _) => {
                self.expression(target);
                for arg in args {
                    self.expression(arg);
                }
            }
            Expr::List(items, _) => {
                for item in items {
                    self.expression(item);
                }
            }
            Expr::Record(fields, _) => {
                // A field name is a key, not a name to resolve.
                for (_, value) in fields {
                    self.expression(value);
                }
            }
            Expr::Str(parts, _) => {
                for part in parts {
                    if let crate::ast::StrPart::Expr(value) = part {
                        self.expression(value);
                    }
                }
            }
            Expr::Lambda(params, body, _) => {
                self.declared.push(HashSet::new());
                for param in params {
                    self.declare_name(param, Binding::Value, body.span());
                }
                self.expression(body);
                self.declared.pop();
            }
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Length(..)
            | Expr::Color(..)
            | Expr::Bool(..)
            | Expr::None(_) => {}
        }
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        self.error("E210", format!("`{name}` is not declared"), span);
        if let Some(candidate) = self.closest(name) {
            self.suggest(format!("Did you mean `{candidate}`?"));
        }
        self.note("A bare word is a variable when something declares it, and a keyword, a theme token or a CSS value when nothing does.")
            .note("Declare it with `let`, or put it in quotes if you meant the word itself.");
    }

    // --- elements ---------------------------------------------------------

    fn element(&mut self, element: &mut Element) {
        for arg in element.args.iter_mut() {
            self.expression(arg);
        }
        if element.def.role == ArgRole::None && !element.args.is_empty() {
            let span = element.args[0].span();
            self.error(
                "E209",
                format!("`{}` does not take text", element.name),
                span,
            )
            .note("A container holds elements; the words belong in one of them.")
            .suggest(format!(
                "Put the text in a child: `{}\\n  text \\\"…\\\"`.",
                element.name
            ));
        }

        self.attributes(element);
        self.accessible_name(element);
        self.nesting(element);

        for prop in element.props.iter_mut() {
            self.style(prop);
        }
        for handler in element.handlers.iter_mut() {
            self.handler(handler);
        }
        if let Some(target) = &element.bind {
            self.binding(&element.name, &target.path.root, target.span);
        }
        if let Some(condition) = &element.show {
            self.expression(condition);
        }
        for block in element.pseudos.iter_mut() {
            for prop in block.props.iter_mut() {
                self.style(prop);
            }
        }
        for block in element.responsive.iter_mut() {
            for prop in block.props.iter_mut() {
                self.style(prop);
            }
        }

        self.parents.push(element.name.clone());
        self.nodes(&mut element.children);
        self.parents.pop();

        if element.def.tag == "h1" && level_of(element) == 1 {
            self.headings.push(element.span);
        }
    }

    fn attributes(&mut self, element: &mut Element) {
        for attribute in element.attrs.iter_mut() {
            for value in attribute.values.iter_mut() {
                // A bare word in an attribute is a keyword — `type email`,
                // `target _blank` — unless something declares it, in which case
                // it is that value.
                if let Expr::Name(name, _) = value {
                    if self.known(name) {
                        continue;
                    }
                } else {
                    self.expression(value);
                }
            }
            attribute.dynamic = attribute
                .values
                .iter()
                .any(|value| expr::is_dynamic(value, &self.scope));
            if attribute.name == "id" {
                if let Some(id) = literal_text(&attribute.values) {
                    if !id.is_empty() && !self.ids.insert(id.clone()) {
                        self.warning(
                            "W102",
                            format!("`{id}` is used as an `id` twice"),
                            attribute.span,
                        )
                        .note("An `id` is how a label, a skip link and `document.getElementById` find one particular element, so two of them make all three guess.")
                        .suggest("Give one of them a different id.");
                    }
                }
            }
        }
        if element.def.tag == "a" && !has_destination(element) {
            self.warning(
                "W103",
                "a `link` with nowhere to go".to_string(),
                element.span,
            )
            .note("An `<a>` without an `href` is not a link: it is not focusable and it is not announced as one.")
            .suggest("Give it a destination, or use a `button` if it acts on this page.");
        }
    }

    fn style(&mut self, prop: &mut StyleProp) {
        for value in prop.values.iter() {
            // A bare word here is a theme token or a CSS keyword unless
            // something declares it; `color red` is a colour and `color brand`
            // is a variable.
            if let Expr::Name(name, _) = value {
                if self.known(name) {
                    continue;
                }
            } else {
                self.expression(value);
            }
        }
        prop.dynamic = prop
            .values
            .iter()
            .any(|value| expr::is_dynamic(value, &self.scope));
    }

    fn handler(&mut self, handler: &mut Handler) {
        self.declared.push(HashSet::new());
        self.declare_name("event", Binding::Value, handler.span);
        self.nodes(&mut handler.body);
        self.declared.pop();
    }

    fn binding(&mut self, element: &str, root: &str, span: Span) {
        if self.computed.contains(root) {
            self.error(
                "E205",
                format!("`{root}` is derived, so `{element}` cannot bind to it"),
                span,
            )
            .note("Binding writes. A derived value is worked out from other state and would be overwritten at once.")
            .suggest("Bind to the `let` it is derived from.");
            return;
        }
        match self.scope.binding(root) {
            Some(Binding::Accessor) => {}
            Some(_) => {
                self.error(
                    "E207",
                    format!("`{root}` is not state, so `{element}` cannot bind to it"),
                    span,
                )
                .note("`bind` needs something to write to, and only a `let` is that.");
            }
            None if !self.known(root) => self.unknown_name(root, span),
            None => {}
        }
    }

    fn accessible_name(&mut self, element: &Element) {
        if !element.def.needs_name {
            return;
        }
        if element.modifiers.iter().any(|m| m.name == "decorative") {
            if element.name == "image" {
                return;
            }
            self.warning(
                "W104",
                format!("`decorative` on a `{}` hides it from assistive technology", element.name),
                element.span,
            )
            .note("`decorative` means the element carries no information. On a control that is rarely what was meant.")
            .suggest("Give it a `label` instead.");
        }
        if named_by_attribute(element) {
            return;
        }
        match element.def.role {
            ArgRole::Content => {
                if !element.args.is_empty() || text_in_children(&element.children) {
                    return;
                }
            }
            // `image "/cat.png" "A cat sleeping"`: the second argument is the
            // alternative text, which is the name.
            ArgRole::Src | ArgRole::Href => {
                if element.args.len() > 1 {
                    return;
                }
            }
            ArgRole::None => {}
        }
        let what = match element.name.as_str() {
            "image" => "an alternative text",
            _ => "a name",
        };
        let code = if element.name == "image" {
            "E202"
        } else {
            "E201"
        };
        self.error(
            code,
            format!("this `{}` has no accessible name", element.name),
            element.span,
        )
        .note(match element.name.as_str() {
            "image" => "Without `alt`, a screen reader announces the file name, or nothing at all, and the image is lost either way.",
            _ => "A control with no name is announced as its role and nothing else: \"button\", \"checkbox\", \"text field\". There is no way to tell two of them apart.",
        })
        .suggest(match element.name.as_str() {
            "image" => format!("Write `image \"/cat.png\" alt \"A cat sleeping\"`, or add `decorative` if it is only decoration. It needs {what}."),
            "input" | "textarea" | "select" | "check" | "radio" | "slider" | "toggle" => {
                format!("Add `label \"…\"`, which becomes the control's accessible name. It needs {what}.")
            }
            _ => format!("Give it text — `{} \"…\"` — or a `label`. It needs {what}.", element.name),
        });
    }

    fn nesting(&mut self, element: &Element) {
        let Some((_, allowed)) = NESTING
            .iter()
            .find(|(name, _)| *name == element.name.as_str())
        else {
            return;
        };
        if self
            .parents
            .iter()
            .any(|parent| allowed.contains(&parent.as_str()))
        {
            return;
        }
        // Inside a component the writer may be filling a slot that lands in the
        // right place, so this is a warning rather than an error.
        let inside_component = self.in_component;
        let title = format!("`{}` belongs inside {}", element.name, list_of(allowed));
        let note = "A browser moves an element it does not expect out of the table or list, which breaks the layout and, for a table, the meaning of the rows.";
        let suggestion = format!("Put it inside {}.", list_of(allowed));
        if inside_component {
            self.warning("W105", title, element.span)
                .note(note)
                .suggest(&suggestion);
        } else {
            self.error("E208", title, element.span)
                .note(note)
                .suggest(&suggestion);
        }
    }

    // --- components -------------------------------------------------------

    fn invocation(&mut self, invocation: &mut Use) {
        let Some(info) = self.components.get(&invocation.name).cloned() else {
            let mut names: Vec<&str> = self.components.keys().map(String::as_str).collect();
            names.sort_unstable();
            let suggestion = match closest_of(&invocation.name, &names) {
                Some(candidate) => format!("Did you mean `{candidate}`?"),
                None if names.is_empty() => {
                    "No component is declared in this file yet.".to_string()
                }
                None => format!("The components in this file are {}.", list_of(&names)),
            };
            self.error(
                "E203",
                format!("`{}` is not a component", invocation.name),
                invocation.span,
            )
            .note("A name starting with a capital letter is a component invocation, so it has to be declared with `component`.")
            .suggest(&suggestion);
            // The props and children are still worth checking for names.
            self.invocation_parts(invocation, None);
            return;
        };
        self.invocation_parts(invocation, Some(&info));
    }

    fn invocation_parts(&mut self, invocation: &mut Use, info: Option<&ComponentInfo>) {
        let declared: Vec<String> = match info {
            Some(info) => info.props.iter().map(|prop| prop.name.clone()).collect(),
            None => Vec::new(),
        };

        if let Some(info) = info {
            if invocation.positional.len() > info.props.len() {
                self.error(
                    "E212",
                    format!(
                        "`{}` takes {} prop{}, but {} were given",
                        invocation.name,
                        info.props.len(),
                        if info.props.len() == 1 { "" } else { "s" },
                        invocation.positional.len()
                    ),
                    invocation.span,
                )
                .note(format!("Its props are {}.", list_of(&declared)))
                .suggest("Pass the extra value by name, or leave it out.");
            }
        }

        let mut given: HashSet<String> = HashSet::new();
        for (index, value) in invocation.positional.iter_mut().enumerate() {
            self.expression(value);
            if let Some(name) = declared.get(index) {
                given.insert(name.clone());
            }
        }
        for (span, name, value) in invocation.props.iter_mut() {
            self.expression(value);
            given.insert(name.clone());
            if info.is_some() && !declared.iter().any(|prop| prop.as_str() == name.as_str()) {
                let suggestion = match closest_of(name, &declared) {
                    Some(candidate) => format!("Did you mean `{candidate}`?"),
                    None => format!("Its props are {}.", list_of(&declared)),
                };
                self.error(
                    "E204",
                    format!("`{}` has no prop called `{name}`", invocation.name),
                    *span,
                )
                .note("A prop that is not declared is silently dropped, so the component never sees it.")
                .suggest(&suggestion);
            }
        }
        if let Some(info) = info {
            for prop in &info.props {
                if !prop.optional && !given.contains(&prop.name) {
                    self.error(
                        "E213",
                        format!("`{}` needs a `{}`", invocation.name, prop.name),
                        invocation.span,
                    )
                    .note("The prop has no default, so the component would render without the value it was written to use.")
                    .suggest(format!(
                        "Pass it: `{}({}: …)`, or give the prop a default where it is declared.",
                        invocation.name, prop.name
                    ));
                }
            }
            for (name, _) in &invocation.slots {
                if !info.slots.is_empty() && !info.slots.contains(name) {
                    self.warning(
                        "W106",
                        format!("`{}` never renders a slot called `{name}`", invocation.name),
                        invocation.span,
                    )
                    .note(format!("Its slots are {}.", list_of(&info.slots)))
                    .suggest("Check the spelling, or add `slot` for it in the component.");
                }
            }
        }

        for prop in invocation.props_style.iter_mut() {
            self.style(prop);
        }
        for attribute in invocation.attrs.iter_mut() {
            for value in attribute.values.iter_mut() {
                if let Expr::Name(name, _) = value {
                    if self.known(name) {
                        continue;
                    }
                } else {
                    self.expression(value);
                }
            }
            attribute.dynamic = attribute
                .values
                .iter()
                .any(|value| expr::is_dynamic(value, &self.scope));
        }
        for handler in invocation.handlers.iter_mut() {
            self.handler(handler);
        }
        if let Some(condition) = &invocation.show {
            self.expression(condition);
        }
        self.nodes(&mut invocation.children);
        for (_, nodes) in invocation.slots.iter_mut() {
            self.nodes(nodes);
        }
    }
}

/// The named slots a body reads.
fn collect_slots(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match node {
            Node::Slot(slot) => {
                if let Some(name) = &slot.name {
                    if !out.contains(name) {
                        out.push(name.clone());
                    }
                }
            }
            Node::Element(element) => collect_slots(&element.children, out),
            Node::Use(invocation) => {
                collect_slots(&invocation.children, out);
                for (_, nodes) in &invocation.slots {
                    collect_slots(nodes, out);
                }
            }
            Node::If(condition) => {
                for (_, body) in &condition.branches {
                    collect_slots(body, out);
                }
                if let Some(body) = &condition.else_branch {
                    collect_slots(body, out);
                }
            }
            Node::Each(each) => collect_slots(&each.body, out),
            Node::Fn(function) => collect_slots(&function.body, out),
            _ => {}
        }
    }
}

/// The level a heading ends up at.
fn level_of(element: &Element) -> u32 {
    match literal_attribute(element, "level") {
        Some(level) => level.parse::<u32>().unwrap_or(1).clamp(1, 6),
        None => 1,
    }
}

fn literal_attribute(element: &Element, name: &str) -> Option<String> {
    let attribute = element.attrs.iter().find(|a| a.name == name)?;
    literal_text(&attribute.values)
}

/// The text of a value list, if it is all literal.
fn literal_text(values: &[Expr]) -> Option<String> {
    let mut out = String::new();
    for value in values {
        match value {
            Expr::Str(parts, _) => {
                for part in parts {
                    match part {
                        crate::ast::StrPart::Text(text) => out.push_str(text),
                        crate::ast::StrPart::Expr(_) => return None,
                    }
                }
            }
            Expr::Name(word, _) => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(word);
            }
            Expr::Int(number, _) => out.push_str(&number.to_string()),
            _ => return None,
        }
    }
    Some(out)
}

fn has_destination(element: &Element) -> bool {
    !element.args.is_empty()
        || element
            .attrs
            .iter()
            .any(|attribute| matches!(attribute.name.as_str(), "href" | "to"))
}

fn named_by_attribute(element: &Element) -> bool {
    element.attrs.iter().any(|attribute| {
        NAME_ATTRIBUTES.contains(&attribute.name.as_str())
            // A value that is worked out at run time may well be the name, and
            // an empty literal certainly is not.
            && match literal_text(&attribute.values) {
                Some(text) => !text.is_empty(),
                // A value worked out at run time may well be the name.
                None => true,
            }
    })
}

/// True when an element's children supply text, which is a name for a `link`, a
/// `button` or a `toggle` just as much as an argument on the element line is.
fn text_in_children(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Element(child) => {
            (child.def.role == ArgRole::Content && !child.args.is_empty())
                || text_in_children(&child.children)
        }
        // The caller fills a slot, so the text may well arrive from outside.
        Node::Slot(_) => true,
        Node::If(condition) => {
            condition
                .branches
                .iter()
                .any(|(_, body)| text_in_children(body))
                || condition
                    .else_branch
                    .as_ref()
                    .is_some_and(|body| text_in_children(body))
        }
        Node::Each(each) => text_in_children(&each.body),
        _ => false,
    })
}

fn list_of(names: &[impl AsRef<str>]) -> String {
    match names.len() {
        0 => "nothing".to_string(),
        1 => format!("`{}`", names[0].as_ref()),
        _ => {
            let quoted: Vec<String> = names
                .iter()
                .map(|name| format!("`{}`", name.as_ref()))
                .collect();
            let (last, rest) = quoted.split_last().expect("checked above");
            format!("{} or {last}", rest.join(", "))
        }
    }
}

fn closest_of<'a>(name: &str, candidates: &'a [impl AsRef<str>]) -> Option<&'a str> {
    let letters: Vec<char> = name.chars().collect();
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        let other: Vec<char> = candidate.as_ref().chars().collect();
        let distance = tables::edit_distance(&letters, &other);
        if distance > 2 {
            continue;
        }
        match best {
            Some((best_distance, _)) if distance >= best_distance => {}
            _ => best = Some((distance, candidate.as_ref())),
        }
    }
    best.map(|(_, candidate)| candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css;
    use crate::parser::parse_source;

    /// Parses and checks, returning the diagnostics by code.
    fn codes(source: &str) -> Vec<String> {
        let (mut program, parse_errors) = parse_source(FileId(0), source);
        assert!(
            parse_errors.is_empty(),
            "parser diagnostics {parse_errors:?} for:\n{source}"
        );
        let found = check(&mut program);
        found.into_iter().map(|d| d.code).collect()
    }

    fn titles(source: &str) -> Vec<String> {
        let (mut program, _) = parse_source(FileId(0), source);
        check(&mut program).into_iter().map(|d| d.title).collect()
    }

    fn clean(source: &str) {
        let found = codes(source);
        assert!(found.is_empty(), "unexpected {found:?} for:\n{source}");
    }

    #[test]
    fn a_page_that_needs_nothing_says_nothing() {
        clean("main\n  heading \"Hello\"\n  text \"A page\"\n  button \"Go\"\n    on click => run()\n");
        clean("let count = 0\ntext \"~(count)\"\n");
        clean("image \"/cat.png\" alt \"A cat\"\n");
    }

    #[test]
    fn a_control_without_a_name_is_an_error() {
        let found = codes("button\n  on click => run()\n");
        assert!(found.contains(&"E201".to_string()), "{found:?}");
        assert!(
            titles("button\n  on click => run()\n")[0].contains("no accessible name"),
            "title"
        );

        // Text, a child that has text, or a label all name it.
        clean("button \"Go\"\n");
        clean("button\n  text \"Go\"\n");
        clean("check label \"I agree\"\n");
        clean("input label \"Email\" type email\n");
    }

    #[test]
    fn an_image_needs_alternative_text_or_has_to_say_it_is_decoration() {
        assert!(codes("image \"/cat.png\"\n").contains(&"E202".to_string()));
        clean("image \"/cat.png\" alt \"A cat\"\n");
        clean("image \"/cat.png\" decorative\n");
        clean("image \"/cat.png\" \"A cat\"\n");
    }

    #[test]
    fn an_unknown_name_is_reported_with_the_nearest_one() {
        let found = codes("let count = 0\ntext \"~(cont)\"\n");
        assert!(found.contains(&"E210".to_string()), "{found:?}");
        let (mut program, _) = parse_source(FileId(0), "let count = 0\ntext \"~(cont)\"\n");
        let diagnostics = check(&mut program);
        assert!(
            diagnostics[0]
                .suggestion
                .as_deref()
                .unwrap_or_default()
                .contains("count"),
            "{:?}",
            diagnostics[0].suggestion
        );
    }

    #[test]
    fn a_bare_word_is_a_variable_only_when_something_declares_it() {
        // `red` is a colour and `primary` is a theme token; neither is declared.
        clean("box color red padding 1rem\n");
        clean("theme\n  colors\n    primary #4f46e5\nbox color primary\n");
        // Declared, so it is the variable, and the property becomes dynamic.
        clean("let brand = \"#f00\"\nbox color brand\n");

        let (mut program, _) = parse_source(FileId(0), "let brand = \"#f00\"\nbox color brand\n");
        check(&mut program);
        let node = first_element(&program);
        assert!(node.props[0].dynamic, "a declared value is dynamic");

        let (mut program, _) = parse_source(FileId(0), "box color red\n");
        check(&mut program);
        let node = first_element(&program);
        assert!(!node.props[0].dynamic, "a CSS keyword is not dynamic");
    }

    #[test]
    fn an_attribute_keyword_is_not_a_variable_but_a_declared_name_is() {
        let (mut program, _) = parse_source(FileId(0), "input type email\n");
        check(&mut program);
        let node = first_element(&program);
        assert!(!node.attrs[0].dynamic, "`email` is the keyword");

        let (mut program, _) = parse_source(FileId(0), "let kind = \"email\"\ninput type kind\n");
        check(&mut program);
        let node = first_element(&program);
        assert!(node.attrs[0].dynamic, "`kind` is the variable");
    }

    #[test]
    fn a_derived_value_cannot_be_written() {
        assert!(
            codes("let count = 0\nderive double = count * 2\nbox\n  on click => double = 4\n")
                .contains(&"E205".to_string())
        );
        clean("let count = 0\nderive double = count * 2\nbox\n  on click => count = 4\n");
    }

    #[test]
    fn binding_needs_state() {
        assert!(codes("input bind nothing label \"X\"\n").contains(&"E210".to_string()));
        assert!(
            codes("derive total = 1\ninput bind total label \"X\"\n").contains(&"E205".to_string())
        );
        clean("let email = \"\"\ninput bind email label \"Email\"\n");
    }

    #[test]
    fn a_duplicate_declaration_is_reported() {
        assert!(codes("let a = 1\nlet a = 2\ntext \"~(a)\"\n").contains(&"E206".to_string()));
        // A nested block may shadow, which is not a mistake.
        clean("let a = 1\nfn go()\n  let a = 2\n  a = 3\n");
    }

    #[test]
    fn an_unknown_component_is_reported() {
        assert!(codes("main\n  Card\n").contains(&"E203".to_string()));
        clean("component Card\n  text \"hi\"\nmain\n  Card\n");
    }

    #[test]
    fn a_prop_the_component_does_not_have_is_reported() {
        let found = codes(
            "component Card(title: string)\n  text \"~(title)\"\nmain\n  Card(titel: \"x\")\n",
        );
        assert!(found.contains(&"E204".to_string()), "{found:?}");
        clean("component Card(title: string)\n  text \"~(title)\"\nmain\n  Card(title: \"x\")\n");
    }

    #[test]
    fn a_prop_without_a_default_has_to_be_given() {
        assert!(
            codes("component Card(title: string)\n  text \"~(title)\"\nmain\n  Card\n")
                .contains(&"E213".to_string())
        );
        clean("component Card(title: string = \"Hello\")\n  text \"~(title)\"\nmain\n  Card\n");
    }

    #[test]
    fn too_many_positional_props_are_reported() {
        assert!(codes(
            "component Card(title: string)\n  text \"~(title)\"\nmain\n  Card \"a\" \"b\"\n"
        )
        .contains(&"E212".to_string()));
    }

    #[test]
    fn a_slot_nothing_renders_is_a_warning() {
        let found = codes("component Card\n  box\n    slot header\nmain\n  Card\n    slot footer\n      text \"x\"\n");
        assert!(found.contains(&"W106".to_string()), "{found:?}");
    }

    #[test]
    fn children_outside_a_component_is_an_error() {
        assert!(codes("main\n  children\n").contains(&"E211".to_string()));
        clean("component Card\n  box\n    children\nmain\n  Card\n");
    }

    #[test]
    fn a_list_item_outside_a_list_is_reported() {
        assert!(codes("main\n  item \"x\"\n").contains(&"E208".to_string()));
        clean("list\n  item \"x\"\n");
        clean("table\n  table-row\n    table-cell \"x\"\n");
        assert!(codes("table\n  table-cell \"x\"\n").contains(&"E208".to_string()));
        assert!(codes("select\n  text \"x\"\n  item \"y\"\n").contains(&"E208".to_string()));
    }

    #[test]
    fn several_top_level_headings_are_a_warning() {
        let found = codes("main\n  heading \"One\"\n  heading \"Two\"\n");
        assert!(found.contains(&"W101".to_string()), "{found:?}");
        clean("main\n  heading \"One\"\n  heading \"Two\"\n    level 2\n");
    }

    #[test]
    fn a_duplicated_id_is_a_warning() {
        let found = codes("main\n  box id \"a\"\n  box id \"a\"\n");
        assert!(found.contains(&"W102".to_string()), "{found:?}");
    }

    #[test]
    fn a_link_with_nowhere_to_go_is_a_warning() {
        // A link's first argument is its destination, so a link with no
        // argument and no `href` is the one that goes nowhere.
        let found = codes("link\n  on click => run()\n");
        assert!(found.contains(&"W103".to_string()), "{found:?}");
        clean("link \"/about\" \"About\"\n");
        clean("link to \"/about\" \"About\"\n");
    }

    #[test]
    fn a_container_does_not_take_text() {
        assert!(codes("box \"hello\"\n").contains(&"E209".to_string()));
        clean("box\n  text \"hello\"\n");
    }

    #[test]
    fn a_storage_key_has_to_be_literal() {
        assert!(codes("let key = \"k\"\nlet saved = 0 persist key\n").contains(&"E214".to_string()));
        clean("let saved = 0 persist \"saved\"\n");
    }

    #[test]
    fn javascript_globals_are_reachable_and_typos_are_not() {
        clean("box\n  on click => console.log(\"hi\")\n");
        clean("let width = window.innerWidth\n");
        assert!(codes("box\n  on click => consle.log(\"hi\")\n").contains(&"E210".to_string()));
    }

    #[test]
    fn an_each_binds_its_row_and_its_index() {
        clean(
            "let todos = [1]\nlist\n  each todo, i in todos key todo\n    item \"~(i) ~(todo)\"\n",
        );
        assert!(
            codes("let todos = [1]\nlist\n  each todo in todos\n    item \"~(nope)\"\n")
                .contains(&"E210".to_string())
        );
    }

    #[test]
    fn checked_output_still_compiles_to_a_stylesheet_and_a_module() {
        let source = "let count = 0\nmain\n  heading \"~(count) items\"\n  button \"Add\"\n    on click => count = count + 1\n";
        let (mut program, parse_errors) = parse_source(FileId(0), source);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let found = check(&mut program);
        assert!(found.is_empty(), "{found:?}");
        let styles = css::collect(&mut program, "p");
        let (module, warnings) = crate::js::module(&program, "#app");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(module.contains("P.signal(0)"), "{module}");
        assert!(module.contains("P.mount(\"#app\", App);"), "{module}");
        // The base stylesheet is always there, so an app is styled even when it
        // declares nothing of its own.
        assert!(styles.stylesheet(false).contains(".pl-stack"), "{source}");
    }

    /// The first element in a program, for assertions about what checking did to
    /// it.
    fn first_element(program: &Program) -> &Element {
        find_element(&program.body).expect("an element")
    }

    fn find_element(nodes: &[Node]) -> Option<&Element> {
        for node in nodes {
            match node {
                Node::Element(element) => return Some(element),
                Node::Use(invocation) => {
                    if let Some(found) = find_element(&invocation.children) {
                        return Some(found);
                    }
                }
                Node::If(condition) => {
                    for (_, body) in &condition.branches {
                        if let Some(found) = find_element(body) {
                            return Some(found);
                        }
                    }
                }
                Node::Each(each) => {
                    if let Some(found) = find_element(&each.body) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
}
