//! Scoped CSS generation.
//!
//! Plain styles are written on the element they belong to, so there is no
//! cascade to reason about and no selector to invent. This pass turns each
//! element's style properties, `when` blocks and `@breakpoint` blocks into one
//! generated class, writes that class back onto the element for the JavaScript
//! pass to pick up, and emits the rules in first-use order.
//!
//! # One class per set of declarations
//!
//! Classes are deduplicated by the rules they produce, so two elements styled the
//! same way share one class and the stylesheet does not grow with the page. The
//! numbering follows first use, which makes the output deterministic: the same
//! source always produces the same CSS, so a diff between two builds shows what
//! changed in the program rather than what changed in a hash.
//!
//! # What is not here
//!
//! A property whose value is not known until runtime is left to the JavaScript
//! pass, which writes it as an inline style effect — see [`is_dynamic`]. The
//! layout primitives (`.pl-stack`, `.pl-row`, `.pl-grid`, `.pl-layer`) live in the
//! runtime's base stylesheet rather than here, because they are the same in every
//! program.

use std::collections::HashMap;

use plain_core::{Diagnostic, FileId};

use crate::ast::{
    Expr, Modifier, Node, Program, PseudoBlock, ResponsiveBlock, StrPart, StyleProp, Theme,
    ThemeEntry,
};
use crate::expr::{self, Scope};
use crate::parser::trim_number;
use crate::runtime::BASE_CSS;
use crate::tables;

/// One CSS rule: a selector suffix, the media query it belongs to, and the
/// declarations inside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Rule {
    /// `":hover"`, or empty for the element itself.
    suffix: String,
    /// `"(min-width: 640px)"`, or empty for an unconditional rule.
    query: String,
    declarations: Vec<(String, String)>,
}

/// The stylesheet a program compiles to.
#[derive(Debug, Default)]
pub struct Styles {
    /// `:root` custom properties from the theme, in declaration order.
    variables: Vec<(String, String)>,
    /// Dark-mode overrides of those properties.
    dark: Vec<(String, String)>,
    /// Class rules in first-use order.
    classes: Vec<(String, Vec<Rule>)>,
    /// Media queries in first-use order, so the stylesheet groups them.
    queries: Vec<String>,
    /// Problems that did not stop the build.
    pub diagnostics: Vec<Diagnostic>,
}

/// Collects the styles of a whole program.
///
/// Every element and every component invocation that has styles gets its
/// generated class written onto the AST, which is what the JavaScript pass reads
/// when it builds the node.
pub fn collect(program: &mut Program, class_prefix: &str) -> Styles {
    let theme = ThemeVars::new(program.theme.as_ref());
    let mut builder = Builder {
        file: program.file,
        prefix: class_prefix.to_string(),
        theme,
        seen: HashMap::new(),
        styles: Styles::default(),
    };
    builder.styles.variables = builder.theme.definitions();
    builder.styles.dark = builder.theme.dark_definitions();

    for component in &mut program.components {
        for node in &mut component.body {
            builder.walk(node);
        }
    }
    for node in &mut program.body {
        builder.walk(node);
    }
    builder.styles
}

struct Builder {
    file: FileId,
    prefix: String,
    theme: ThemeVars,
    /// The rule list a class was already given, so identical styles share one.
    seen: HashMap<Vec<Rule>, String>,
    styles: Styles,
}

impl Builder {
    fn walk(&mut self, node: &mut Node) {
        match node {
            Node::Element(element) => {
                let rules = self.rules(
                    &element.props,
                    &element.pseudos,
                    &element.responsive,
                    &element.modifiers,
                );
                if !rules.is_empty() {
                    element.class = Some(self.class_for(rules));
                }
                for child in &mut element.children {
                    self.walk(child);
                }
            }
            // A component invocation that carries styles of its own is rendered
            // into a wrapper box, and the wrapper is what carries the class.
            Node::Use(invocation) => {
                let rules = self.rules(
                    &invocation.props_style,
                    &invocation.pseudos,
                    &invocation.responsive,
                    &invocation.modifiers,
                );
                if !rules.is_empty() {
                    invocation.class = Some(self.class_for(rules));
                }
                for child in &mut invocation.children {
                    self.walk(child);
                }
                for (_, nodes) in &mut invocation.slots {
                    for child in nodes {
                        self.walk(child);
                    }
                }
            }
            Node::If(condition) => {
                for (_, nodes) in &mut condition.branches {
                    for child in nodes {
                        self.walk(child);
                    }
                }
                if let Some(nodes) = &mut condition.else_branch {
                    for child in nodes {
                        self.walk(child);
                    }
                }
            }
            Node::Each(each) => {
                for child in &mut each.body {
                    self.walk(child);
                }
            }
            _ => {}
        }
    }

