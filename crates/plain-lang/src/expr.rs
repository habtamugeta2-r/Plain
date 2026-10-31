//! Plain expressions become JavaScript expressions.
//!
//! The translation is one recursive function over the expression tree, and it is
//! total: every [`Expr`] produces something. A name the compiler has never heard
//! of becomes a JavaScript identifier, so the browser reports it and the build
//! still finishes — the checker has already said what it thinks of it.
//!
//! # Reactivity
//!
//! Plain state is an accessor. `let count = 0` becomes `const count =
//! P.signal(0)`, so *reading* the value means calling it. That is what [`Scope`]
//! records: how each name in scope is read. [`to_js`] asks before it emits an
//! identifier.
//!
//! [`is_dynamic`] then answers the only other question code generation has about
//! an expression: does reading it subscribe to something? If it does, the value
//! has to reach the runtime as an accessor — `() => ...` — so the effect reading
//! it runs again when the state changes. If it does not, it is emitted as a
//! literal and written to the DOM once, which is why a page of static text costs
//! nothing at runtime and needs no diffing.
//!
//! # Interpolation
//!
//! `"~(count) items"` becomes a template literal, and each interpolated value
//! goes through `P.text`, so a value that has not arrived yet shows as nothing
//! instead of the word `undefined`. Plain has no parentheses on the page; it
//! should not have `undefined` on it either.

use std::collections::HashMap;

use crate::ast::{Expr, Path, PathStep, StrPart};
use crate::parser::trim_number;
use crate::tables;

/// How a name is read in generated code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// A `let`, a `derive`, a component prop or an `each` row: reactive state,
    /// which the runtime hands out as an accessor, so reading it calls it.
    Accessor,
    /// A Plain `fn` or a runtime function: called directly, and assumed to read
    /// state of its own.
    Function,
    /// A lambda parameter or an `each` key: an ordinary JavaScript value.
    Value,
}

/// The names in scope, and how each one is read.
///
/// A scope is a small map that code generation clones as it walks into a
/// component, a row, a branch or a lambda. Cloning is cheap — a handful of names
/// — and it means a generator never has to remember to pop what it pushed.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    bindings: HashMap<String, Binding>,
}

impl Scope {
    /// A scope with nothing in it.
    pub fn new() -> Self {
        Scope::default()
    }

    /// A copy of this scope, to extend without disturbing the caller.
    pub fn child(&self) -> Scope {
        self.clone()
    }

    /// Records how `name` is read.
    pub fn bind(&mut self, name: &str, binding: Binding) -> &mut Self {
        self.bindings.insert(name.to_string(), binding);
        self
    }

    /// How `name` is read, if it is in scope at all.
    pub fn binding(&self, name: &str) -> Option<Binding> {
        self.bindings.get(name).copied()
    }