    /// Everything one element's styling turns into.
    fn rules(
        &mut self,
        props: &[StyleProp],
        pseudos: &[PseudoBlock],
        responsive: &[ResponsiveBlock],
        modifiers: &[Modifier],
    ) -> Vec<Rule> {
        let mut rules = Vec::new();

        let mut base = self.declarations(props);
        for modifier in modifiers {
            base.extend(modifier_declarations(&modifier.name));
        }
        if !base.is_empty() {
            rules.push(Rule {
                suffix: String::new(),
                query: String::new(),
                declarations: base,
            });
        }

        for pseudo in pseudos {
            self.pseudo_rules(pseudo, "", &mut rules);
        }
        for block in responsive {
            self.responsive_rules(block, &mut rules);
        }
        rules
    }

    fn pseudo_rules(&mut self, pseudo: &PseudoBlock, query: &str, rules: &mut Vec<Rule>) {
        let suffix = tables::pseudo(&pseudo.name).unwrap_or(":hover").to_string();
        let declarations = self.declarations(&pseudo.props);
        if !declarations.is_empty() {
            rules.push(Rule {
                suffix,
                query: query.to_string(),
                declarations,
            });
        }
        for nested in &pseudo.responsive {
            self.responsive_rules(nested, rules);
        }
    }

    fn responsive_rules(&mut self, block: &ResponsiveBlock, rules: &mut Vec<Rule>) {
        // The parser fills in the query for every breakpoint Plain knows. A
        // custom one is declared in the theme, which only this pass can see.
        let query = if block.query.is_empty() {
            self.theme.breakpoint(&block.name)
        } else {
            Some(block.query.clone())
        };
        let Some(query) = query else {
            // Nothing declares it, and the semantic pass says so. Emitting a rule
            // with no query would apply it at every width.
            return;
        };
        if !self.styles.queries.contains(&query) {
            self.styles.queries.push(query.clone());
        }

        let mut declarations = self.declarations(&block.props);
        for modifier in &block.modifiers {
            declarations.extend(modifier_declarations(&modifier.name));
        }
        // `show false` under a breakpoint is a common way to drop something on a
        // small screen, and when the condition is static it is pure CSS.
        if let Some(condition) = &block.show {
            if is_static_condition(condition) {
                declarations.push((
                    "display".to_string(),
                    if is_false(condition) {
                        "none"
                    } else {
                        "revert"
                    }
                    .to_string(),
                ));
            } else {
                self.styles.diagnostics.push(
                    Diagnostic::warning(
                        "W120",
                        "`show` under a breakpoint needs a value known at compile time",
                        self.file,
                    )
                    .with_label(block.span, "")
                    .with_note("A condition that reads state has to be evaluated in the browser, and v0.1 does not watch media queries at runtime.")
                    .with_suggestion("Use a static condition here, or put the element inside an `if`."),
                );
            }
        }
        if !declarations.is_empty() {
            rules.push(Rule {
                suffix: String::new(),
                query: query.clone(),
                declarations,
            });
        }
        for pseudo in &block.pseudos {
            self.pseudo_rules(pseudo, &query, rules);
        }
    }

    /// The class name for a set of rules, reusing one that already exists.
    fn class_for(&mut self, rules: Vec<Rule>) -> String {
        if let Some(existing) = self.seen.get(&rules) {
            return existing.clone();
        }
        let name = format!("{}{}", self.prefix, self.styles.classes.len() + 1);
        self.seen.insert(rules.clone(), name.clone());
        self.styles.classes.push((name.clone(), rules));
        name
    }

    /// The declarations a list of style properties produces.
    fn declarations(&self, props: &[StyleProp]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for prop in props {
            if is_dynamic(prop) {
                // The JavaScript pass writes this one as an inline style effect.
                continue;
            }
            out.extend(declarations_for(prop, &self.theme));
        }
        out
    }
}

/// The theme's custom properties, so a value becomes a variable only when the
/// theme actually declares it.
#[derive(Debug, Default)]
struct ThemeVars {
    /// Category, token name, value.
    entries: Vec<(String, String, String)>,
    dark: Vec<(String, String, String)>,
    /// Breakpoints the theme declares: name and the media query it stands for.
    breakpoints: Vec<(String, String)>,
}

impl ThemeVars {
    fn new(theme: Option<&Theme>) -> ThemeVars {
        let mut vars = ThemeVars::default();
        let Some(theme) = theme else {
            return vars;
        };
        for group in &theme.groups {
            if group.name == "breakpoints" {
                // Custom breakpoints extend the media queries, not the variables.
                for entry in &group.entries {
                    if let Some(query) = breakpoint_query(entry) {
                        vars.breakpoints.push((entry.name.clone(), query));
                    }
                }
                continue;
            }
            for entry in &group.entries {
                vars.add(&group.name, entry, false);
            }
            for entry in &group.dark {
                vars.add(&group.name, entry, true);
            }
        }
        vars
    }

    fn add(&mut self, category: &str, entry: &ThemeEntry, dark: bool) {
        // A theme's own values are literal CSS: resolving them against the theme
        // would let one token point at another and never terminate.
        let value = value_text(category, "", &entry.values, self);
        if dark {
            self.dark
                .push((category.to_string(), entry.name.clone(), value));
        } else {
            self.entries
                .push((category.to_string(), entry.name.clone(), value));
        }
    }

    /// The media query a custom breakpoint stands for, if the theme declares it.
    fn breakpoint(&self, name: &str) -> Option<String> {
        self.breakpoints
            .iter()
            .find(|(token, _)| token == name)
            .map(|(_, query)| query.clone())
    }

    /// `var(--color-primary)`, when the theme declares that token.
    fn variable(&self, category: &str, name: &str) -> Option<String> {
        let declared = self
            .entries
            .iter()
            .any(|(token_category, token, _)| token_category == category && token == name);
        declared.then(|| format!("var({})", custom_property(category, name)))
    }

    fn definitions(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|(category, name, value)| (custom_property(category, name), value.clone()))
            .collect()
    }

    fn dark_definitions(&self) -> Vec<(String, String)> {
        self.dark
            .iter()
            .map(|(category, name, value)| (custom_property(category, name), value.clone()))
            .collect()
    }
}

/// The media query a theme breakpoint declares. `huge 1800px` is a width and
/// `reduced-motion` is a feature Plain already knows how to spell.
fn breakpoint_query(entry: &ThemeEntry) -> Option<String> {
    match entry.values.as_slice() {
        [Expr::Length(number, unit, _)] => {
            Some(format!("(min-width: {}{unit})", trim_number(*number)))
        }
        [Expr::Int(number, _)] if *number > 0 => Some(format!("(min-width: {number}px)")),
        // `@reduced-motion` is a feature Plain already knows how to spell.
        [Expr::Name(feature, _)] => tables::media_query(feature).map(|query| query.to_string()),
        _ => None,
    }
}

/// The custom property a theme token becomes.
///
/// The categories are plural in Plain (`colors`, `shadows`) and singular in CSS
/// (`--color-primary`, `--shadow-sm`), which is what the base stylesheet already
/// refers to.
pub fn custom_property(category: &str, name: &str) -> String {
    let prefix = match category {
        "colors" => "color",
        "fonts" => "font",
        "sizes" => "size",
        "spacing" => "space",
        "shadows" => "shadow",
        other => other,
    };
    format!("--{prefix}-{name}")
}

/// Properties whose values are counts rather than lengths, so a bare number stays
/// a bare number instead of becoming pixels.
const UNITLESS: &[&str] = &[
    "z-index",
    "opacity",
    "order",
    "flex",
    "flex-grow",
    "flex-shrink",
    "line-height",
    "font-weight",
    "zoom",
    "aspect-ratio",
    "column-count",
    "orphans",
    "widows",
    "tab-size",
    "animation-iteration-count",
    "grid-row",
    "grid-column",
    "grid-area",
    "scale",
];

/// The modifiers that are layout rather than attributes. Everything else in
/// [`tables::MODIFIERS`] becomes an HTML attribute or an accessibility marker in
/// the JavaScript pass.
const STYLE_MODIFIERS: &[&str] = &["fill", "fill-x", "fill-y", "hidden"];

/// True when a modifier is emitted as CSS rather than as an attribute.
pub fn is_style_modifier(name: &str) -> bool {
    STYLE_MODIFIERS.contains(&name)
}

fn modifier_declarations(name: &str) -> Vec<(String, String)> {
    // `flex-basis: 0%` and a zero minimum size are what make "take the remaining
    // space" work: the default `min-width: auto` stops a flex item shrinking
    // below its content, which is the bug behind most broken flex layouts.
    let grow = vec![
        ("flex-grow".to_string(), "1".to_string()),
        ("flex-shrink".to_string(), "1".to_string()),
        ("flex-basis".to_string(), "0%".to_string()),
    ];
    let mut out = match name {
        "fill" | "fill-x" | "fill-y" => grow,
        "hidden" => return vec![("display".to_string(), "none".to_string())],
        _ => return Vec::new(),
    };
    if name != "fill-y" {
        out.push(("min-width".to_string(), "0".to_string()));
    }
    if name != "fill-x" {
        out.push(("min-height".to_string(), "0".to_string()));
    }
    out
}

/// The CSS property a Plain property name becomes.
pub fn css_name(property: &str) -> String {
    tables::css_property(property)
}