    /// True when `name` is declared here.
    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    /// Every name in scope, sorted. For diagnostics, which must not depend on
    /// the order a hash map happens to iterate in.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.bindings.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

/// Words JavaScript will not accept as a binding name.
///
/// Plain reserves its own vocabulary and says nothing about JavaScript's, so a
/// program may declare a variable called `class` or `new`. Generated code
/// prefixes those with an underscore instead of emitting something a browser
/// refuses to parse. The mangling is a pure function of the name, so every
/// reference to the same variable agrees on it.
const JS_RESERVED: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Runtime namespaces a bare name can reach into: `str.upper(x)`,
/// `list.sort(xs)`, `device.width`, `store.get("theme")`.
const NAMESPACES: &[&str] = &[
    "str",
    "list",
    "num",
    "color",
    "datetime",
    "device",
    "store",
    "session",
    "clipboard",
];

/// Namespaces whose members are reactive, so a read of one has to be wrapped in
/// an accessor. `device.width` is a getter over a signal.
const REACTIVE_NAMESPACES: &[&str] = &["device"];

/// The name JavaScript will accept for a Plain identifier.
pub fn js_ident(name: &str) -> String {
    if JS_RESERVED.contains(&name) {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// True when `name` can be written after a dot without quoting it.
fn is_js_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// A double-quoted JavaScript string literal.
pub fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32))
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The literal text of a string, escaped for a template literal.
///
/// Every `$` is escaped, not just the ones before a brace: a `$` that means
/// nothing to Plain should not start meaning something to JavaScript.
fn template_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// Plain spells verbs in kebab-case; JavaScript spells them in camelCase.
fn camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut capitalise = false;
    for character in name.chars() {
        if character == '-' {
            capitalise = true;
        } else if capitalise {
            out.extend(character.to_uppercase());
            capitalise = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// Translates `expr` into a JavaScript expression.
pub fn to_js(expr: &Expr, scope: &Scope) -> String {
    match expr {
        Expr::Int(value, _) => value.to_string(),
        Expr::Float(value, _) => trim_number(*value),
        // A length is a string with a unit on it: `12px` is not a number.
        Expr::Length(value, unit, _) => js_string(&format!("{}{unit}", trim_number(*value))),
        Expr::Color(value, _) => js_string(value),
        Expr::Bool(value, _) => value.to_string(),
        Expr::None(_) => "undefined".to_string(),
        Expr::Str(parts, _) => string(parts, scope),
        Expr::Name(name, _) => name_js(name, scope),
        Expr::Member(target, field, _) => member(to_js(target, scope), field),
        Expr::Index(target, index, _) => {
            format!("{}[{}]", to_js(target, scope), to_js(index, scope))
        }
        Expr::Call(callee, args, _) => {
            format!("{}({})", to_js(callee, scope), arguments(args, scope))
        }
        Expr::Method(target, name, args, _) => method(target, name, args, scope),
        Expr::Unary(operator, operand, _) => {
            let js = if operator == "not" {
                "!"
            } else {
                operator.as_str()
            };
            format!("({js}{})", to_js(operand, scope))
        }
        Expr::Binary(operator, left, right, _) => binary(operator, left, right, scope),
        Expr::Pipe(left, right, _) => pipe(left, right, scope),
        Expr::Conditional(condition, then, otherwise, _) => format!(
            "({} ? {} : {})",
            to_js(condition, scope),
            to_js(then, scope),
            to_js(otherwise, scope)
        ),
        Expr::Lambda(params, body, _) => {
            // A lambda parameter shadows anything of the same name outside it,
            // and inside the body it is a plain value.
            let mut inner = scope.child();
            for param in params {
                inner.bind(param, Binding::Value);
            }
            let names = params
                .iter()
                .map(|param| js_ident(param))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({names}) => {}", to_js(body, &inner))
        }
        Expr::List(items, _) => format!("[{}]", arguments(items, scope)),
        Expr::Record(fields, _) => record(fields, scope),
        Expr::Spread(inner, _) => format!("...{}", to_js(inner, scope)),
        Expr::Range(from, to, _) => {
            format!("P.range({}, {})", to_js(from, scope), to_js(to, scope))
        }
    }
}

/// True when reading `expr` subscribes to reactive state, and so the value has
/// to reach the runtime as an accessor rather than as a literal.
pub fn is_dynamic(expr: &Expr, scope: &Scope) -> bool {
    match expr {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Length(..)
        | Expr::Color(..)
        | Expr::Bool(..)
        | Expr::None(..) => false,
        // A function value is passed, not evaluated; whatever it reads, it
        // reads when it is called.
        Expr::Lambda(..) => false,
        Expr::Name(name, _) => scope.binding(name) == Some(Binding::Accessor),
        Expr::Str(parts, _) => parts.iter().any(|part| match part {
            StrPart::Expr(inner) => is_dynamic(inner, scope),
            StrPart::Text(_) => false,
        }),
        Expr::Member(target, _, _) => {
            is_dynamic(target, scope)
                || matches!(target, Expr::Name(name) if REACTIVE_NAMESPACES.contains(&name.as_str()))
        }
        Expr::Unary(_, operand, _) | Expr::Spread(operand, _) => is_dynamic(operand, scope),
        Expr::Index(target, index, _) => is_dynamic(target, scope) || is_dynamic(index, scope),
        Expr::Binary(_, left, right, _) => is_dynamic(left, scope) || is_dynamic(right, scope),
        Expr::Pipe(left, right, _) => is_dynamic(left, scope) || is_dynamic(right, scope),
        Expr::Range(from, to, _) => is_dynamic(from, scope) || is_dynamic(to, scope),
        Expr::Conditional(condition, then, otherwise, _) => {
            is_dynamic(condition, scope) || is_dynamic(then, scope) || is_dynamic(otherwise, scope)
        }
        Expr::List(items, _) => items.iter().any(|item| is_dynamic(item, scope)),
        Expr::Record(fields, _) => fields.iter().any(|(_, value)| is_dynamic(value, scope)),
        Expr::Method(target, _, args, _) => {
            is_dynamic(target, scope) || args.iter().any(|arg| is_dynamic(arg, scope))
        }
        Expr::Call(callee, args, _) => {
            // A call to a Plain function is treated as dynamic even when its
            // arguments are not: the function may read state of its own, and
            // v0.1 does not analyse bodies to find out. Getting this wrong the
            // other way costs one effect; getting it wrong this way means a page
            // that quietly stops updating.
            let calls_a_function = matches!(callee, Expr::Name(name)
                if scope.binding(name) == Some(Binding::Function));
            calls_a_function
                || is_dynamic(callee, scope)
                || args.iter().any(|a| is_dynamic(a, scope))
        }
    }
}

/// `expr` as the runtime wants it: an accessor when it reads state, and the
/// value itself when it does not.
///
/// Every reactive runtime helper in Plain accepts either, so generated code says
/// which one it means and pays for an effect only where one is needed.
pub fn accessor(expr: &Expr, scope: &Scope) -> String {
    let js = to_js(expr, scope);
    if is_dynamic(expr, scope) {
        format!("() => {js}")
    } else {
        js
    }
}

/// The JavaScript for an assignment target: the signal to write, and the keys
/// below it.
///
/// `user.name` gives `("user", ["\"name\""])` and `items[i].done` gives
/// `("items", ["i()", "\"done\""])`. The runtime copies every level on the way
/// down, so a nested write is still one signal update.
pub fn assignment(path: &Path, scope: &Scope) -> (String, Vec<String>) {
    let keys = path
        .steps
        .iter()
        .map(|step| match step {
            PathStep::Field(name, _) => js_string(name),
            PathStep::Index(index, _) => to_js(index, scope),
        })
        .collect();
    (js_ident(&path.root), keys)
}

fn arguments(args: &[Expr], scope: &Scope) -> String {
    args.iter()
        .map(|arg| to_js(arg, scope))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a bare name is read.
fn name_js(name: &str, scope: &Scope) -> String {
    match name {
        "true" => return "true".to_string(),
        "false" => return "false".to_string(),
        "none" | "nothing" => return "undefined".to_string(),
        _ => {}
    }
    // Anything declared in this file wins: a variable called `str` is the
    // programmer's, not the runtime's.
    if let Some(binding) = scope.binding(name) {
        return match binding {
            Binding::Accessor => format!("{}()", js_ident(name)),
            Binding::Function | Binding::Value => js_ident(name),
        };
    }
    if let Some(path) = tables::builtin(name) {
        return format!("P.{path}");
    }
    if NAMESPACES.contains(&name) {
        return format!("P.{name}");
    }
    js_ident(name)
}

fn member(target: String, field: &str) -> String {
    // Plain allows hyphens in names and JavaScript does not allow them after a
    // dot, so `user.first-name` becomes a computed member.
    if is_js_ident(field) {
        format!("{target}.{field}")
    } else {
        format!("{target}[{}]", js_string(field))
    }
}

fn method(target: &Expr, name: &str, args: &[Expr], scope: &Scope) -> String {
    let receiver = to_js(target, scope);
    match tables::method(name) {
        // A Plain verb: the value is the first argument, which is what lets the
        // same verb work on a string, a list or a number.
        Some(path) => {
            let mut all = vec![receiver];
            all.extend(args.iter().map(|arg| to_js(arg, scope)));
            format!("P.{path}({})", all.join(", "))
        }
        // Not a Plain verb, so an ordinary JavaScript method. The checker has
        // already said whether it knows the name.
        None => format!("{}.{}({})", receiver, camel(name), arguments(args, scope)),
    }
}

fn binary(operator: &str, left: &Expr, right: &Expr, scope: &Scope) -> String {
    // Comparing with `none` uses JavaScript's loose check, so a value that came
    // back `null` from an API and a value that never arrived both compare equal.
    let nothing = matches!(left, Expr::None(_)) || matches!(right, Expr::None(_));
    if nothing && matches!(operator, "==" | "!=") {
        let js = if operator == "==" { "==" } else { "!=" };
        return format!("({} {js} null)", to_js(left, scope), to_js(right, scope));
    }
    let js = match operator {
        "and" => "&&",
        "or" => "||",
        "==" => "===",
        "!=" => "!==",
        other => other,
    };
    format!("({} {js} {})", to_js(left, scope), to_js(right, scope))
}

fn pipe(left: &Expr, right: &Expr, scope: &Scope) -> String {
    // `name | upper` is `upper(name)`, and `name | take(3)` is `take(name, 3)`:
    // the piped value is always the first argument.
    let value = to_js(left, scope);
    match right {
        Expr::Call(callee, args, _) => {
            let mut all = vec![value];
            all.extend(args.iter().map(|arg| to_js(arg, scope)));
            format!("{}({})", function_of(callee, scope), all.join(", "))
        }
        other => format!("{}({value})", function_of(other, scope)),
    }
}

/// The function a pipe applies, resolved like any other name.
fn function_of(expr: &Expr, scope: &Scope) -> String {
    match expr {
        Expr::Name(name, _) => match tables::pipe_function(name) {
            Some(path) => format!("P.{path}"),
            None => name_js(name, scope),
        },
        other => to_js(other, scope),
    }
}

fn record(fields: &[(String, Expr)], scope: &Scope) -> String {
    if fields.is_empty() {
        return "{}".to_string();
    }
    let entries = fields
        .iter()
        .map(|(key, value)| {
            let key_js = if is_js_ident(key) {
                key.clone()
            } else {
                js_string(key)
            };
            format!("{key_js}: {}", to_js(value, scope))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {entries} }}")
}

fn string(parts: &[StrPart], scope: &Scope) -> String {
    if parts.iter().all(|part| matches!(part, StrPart::Text(_))) {
        let text: String = parts
            .iter()
            .filter_map(|part| match part {
                StrPart::Text(text) => Some(text.as_str()),
                StrPart::Expr(_) => None,
            })
            .collect();
        return js_string(&text);
    }
    let mut out = String::from("`");
    for part in parts {
        match part {
            StrPart::Text(text) => out.push_str(&template_text(text)),
            // Through P.text, so a value that has not arrived shows as nothing
            // instead of the word "undefined".
            StrPart::Expr(inner) => out.push_str(&format!("${{P.text({})}}", to_js(inner, scope))),
        }
    }
    out.push('`');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Node;
    use crate::parser::parse_source;
    use plain_core::FileId;

    /// The expression in `text <source>`, so the tests read like the language.
    fn expr(source: &str) -> Expr {
        let (program, diagnostics) = parse_source(FileId(0), &format!("text {source}\n"));
        assert!(diagnostics.is_empty(), "{diagnostics:?} for `{source}`");
        match program.body.first() {
            Some(Node::Element(element)) => match element.args.first() {
                Some(argument) => argument.clone(),
                None => panic!("`{source}` produced no content"),
            },
            other => panic!("expected an element, got {other:?}"),
        }
    }

    fn js(source: &str) -> String {
        to_js(&expr(source), &Scope::new())
    }

    /// `js` with `names` bound as reactive state, which is how a `let` reads.
    fn stateful(source: &str, names: &[&str]) -> String {
        let mut scope = Scope::new();
        for name in names {
            scope.bind(name, Binding::Accessor);
        }
        to_js(&expr(source), &scope)
    }

    #[test]
    fn literals_become_javascript_literals() {
        assert_eq!(js("42"), "42");
        assert_eq!(js("2.5"), "2.5");
        assert_eq!(js("true"), "true");
        assert_eq!(js("false"), "false");
        assert_eq!(js("none"), "undefined");
        assert_eq!(js("#4f46e5"), "\"#4f46e5\"");
        assert_eq!(js("\"hello\""), "\"hello\"");
    }

    #[test]
    fn a_length_is_a_string_because_it_is_not_a_number() {
        assert_eq!(js("12px"), "\"12px\"");
        assert_eq!(js("1.5rem"), "\"1.5rem\"");
        assert_eq!(js("100%"), "\"100%\"");
        assert_eq!(js("300ms"), "\"300ms\"");
    }

    #[test]
    fn strings_are_quoted_and_escaped() {
        assert_eq!(js_string("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(js_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(js_string("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(js_string("tab\there"), "\"tab\\there\"");
        // Non-ASCII passes through; the file is UTF-8 and so is the browser.
        assert_eq!(js_string("ሰላም"), "\"ሰላም\"");
    }

    #[test]
    fn interpolation_becomes_a_template_literal() {
        assert_eq!(js("\"~(2 + 2)\""), "`${P.text((2 + 2))}`");
        assert_eq!(
            stateful("\"~(count) items\"", &["count"]),
            "`${P.text(count())} items`"
        );
        assert_eq!(
            stateful("\"a ~(x) b ~(y) c\"", &["x", "y"]),
            "`a ${P.text(x())} b ${P.text(y())} c`"
        );
        // Nothing in a template literal may start meaning something else.
        assert_eq!(js("\"costs $5 and `ticks`\""), "\"costs $5 and `ticks`\"");
    }

    #[test]
    fn state_is_read_by_calling_it() {
        assert_eq!(stateful("count", &["count"]), "count()");
        assert_eq!(stateful("user.name", &["user"]), "user().name");
        assert_eq!(stateful("items[0]", &["items"]), "items()[0]");
        // Without a binding the name is left alone: the checker reports it.
        assert_eq!(js("count"), "count");
    }

    #[test]
    fn reserved_javascript_words_are_mangled_consistently() {
        assert_eq!(js_ident("class"), "_class");
        assert_eq!(js_ident("new"), "_new");
        assert_eq!(js_ident("count"), "count");
        let mut scope = Scope::new();
        scope.bind("new", Binding::Accessor);
        assert_eq!(to_js(&expr("new"), &scope), "_new()");
    }

    #[test]
    fn operators_map_onto_javascript_operators() {
        assert_eq!(js("1 + 2"), "(1 + 2)");
        assert_eq!(js("a - b"), "(a - b)");
        assert_eq!(js("a * b"), "(a * b)");
        assert_eq!(js("a / b"), "(a / b)");
        assert_eq!(js("a % b"), "(a % b)");
        assert_eq!(js("1 == 2"), "(1 === 2)");
        assert_eq!(js("1 != 2"), "(1 !== 2)");
        assert_eq!(js("a < b"), "(a < b)");
        assert_eq!(js("a and b"), "(a && b)");
        assert_eq!(js("a or b"), "(a || b)");
        assert_eq!(js("not a"), "(!a)");
        assert_eq!(js("-5"), "(-5)");
    }

    #[test]
    fn comparing_with_nothing_is_a_null_check() {
        // A value that came back null and a value that never arrived are the
        // same thing on a page.
        assert_eq!(js("user == none"), "(user == null)");
        assert_eq!(js("user != none"), "(user != null)");
        assert_eq!(js("none == user"), "(undefined == null)");
    }

    #[test]
    fn a_conditional_expression_becomes_a_ternary() {
        assert_eq!(
            stateful("if count == 1 then \"one\" else \"many\"", &["count"]),
            "((count() === 1) ? \"one\" : \"many\")"
        );
    }

    #[test]
    fn lists_records_and_spreads() {
        assert_eq!(js("[1, 2, 3]"), "[1, 2, 3]");
        assert_eq!(
            js("record(name: \"Ada\", age: 30)"),
            "{ name: \"Ada\", age: 30 }"
        );
        assert_eq!(js("record()"), "{}");
        // A hyphenated key is quoted: JavaScript will not take it bare.
        assert_eq!(
            js("record(first-name: \"Ada\")"),
            "{ \"first-name\": \"Ada\" }"
        );
        assert_eq!(stateful("[...items]", &["items"]), "[...items()]");
    }

    #[test]
    fn member_access_quotes_what_javascript_will_not_take() {
        assert_eq!(stateful("user.name", &["user"]), "user().name");
        assert_eq!(
            stateful("user.first-name", &["user"]),
            "user()[\"first-name\"]"
        );
        // Reserved words are fine after a dot.
        assert_eq!(stateful("field.class", &["field"]), "field().class");
    }

    #[test]
    fn calls_and_methods_resolve_through_the_runtime() {
        assert_eq!(js("greet(\"Ada\")"), "greet(\"Ada\")");
        assert_eq!(
            js("format-date(today, \"en\")"),
            "P.formatDate(today, \"en\")"
        );
        // A Plain verb takes the receiver as its first argument.
        assert_eq!(js("\"abc\".upper()"), "P.str.upper(\"abc\")");
        assert_eq!(
            js("\"abc\".starts-with(\"a\")"),
            "P.str.startsWith(\"abc\", \"a\")"
        );
        // Not a Plain verb, so an ordinary JavaScript method, camelCased.
        assert_eq!(js("items.to-json()"), "items.toJSON()");
    }

    #[test]
    fn a_pipe_puts_the_value_first() {
        assert_eq!(js("\"abc\" | upper"), "P.str.upper(\"abc\")");
        assert_eq!(js("[1, 2] | take(1)"), "P.list.take([1, 2], 1)");
        assert_eq!(stateful("name | upper", &["name"]), "P.str.upper(name())");
    }

    #[test]
    fn lambdas_bind_their_own_parameters() {
        assert_eq!(js("x => x"), "(x) => x");
        assert_eq!(js("(a, b) => a"), "(a, b) => a");
        // A parameter shadows state of the same name.
        assert_eq!(stateful("count => count", &["count"]), "(count) => count");
    }

    #[test]
    fn ranges_call_the_runtime_helper() {
        assert_eq!(js("1..5"), "P.range(1, 5)");
        assert_eq!(stateful("0..total", &["total"]), "P.range(0, total())");
    }

    #[test]
    fn a_device_read_is_dynamic_because_its_fields_are_signals() {
        let value = expr("device.width");
        assert_eq!(to_js(&value, &Scope::new()), "P.device.width");
        assert!(is_dynamic(&value, &Scope::new()));
        // A variable of the same name is the programmer's, and reads as state.
        let mut scope = Scope::new();
        scope.bind("device", Binding::Accessor);
        assert_eq!(to_js(&value, &scope), "device().width");
    }

    #[test]
    fn only_expressions_that_read_state_are_dynamic() {
        let mut scope = Scope::new();
        scope.bind("count", Binding::Accessor);
        scope.bind("total", Binding::Function);

        assert!(!is_dynamic(&expr("1 + 2"), &scope));
        assert!(!is_dynamic(&expr("\"hello\""), &scope));
        assert!(is_dynamic(&expr("count"), &scope));
        assert!(is_dynamic(&expr("count + 1"), &scope));
        assert!(is_dynamic(&expr("\"~(count)\""), &scope));
        assert!(is_dynamic(&expr("[1, count]"), &scope));
        assert!(is_dynamic(&expr("record(a: count)"), &scope));
        // A call to a Plain function is dynamic: it may read state itself.
        assert!(is_dynamic(&expr("total()"), &scope));
        // A lambda is a value, not a read.
        assert!(!is_dynamic(&expr("x => count"), &scope));
    }

    #[test]
    fn an_accessor_is_only_added_where_one_is_needed() {
        let mut scope = Scope::new();
        scope.bind("count", Binding::Accessor);
        assert_eq!(accessor(&expr("\"hello\""), &scope), "\"hello\"");
        assert_eq!(accessor(&expr("count + 1"), &scope), "() => (count() + 1)");
    }

    #[test]
    fn an_assignment_target_splits_into_a_signal_and_its_keys() {
        let mut scope = Scope::new();
        scope.bind("user", Binding::Accessor);
        scope.bind("items", Binding::Accessor);
        scope.bind("i", Binding::Accessor);

        let (root, keys) = assignment(&path("user.name = \"Ada\""), &scope);
        assert_eq!(root, "user");
        assert_eq!(keys, vec!["\"name\"".to_string()]);

        let (root, keys) = assignment(&path("items[i].done = true"), &scope);
        assert_eq!(root, "items");
        assert_eq!(keys, vec!["i()".to_string(), "\"done\"".to_string()]);

        let (root, keys) = assignment(&path("count = 0"), &scope);
        assert_eq!(root, "count");
        assert!(keys.is_empty());
    }

    /// The target of an assignment, parsed out of a statement.
    fn path(source: &str) -> Path {
        let (program, diagnostics) = parse_source(FileId(0), &format!("{source}\n"));
        assert!(diagnostics.is_empty(), "{diagnostics:?} for `{source}`");
        match program.body.first() {
            Some(Node::Assign(assign)) => assign.target.clone(),
            other => panic!("expected an assignment, got {other:?}"),
        }
    }

    #[test]
    fn a_scope_answers_what_is_in_it() {
        let mut scope = Scope::new();
        assert!(!scope.contains("count"));
        scope.bind("count", Binding::Accessor);
        scope.bind("reset", Binding::Function);
        assert_eq!(scope.binding("count"), Some(Binding::Accessor));
        assert_eq!(scope.binding("reset"), Some(Binding::Function));
        assert_eq!(scope.binding("other"), None);

        // A child inherits and can shadow, without disturbing its parent.
        let mut child = scope.child();
        child.bind("count", Binding::Value);
        assert_eq!(child.binding("count"), Some(Binding::Value));
        assert_eq!(scope.binding("count"), Some(Binding::Accessor));

        // Names come back sorted, so a diagnostic that lists them is stable.
        assert_eq!(scope.names(), vec!["count", "reset"]);
    }
}