/// The declarations one style property produces.
///
/// Most produce one. `size` produces two, because it sets both dimensions, and
/// `columns 2` produces a `repeat()` rather than a number, because that is what
/// "two equal columns that are allowed to shrink" means in CSS grid.
fn declarations_for(prop: &StyleProp, theme: &ThemeVars) -> Vec<(String, String)> {
    if prop.name == "size" {
        return match prop.values.as_slice() {
            [one] => {
                let value = value_text("", "width", std::slice::from_ref(one), theme);
                vec![
                    ("width".to_string(), value.clone()),
                    ("height".to_string(), value),
                ]
            }
            [width, height, ..] => vec![
                (
                    "width".to_string(),
                    value_text("", "width", std::slice::from_ref(width), theme),
                ),
                (
                    "height".to_string(),
                    value_text("", "height", std::slice::from_ref(height), theme),
                ),
            ],
            [] => Vec::new(),
        };
    }

    let name = css_name(&prop.name);
    if matches!(
        name.as_str(),
        "grid-template-columns" | "grid-template-rows"
    ) {
        if let Some(tracks) = single_number(&prop.values) {
            // Equal tracks that may shrink below their content, which is what
            // `1fr` on its own does not allow.
            return vec![(name, format!("repeat({tracks}, minmax(0, 1fr))"))];
        }
    }
    let value = value_text(&prop.name, &name, &prop.values, theme);
    vec![(name, value)]
}

/// The number in a one-number value list, for `columns 2`.
fn single_number(values: &[Expr]) -> Option<String> {
    match values {
        [Expr::Int(number, _)] => Some(number.to_string()),
        [Expr::Float(number, _)] => Some(trim_number(*number)),
        _ => None,
    }
}

/// Renders a value list as CSS text.
///
/// `plain` is the property name as written and `css` the property it became;
/// either may be empty, which is how a theme's own values are rendered without
/// resolving tokens against themselves.
fn value_text(plain: &str, css: &str, values: &[Expr], theme: &ThemeVars) -> String {
    values
        .iter()
        .map(|value| one_value(plain, css, value, theme))
        .collect::<Vec<_>>()
        .join(" ")
}

fn one_value(plain: &str, css: &str, value: &Expr, theme: &ThemeVars) -> String {
    match value {
        Expr::Length(number, unit, _) => format!("{}{unit}", trim_number(*number)),
        Expr::Color(text, _) => text.clone(),
        Expr::Int(number, _) => with_unit(css, number.to_string()),
        Expr::Float(number, _) => with_unit(css, trim_number(*number)),
        Expr::Str(parts, _) => {
            let text = string_text(parts);
            // `content` is the one property whose value is a string rather than a
            // keyword, and CSS wants the quotes kept.
            if css == "content" {
                expr::js_string(&text)
            } else {
                text
            }
        }
        Expr::Name(name, _) => token(plain, css, name, theme),
        Expr::Bool(flag, _) => flag.to_string(),
        Expr::None(_) => "none".to_string(),
        Expr::Unary(operator, operand, _) => {
            format!("{operator}{}", one_value(plain, css, operand, theme))
        }
        // Anything else is a computed value, which the JavaScript pass handles.
        other => expr::to_js(other, &Scope::new()),
    }
}

fn with_unit(property: &str, number: String) -> String {
    if property.is_empty() || UNITLESS.contains(&property) {
        number
    } else {
        format!("{number}px")
    }
}

/// A bare name in a value: a theme token when the theme declares one for this
/// property, and otherwise a CSS keyword.
fn token(plain: &str, css: &str, name: &str, theme: &ThemeVars) -> String {
    // The property decides which group of tokens to look in: `color primary` is a
    // colour and `padding lg` is a spacing. `fill` is spelled differently in Plain
    // than in CSS, so both spellings are tried.
    let mut category = tables::token_category(css);
    if category.is_empty() {
        category = tables::token_category(plain);
    }
    if category.is_empty() {
        return name.to_string();
    }
    theme
        .variable(category, name)
        .unwrap_or_else(|| name.to_string())
}

/// The text of a string value. Plain's quotes are not CSS's, so they are dropped.
fn string_text(parts: &[StrPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            StrPart::Text(text) => text.as_str(),
            // A computed value: the JavaScript pass writes it as an inline style.
            StrPart::Expr(_) => "",
        })
        .collect()
}

/// True when a property's value is not known until runtime, and so has to be
/// written to the element by the runtime instead of by a stylesheet.
///
/// The semantic pass sets [`StyleProp::dynamic`] when a value name resolves to a
/// variable or a prop. An interpolation inside a string value is dynamic on its
/// face, and that one this pass can see for itself.
pub fn is_dynamic(prop: &StyleProp) -> bool {
    prop.dynamic || prop.values.iter().any(dynamic_value)
}

fn dynamic_value(value: &Expr) -> bool {
    match value {
        Expr::Str(parts, _) => parts.iter().any(|part| matches!(part, StrPart::Expr(_))),
        other => expr::is_dynamic(other, &Scope::new()),
    }
}

/// The JavaScript for a style value the runtime has to work out.
///
/// One value is handed over as it is; several become a template literal, because
/// `border 1px solid brand` has to arrive as one string.
pub fn value_js(prop: &StyleProp, scope: &Scope) -> String {
    match prop.values.as_slice() {
        [one] => expr::to_js(one, scope),
        [] => "undefined".to_string(),
        many => {
            let parts = many
                .iter()
                .map(|value| value_part(value, scope))
                .collect::<Vec<_>>()
                .join(" ");
            format!("`{parts}`")
        }
    }
}

/// One piece of a multi-part style value inside a template literal.
///
/// A literal contributes its CSS text and a computed value contributes an
/// interpolation, so `1px solid brand` becomes `1px solid ${brand()}` rather than
/// putting JavaScript's quotes into the declaration.
fn value_part(value: &Expr, scope: &Scope) -> String {
    let computed = match value {
        // Only the semantic pass knows whether a bare word is a CSS keyword or a
        // variable, and it says so by putting the name in scope.
        Expr::Name(name, _) => scope.contains(name),
        Expr::Str(parts, _) => parts.iter().any(|part| matches!(part, StrPart::Expr(_))),
        other => expr::is_dynamic(other, scope),
    };
    if computed {
        format!("${{{}}}", expr::to_js(value, scope))
    } else {
        literal_css(value, scope)
    }
}

/// The CSS text of a value that is known at compile time.
fn literal_css(value: &Expr, scope: &Scope) -> String {
    match value {
        Expr::Length(number, unit, _) => format!("{}{unit}", trim_number(*number)),
        Expr::Color(text, _) => text.clone(),
        Expr::Int(number, _) => number.to_string(),
        Expr::Float(number, _) => trim_number(*number),
        Expr::Str(parts, _) => string_text(parts),
        Expr::Name(name, _) => name.clone(),
        Expr::None(_) => "none".to_string(),
        Expr::Bool(flag, _) => flag.to_string(),
        Expr::Unary(operator, operand, _) => format!("{operator}{}", literal_css(operand, scope)),
        other => expr::to_js(other, scope),
    }
}

fn is_static_condition(condition: &Expr) -> bool {
    let interpolated = matches!(condition, Expr::Str(parts, _)
        if parts.iter().any(|part| matches!(part, StrPart::Expr(_))));
    !interpolated && !expr::is_dynamic(condition, &Scope::new())
}

fn is_false(condition: &Expr) -> bool {
    matches!(condition, Expr::Bool(false, _) | Expr::None(_))
}

impl Styles {
    /// The finished stylesheet: the base reset, the theme, then one block per
    /// class and one per media query.
    pub fn stylesheet(&self, minify: bool) -> String {
        let mut out = String::from(BASE_CSS);

        if !self.variables.is_empty() {
            out.push_str("\n:root {\n");
            for (name, value) in &self.variables {
                out.push_str(&format!("  {name}: {value};\n"));
            }
            out.push_str("}\n");
        }
        if !self.dark.is_empty() {
            // Both the system preference and an explicit class, so an app can
            // offer its own switch without fighting the operating system.
            out.push_str("\n@media (prefers-color-scheme: dark) {\n  :root:not(.light) {\n");
            for (name, value) in &self.dark {
                out.push_str(&format!("    {name}: {value};\n"));
            }
            out.push_str("  }\n}\n\n.dark {\n");
            for (name, value) in &self.dark {
                out.push_str(&format!("  {name}: {value};\n"));
            }
            out.push_str("}\n");
        }

        for (class, rules) in &self.classes {
            for rule in rules {
                if rule.query.is_empty() {
                    out.push('\n');
                    out.push_str(&render_rule(class, rule, ""));
                }
            }
        }

        for query in &self.queries {
            out.push_str(&format!("\n@media {query} {{\n"));
            for (class, rules) in &self.classes {
                for rule in rules {
                    if &rule.query == query {
                        out.push_str(&render_rule(class, rule, "  "));
                    }
                }
            }
            out.push_str("}\n");
        }

        if minify {
            minify_css(&out)
        } else {
            out
        }
    }

    /// The classes generated, in first-use order.
    pub fn classes(&self) -> Vec<&str> {
        self.classes.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// True when the stylesheet declares nothing of its own.
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.variables.is_empty()
    }
}

fn render_rule(class: &str, rule: &Rule, indent: &str) -> String {
    let mut out = format!("{indent}.{class}{} {{\n", rule.suffix);
    for (property, value) in &rule.declarations {
        out.push_str(&format!("{indent}  {property}: {value};\n"));
    }
    out.push_str(&format!("{indent}}}\n"));
    out
}

/// A conservative CSS minifier.
///
/// It removes comments and the whitespace around punctuation, and keeps
/// everything inside a string. It does not rewrite values, reorder rules or drop
/// anything a browser might need: minifying CSS properly means understanding it,
/// and a build step that can change what a page looks like is not worth the
/// bytes.
fn minify_css(source: &str) -> String {
    /// Whitespace next to one of these carries no meaning.
    const DROPPABLE: &[char] = &['{', '}', ':', ';', ',', '(', ')'];

    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(character) = chars.next() {
        if let Some(open) = quote {
            out.push(character);
            if character == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => {
                quote = Some(character);
                out.push(character);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            chars.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            other if other.is_whitespace() => {
                let before = out.chars().last().unwrap_or('{');
                let after = chars.peek().copied().unwrap_or('}');
                if !DROPPABLE.contains(&before) && !DROPPABLE.contains(&after) {
                    out.push(' ');
                }
            }
            other => out.push(other),
        }
    }
    out.trim().to_string()
}

/// The class a node was given, if it has any styles.
pub fn class_of(node: &Node) -> Option<&str> {
    match node {
        Node::Element(element) => element.class.as_deref(),
        Node::Use(invocation) => invocation.class.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Element;
    use crate::expr::Binding;
    use crate::parser::parse_source;
    use plain_core::Span;

    /// The rules this program produced, without the base stylesheet. Every test
    /// carries the same three hundred lines of reset, and printing them on
    /// failure hides the assertion that actually failed.
    fn generated(text: &str) -> &str {
        text.strip_prefix(crate::runtime::BASE_CSS).unwrap_or(text)
    }

    /// Parses `source` and collects its styles.
    fn compiled(source: &str) -> (Program, Styles) {
        let (mut program, diagnostics) = parse_source(FileId(0), source);
        assert!(diagnostics.is_empty(), "{diagnostics:?} for:\n{source}");
        let styles = collect(&mut program, "p");
        (program, styles)
    }

    fn css(source: &str) -> String {
        compiled(source).1.stylesheet(false)
    }

    fn first(program: &Program) -> &Element {
        match program.body.first() {
            Some(Node::Element(element)) => element,
            other => panic!("expected an element, got {other:?}"),
        }
    }

    #[test]
    fn a_style_property_becomes_a_scoped_class() {
        let (program, styles) = compiled("box color red padding 1rem\n");
        assert_eq!(first(&program).class.as_deref(), Some("p1"));
        let text = styles.stylesheet(false);
        assert!(text.contains(".p1 {"), "{}", generated(&text));
        assert!(text.contains("color: red;"), "{}", generated(&text));
        assert!(text.contains("padding: 1rem;"), "{}", generated(&text));
        // The base reset comes with it, so one file is the whole stylesheet.
        assert!(
            text.starts_with("/* Generated by Plain."),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn identical_styles_share_one_class() {
        let (program, styles) = compiled("box color red\nbox color red\nbox color blue\n");
        let classes: Vec<Option<&str>> = program.body.iter().map(class_of).collect();
        assert_eq!(classes, vec![Some("p1"), Some("p1"), Some("p2")]);
        assert_eq!(styles.classes(), vec!["p1", "p2"]);
    }

    #[test]
    fn the_class_prefix_is_honoured() {
        let (mut program, _) = parse_source(FileId(0), "box color red\n");
        let styles = collect(&mut program, "card");
        assert!(styles.stylesheet(false).contains(".card1 {"));
    }

    #[test]
    fn aliases_become_the_css_they_mean() {
        let text = css("box radius md shadow sm z 3 align center justify space-between\n");
        assert!(text.contains("border-radius: md;"), "{}", generated(&text));
        assert!(text.contains("box-shadow: sm;"), "{}", generated(&text));
        assert!(text.contains("z-index: 3;"), "{}", generated(&text));
        assert!(
            text.contains("align-items: center;"),
            "{}",
            generated(&text)
        );
        assert!(
            text.contains("justify-content: space-between;"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn size_sets_both_dimensions() {
        let text = css("box size 2rem\n");
        assert!(text.contains("width: 2rem;"), "{}", generated(&text));
        assert!(text.contains("height: 2rem;"), "{}", generated(&text));

        let text = css("box size 100% 2rem\n");
        assert!(text.contains("width: 100%;"), "{}", generated(&text));
        assert!(text.contains("height: 2rem;"), "{}", generated(&text));
    }

    #[test]
    fn a_bare_number_becomes_pixels_except_where_it_is_a_count() {
        let text = css("box gap 2 opacity 0.5 z 3 line-height 1.5\n");
        assert!(text.contains("gap: 2px;"), "{}", generated(&text));
        assert!(text.contains("opacity: 0.5;"), "{}", generated(&text));
        assert!(text.contains("z-index: 3;"), "{}", generated(&text));
        assert!(text.contains("line-height: 1.5;"), "{}", generated(&text));
    }

    #[test]
    fn columns_become_equal_tracks_that_can_shrink() {
        let text = css("grid columns 2\n");
        assert!(
            text.contains("grid-template-columns: repeat(2, minmax(0, 1fr));"),
            "{}",
            generated(&text)
        );
        // A template written out is left alone.
        let text = css("grid columns \"1fr 2fr\"\n");
        assert!(
            text.contains("grid-template-columns: 1fr 2fr;"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn a_theme_token_becomes_a_variable() {
        let source = "theme\n  colors\n    primary #5dade2\n    danger #c0392b\n  spacing\n    md 1rem\nbox color primary padding md\n";
        let (program, styles) = compiled(source);
        let text = styles.stylesheet(false);
        assert!(
            text.contains("--color-primary: #5dade2;"),
            "{}",
            generated(&text)
        );
        assert!(
            text.contains("--color-danger: #c0392b;"),
            "{}",
            generated(&text)
        );
        assert!(text.contains("--space-md: 1rem;"), "{}", generated(&text));
        assert!(
            text.contains("color: var(--color-primary);"),
            "{}",
            generated(&text)
        );
        assert!(
            text.contains("padding: var(--space-md);"),
            "{}",
            generated(&text)
        );
        assert_eq!(first(&program).class.as_deref(), Some("p1"));
    }

    #[test]
    fn the_property_decides_which_tokens_are_in_scope() {
        // The same word resolves in a different group under a different property.
        let source =
            "theme\n  spacing\n    md 1rem\n  sizes\n    md 1.25rem\nbox padding md font-size md\n";
        let text = css(source);
        assert!(
            text.contains("padding: var(--space-md);"),
            "{}",
            generated(&text)
        );
        assert!(
            text.contains("font-size: var(--size-md);"),
            "{}",
            generated(&text)
        );
        assert!(text.contains("--space-md: 1rem;"), "{}", generated(&text));
        assert!(text.contains("--size-md: 1.25rem;"), "{}", generated(&text));
    }

    #[test]
    fn a_word_the_theme_does_not_define_is_left_alone() {
        // `red` is a CSS keyword; inventing a variable for it would produce a
        // colour that is not there. The base stylesheet uses variables of its
        // own, so only the generated rules are worth looking at.
        let text = generated(&css("box color red\n"));
        assert!(text.contains("color: red;"), "{text}");
        assert!(!text.contains("var(--color"), "{text}");
    }

    #[test]
    fn dark_overrides_come_out_both_ways() {
        let source = "theme\n  colors\n    primary #5dade2\n    dark\n      primary #2e86c1\n";
        let text = css(source);
        assert!(
            text.contains("@media (prefers-color-scheme: dark)"),
            "{}",
            generated(&text)
        );
        assert!(text.contains(":root:not(.light)"), "{}", generated(&text));
        assert!(text.contains(".dark {"), "{}", generated(&text));
        assert!(
            text.contains("--color-primary: #2e86c1;"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn a_pseudo_block_becomes_a_pseudo_selector() {
        let text = css("button color black\n  when hover\n    background red\n");
        assert!(text.contains(".p1 {"), "{}", generated(&text));
        assert!(text.contains(".p1:hover {"), "{}", generated(&text));
        assert!(text.contains("background: red;"), "{}", generated(&text));
    }

    #[test]
    fn focus_means_focus_visible() {
        let text = css("button\n  when focus\n    outline blue\n");
        assert!(text.contains(".p1:focus-visible {"), "{}", generated(&text));
    }

    #[test]
    fn a_breakpoint_block_goes_inside_a_media_query() {
        let text = css("stack gap 1rem\n  @tablet\n    gap 2rem\n");
        assert!(
            text.contains("@media (min-width: 640px) {"),
            "{}",
            generated(&text)
        );
        assert!(text.contains("  .p1 {"), "{}", generated(&text));
        assert!(text.contains("gap: 2rem;"), "{}", generated(&text));
        // The unconditional rule is still outside the query.
        assert!(
            text.contains("\n.p1 {\n  gap: 1rem;\n}"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn one_media_query_holds_every_rule_that_shares_it() {
        let text = css(
            "box color red\n  @tablet\n    color blue\nbox color green\n  @tablet\n    color teal\n",
        );
        assert_eq!(
            text.matches("@media (min-width: 640px)").count(),
            1,
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn a_custom_breakpoint_becomes_its_own_query() {
        let source =
            "theme\n  breakpoints\n    huge 1800px\nbox color red\n  @huge\n    color blue\n";
        let text = css(source);
        assert!(
            text.contains("@media (min-width: 1800px)"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn a_media_feature_resolves_too() {
        let text = css("box color red\n  @reduced-motion\n    transition none\n");
        assert!(
            text.contains("@media (prefers-reduced-motion: reduce)"),
            "{}",
            generated(&text)
        );
        assert!(text.contains("transition: none;"), "{}", generated(&text));
    }

    #[test]
    fn show_false_under_a_breakpoint_is_pure_css() {
        let text = css("box color red\n  @mobile\n    show false\n");
        assert!(text.contains("display: none;"), "{}", generated(&text));
    }

    #[test]
    fn layout_modifiers_become_flex_declarations() {
        let text = css("box fill\n");
        assert!(text.contains("flex-grow: 1;"), "{}", generated(&text));
        assert!(text.contains("flex-basis: 0%;"), "{}", generated(&text));
        // The minimum sizes are what let a flex item actually shrink.
        assert!(text.contains("min-width: 0;"), "{}", generated(&text));
        assert!(text.contains("min-height: 0;"), "{}", generated(&text));

        assert!(css("box fill-x\n").contains("min-width: 0;"));
        assert!(!css("box fill-x\n").contains("min-height: 0;"));
        assert!(css("box hidden\n").contains("display: none;"));
        assert!(is_style_modifier("fill"));
        assert!(!is_style_modifier("required"), "required is an attribute");
    }

    #[test]
    fn a_dynamic_property_is_left_to_the_javascript_pass() {
        // Only the semantic pass can tell that `brand` is a variable rather than
        // a CSS keyword, so it marks the property the way it would.
        let mut prop = StyleProp::new(
            Span::point(0),
            "color".to_string(),
            vec![Expr::Name("brand".to_string(), Span::point(0))],
        );
        assert!(
            !is_dynamic(&prop),
            "a bare word is CSS until told otherwise"
        );
        prop.dynamic = true;
        assert!(is_dynamic(&prop));

        // An interpolation is dynamic on its face, with no semantic pass needed.
        let interpolated = StyleProp::new(
            Span::point(0),
            "color".to_string(),
            vec![Expr::Str(
                vec![
                    StrPart::Text("#".to_string()),
                    StrPart::Expr(Expr::Name("brand".to_string(), Span::point(0))),
                ],
                Span::point(0),
            )],
        );
        assert!(is_dynamic(&interpolated));

        let mut scope = Scope::new();
        scope.bind("brand", Binding::Accessor);
        assert_eq!(value_js(&prop, &scope), "brand()");
        assert_eq!(value_js(&interpolated, &scope), "`#${P.text(brand())}`");

        // Several values become one template literal, with the literal parts as
        // CSS text so no JavaScript quoting lands in the declaration.
        let border = StyleProp::new(
            Span::point(0),
            "border".to_string(),
            vec![
                Expr::Length(1.0, "px".to_string(), Span::point(0)),
                Expr::Name("solid".to_string(), Span::point(0)),
                Expr::Name("brand".to_string(), Span::point(0)),
            ],
        );
        assert_eq!(value_js(&border, &scope), "`1px solid ${brand()}`");
    }

    #[test]
    fn a_quoted_value_keeps_its_text_and_drops_its_quotes() {
        let text = css("box font \"Inter, system-ui, sans-serif\"\n");
        assert!(
            text.contains("font-family: Inter, system-ui, sans-serif;"),
            "{}",
            generated(&text)
        );
    }

    #[test]
    fn content_keeps_its_quotes_because_css_needs_them() {
        let text = css("box content \"hello\"\n");
        assert!(text.contains("content: \"hello\";"), "{}", generated(&text));
    }

    #[test]
    fn minifying_removes_whitespace_but_not_strings() {
        let pretty =
            css("box font \"Inter, system-ui\" padding 1rem\n  when hover\n    color red\n");
        let minified = minify_css(&pretty);
        assert!(!minified.contains('\n'), "{minified}");
        assert!(!minified.contains("/*"), "{minified}");
        // A quoted string in the base stylesheet survives intact.
        assert!(minified.contains("\"Segoe UI\""), "{minified}");
        assert!(minified.contains(".p1:hover{color:red;}"), "{minified}");
        // A comma is punctuation, so the space after it goes; the value is still
        // the same font stack.
        assert!(
            minified.contains("font-family:Inter,system-ui;"),
            "{minified}"
        );
    }

    #[test]
    fn a_component_use_with_styles_gets_a_class_of_its_own() {
        let source = "component Card\n  text \"hi\"\nmain\n  Card\n    gap 1rem\n";
        let (program, styles) = compiled(source);
        let main = first(&program);
        assert_eq!(class_of(&program.body[0]), None, "main has no styles");
        match main.children.first() {
            Some(Node::Use(card)) => assert_eq!(card.class.as_deref(), Some("p1")),
            other => panic!("expected a component use, got {other:?}"),
        }
        assert!(styles.stylesheet(false).contains("gap: 1rem;"));
    }
}
