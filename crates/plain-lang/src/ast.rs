//! The Plain abstract syntax tree.
//!
//! Everything is owned and `Clone`: the tree is built once, walked a handful of
//! times by the semantic pass and the code generators, and thrown away. Sharing
//! with `Rc` would buy nothing here and would make the borrow checker the
//! hardest part of the compiler.

use crate::tables::ElementDef;
use plain_core::{FileId, Span};

/// A whole `.pln` file.
#[derive(Debug, Clone)]
pub struct Program {
    pub file: FileId,
    /// At most one `theme` block per file.
    pub theme: Option<Theme>,
    /// Component declarations, in source order.
    pub components: Vec<Component>,
    /// Page-level UI and declarations: everything that is not inside a
    /// `component` or `theme`.
    pub body: Vec<Node>,
    /// Constructs that parsed cleanly but are not implemented yet. Reported as
    /// diagnostics so the message says *when*, not *what is wrong with you*.
    pub deferred: Vec<DeferredUse>,
}

impl Program {
    pub fn new(file: FileId) -> Self {
        Program {
            file,
            theme: None,
            components: Vec::new(),
            body: Vec::new(),
            deferred: Vec::new(),
        }
    }
}

/// One thing that can appear in a body.
///
/// UI and statements share a single node type on purpose. Whether `each` means
/// "render this list" or "loop over this list" is decided by position — inside a
/// UI body or inside a handler body — which the semantic pass checks.
#[derive(Debug, Clone)]
pub enum Node {
    Element(Element),
    /// A component invocation.
    Use(Use),
    Let(Let),
    Derive(Derive),
    Fn(FnDecl),
    If(If),
    Each(Each),
    Assign(Assign),
    /// A bare expression statement, normally a function call.
    Expr(Expr),
    Return(Return),
    /// `children` or `slot name` inside a component body.
    Slot(Slot),
    Deferred(DeferredUse),
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Element(n) => n.span,
            Node::Use(n) => n.span,
            Node::Let(n) => n.span,
            Node::Derive(n) => n.span,
            Node::Fn(n) => n.span,
            Node::If(n) => n.span,
            Node::Each(n) => n.span,
            Node::Assign(n) => n.span,
            Node::Expr(e) => e.span(),
            Node::Return(n) => n.span,
            Node::Slot(n) => n.span,
            Node::Deferred(n) => n.span,
        }
    }

    /// True when this node produces DOM output.
    pub fn is_ui(&self) -> bool {
        matches!(
            self,
            Node::Element(_) | Node::Use(_) | Node::If(_) | Node::Each(_) | Node::Slot(_)
        )
    }
}

/// A built-in element, e.g. `button "Save"`.
#[derive(Debug, Clone)]
pub struct Element {
    pub span: Span,
    pub name: String,
    pub def: ElementDef,
    /// Positional arguments on the element line, in order.
    pub args: Vec<Expr>,
    /// Style properties. Emitted as scoped CSS.
    pub props: Vec<StyleProp>,
    /// Properties that become HTML attributes.
    pub attrs: Vec<AttrProp>,
    /// Boolean modifiers such as `fill` or `required`.
    pub modifiers: Vec<Modifier>,
    /// `when hover` style blocks.
    pub pseudos: Vec<PseudoBlock>,
    /// `@tablet` property overrides.
    pub responsive: Vec<ResponsiveBlock>,
    /// `on click` handlers.
    pub handlers: Vec<Handler>,
    /// `bind name`.
    pub bind: Option<BindTarget>,
    /// `show expr`.
    pub show: Option<Expr>,
    pub children: Vec<Node>,
    /// Filled in by the CSS pass: the generated scope class.
    pub class: Option<String>,
}

impl Element {
    pub fn has_modifier(&self, name: &str) -> bool {
        self.modifiers.iter().any(|m| m.name == name)
    }

    /// The first positional argument, if any.
    pub fn primary(&self) -> Option<&Expr> {
        self.args.first()
    }

    pub fn attr(&self, name: &str) -> Option<&AttrProp> {
        self.attrs.iter().find(|a| a.name == name)
    }

    pub fn prop(&self, name: &str) -> Option<&StyleProp> {
        self.props.iter().find(|p| p.name == name)
    }
}

/// A component invocation, e.g. `Card` followed by prop lines.
#[derive(Debug, Clone)]
pub struct Use {
    pub span: Span,
    pub name: String,
    /// Named props: `title "Hello"`.
    pub props: Vec<(Span, String, Expr)>,
    /// Positional props: `Card "Hello" 3`.
    pub positional: Vec<Expr>,
    pub props_style: Vec<StyleProp>,
    pub attrs: Vec<AttrProp>,
    pub modifiers: Vec<Modifier>,
    pub handlers: Vec<Handler>,
    pub show: Option<Expr>,
    /// Default-slot children.
    pub children: Vec<Node>,
    /// Named slots, keyed by slot name.
    pub slots: Vec<(String, Vec<Node>)>,
    pub class: Option<String>,
}

/// `let count = 0`, optionally `persist "counter"`.
#[derive(Debug, Clone)]
pub struct Let {
    pub span: Span,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: Expr,
    pub persist: Option<Persist>,
}

/// `derive total = price * quantity` — read-only, recomputed from its inputs.
#[derive(Debug, Clone)]
pub struct Derive {
    pub span: Span,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: Expr,
}

/// `fn add_to_cart(product)` with a statement body.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub span: Span,
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub span: Span,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub default: Option<Expr>,
}

/// `if` / `else if` / `else`.
#[derive(Debug, Clone)]
pub struct If {
    pub span: Span,
    /// The first pair is the `if`, the rest are `else if`s.
    pub branches: Vec<(Expr, Vec<Node>)>,
    pub else_branch: Option<Vec<Node>>,
}

/// `each item, index in items key item.id`.
#[derive(Debug, Clone)]
pub struct Each {
    pub span: Span,
    pub item: String,
    pub index: Option<String>,
    pub iter: Expr,
    pub key: Option<Expr>,
    pub body: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct Assign {
    pub span: Span,
    pub target: Path,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Return {
    pub span: Span,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Slot {
    pub span: Span,
    /// `None` for the default slot (`children`).
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeferredUse {
    pub span: Span,
    pub keyword: String,
    pub message: String,
}

/// A dotted/indexed assignment or binding target.
#[derive(Debug, Clone)]
pub struct Path {
    pub span: Span,
    pub root: String,
    pub steps: Vec<PathStep>,
}

#[derive(Debug, Clone)]
pub enum PathStep {
    Field(String, Span),
    Index(Expr, Span),
}

#[derive(Debug, Clone)]
pub struct Persist {
    pub span: Span,
    pub key: Expr,
    pub session: bool,
}

#[derive(Debug, Clone)]
pub struct BindTarget {
    pub span: Span,
    pub path: Path,
}

/// A style property: `padding 1rem 2rem`.
#[derive(Debug, Clone)]
pub struct StyleProp {
    pub span: Span,
    pub name: String,
    pub values: Vec<Expr>,
    /// Set by the semantic pass when a value name resolves to a variable or
    /// component prop rather than to a theme token or CSS keyword. A dynamic
    /// property is emitted as an inline style effect instead of a CSS rule.
    pub dynamic: bool,
}

impl StyleProp {
    pub fn new(span: Span, name: String, values: Vec<Expr>) -> Self {
        StyleProp {
            span,
            name,
            values,
            dynamic: false,
        }
    }

    /// True when every value is a compile-time constant, so the property can
    /// live in the stylesheet instead of an inline style.
    pub fn is_static(&self) -> bool {
        !self.dynamic && self.values.iter().all(Expr::is_static)
    }
}

/// A property that becomes an HTML attribute: `placeholder "Search"`.
#[derive(Debug, Clone)]
pub struct AttrProp {
    pub span: Span,
    pub name: String,
    pub values: Vec<Expr>,
    /// Set by the semantic pass; see [`StyleProp::dynamic`].
    pub dynamic: bool,
}

impl AttrProp {
    pub fn new(span: Span, name: String, values: Vec<Expr>) -> Self {
        AttrProp {
            span,
            name,
            values,
            dynamic: false,
        }
    }

    pub fn is_static(&self) -> bool {
        !self.dynamic && self.values.iter().all(Expr::is_static)
    }
}

#[derive(Debug, Clone)]
pub struct Modifier {
    pub span: Span,
    pub name: String,
}

/// `when hover` / `when focus` style overrides.
#[derive(Debug, Clone)]
pub struct PseudoBlock {
    pub span: Span,
    pub name: String,
    pub props: Vec<StyleProp>,
    pub responsive: Vec<ResponsiveBlock>,
}

/// `@tablet` and `@reduced-motion` overrides.
#[derive(Debug, Clone)]
pub struct ResponsiveBlock {
    pub span: Span,
    /// Breakpoint or media-feature name as written.
    pub name: String,
    /// The resolved CSS media query.
    pub query: String,
    pub props: Vec<StyleProp>,
    pub pseudos: Vec<PseudoBlock>,
    pub modifiers: Vec<Modifier>,
    /// `show false` under a breakpoint is common enough to support directly.
    pub show: Option<Expr>,
}

/// `on click.prevent` with a statement body.
#[derive(Debug, Clone)]
pub struct Handler {
    pub span: Span,
    pub event: String,
    /// Key filters, e.g. `enter` from `on keydown.enter`.
    pub keys: Vec<String>,
    /// Modifier-key filters: `ctrl`, `shift`, `alt`, `meta`.
    pub key_mods: Vec<String>,
    /// Behaviour modifiers: `prevent`, `stop`, `self`, `once`.
    pub modifiers: Vec<String>,
    pub body: Vec<Node>,
}

/// A `component Name(props)` declaration.
#[derive(Debug, Clone)]
pub struct Component {
    pub span: Span,
    pub name: String,
    pub props: Vec<PropDecl>,
    pub body: Vec<Node>,
    pub doc: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PropDecl {
    pub span: Span,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub default: Option<Expr>,
}

/// A `theme` block.
#[derive(Debug, Clone)]
pub struct Theme {
    pub span: Span,
    pub groups: Vec<ThemeGroup>,
}

/// One theme category, e.g. `colors`, with optional `dark` overrides nested
/// inside it. Nesting `dark` under a category is what makes an override
/// unambiguous: the category tells the compiler which variable to redefine.
#[derive(Debug, Clone)]
pub struct ThemeGroup {
    pub span: Span,
    pub name: String,
    pub entries: Vec<ThemeEntry>,
    pub dark: Vec<ThemeEntry>,
    /// Custom breakpoints declared here extend the built-in set.
    pub breakpoints: Vec<(String, String, Span)>,
}

#[derive(Debug, Clone)]
pub struct ThemeEntry {
    pub span: Span,
    pub name: String,
    pub values: Vec<Expr>,
}

/// A type annotation. v0.1 parses them and checks literal/prop mismatches;
/// full inference is later work.
#[derive(Debug, Clone)]
pub enum TypeRef {
    Named(String, Span),
    Optional(Box<TypeRef>, Span),
    List(Box<TypeRef>, Span),
    Record(Vec<(String, TypeRef)>, Span),
    Union(Vec<TypeRef>, Span),
    Generic(String, Vec<TypeRef>, Span),
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Named(_, s)
            | TypeRef::Optional(_, s)
            | TypeRef::List(_, s)
            | TypeRef::Record(_, s)
            | TypeRef::Union(_, s)
            | TypeRef::Generic(_, _, s) => *s,
        }
    }

    /// A short, human-readable rendering used in diagnostics.
    pub fn describe(&self) -> String {
        match self {
            TypeRef::Named(name, _) => name.clone(),
            TypeRef::Optional(inner, _) => format!("{}?", inner.describe()),
            TypeRef::List(inner, _) => format!("[{}]", inner.describe()),
            TypeRef::Record(fields, _) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.describe()))
                    .collect();
                format!("record({})", parts.join(", "))
            }
            TypeRef::Union(options, _) => {
                let parts: Vec<String> = options.iter().map(|t| t.describe()).collect();
                parts.join(" | ")
            }
            TypeRef::Generic(name, args, _) => {
                let parts: Vec<String> = args.iter().map(|t| t.describe()).collect();
                format!("{name}<{}>", parts.join(", "))
            }
        }
    }
}

/// One piece of a string literal.
#[derive(Debug, Clone)]
pub enum StrPart {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    /// A number carrying a CSS unit: `12px`, `1.5rem`, `100%`, `300ms`.
    Length(f64, String, Span),
    /// A colour literal: `#4f46e5`.
    Color(String, Span),
    Str(Vec<StrPart>, Span),
    Bool(bool, Span),
    None(Span),
    /// A bare name: variable, theme token, CSS keyword or function.
    Name(String, Span),
    Member(Box<Expr>, String, Span),
    Index(Box<Expr>, Box<Expr>, Span),
    Call(Box<Expr>, Vec<Expr>, Span),
    /// `value.method(args)`; kept distinct so the checker can resolve methods.
    Method(Box<Expr>, String, Vec<Expr>, Span),
    Unary(String, Box<Expr>, Span),
    Binary(String, Box<Expr>, Box<Expr>, Span),
    /// `left | filter(f)`.
    Pipe(Box<Expr>, Box<Expr>, Span),
    /// `if a then b else c`.
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    Lambda(Vec<String>, Box<Expr>, Span),
    List(Vec<Expr>, Span),
    Record(Vec<(String, Expr)>, Span),
    Spread(Box<Expr>, Span),
    /// A `1..5` range, used by `each`.
    Range(Box<Expr>, Box<Expr>, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Length(_, _, s)
            | Expr::Color(_, s)
            | Expr::Str(_, s)
            | Expr::Bool(_, s)
            | Expr::None(s)
            | Expr::Name(_, s)
            | Expr::Member(_, _, s)
            | Expr::Index(_, _, s)
            | Expr::Call(_, _, s)
            | Expr::Method(_, _, _, s)
            | Expr::Unary(_, _, s)
            | Expr::Binary(_, _, _, s)
            | Expr::Pipe(_, _, s)
            | Expr::Conditional(_, _, _, s)
            | Expr::Lambda(_, _, s)
            | Expr::List(_, s)
            | Expr::Record(_, s)
            | Expr::Spread(_, s)
            | Expr::Range(_, _, s) => *s,
        }
    }

    /// True when the value is known at compile time, so it can go in the
    /// stylesheet or the HTML instead of an effect.
    pub fn is_static(&self) -> bool {
        match self {
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Length(..)
            | Expr::Color(..)
            | Expr::Bool(..)
            | Expr::None(_)
            | Expr::Name(..) => true,
            Expr::Str(parts, _) => parts.iter().all(|part| matches!(part, StrPart::Text(_))),
            Expr::Unary(_, inner, _) | Expr::Spread(inner, _) => inner.is_static(),
            Expr::List(items, _) => items.iter().all(Expr::is_static),
            _ => false,
        }
    }

    /// The root identifier of a member/index chain, when there is one.
    pub fn root_name(&self) -> Option<&str> {
        match self {
            Expr::Name(name, _) => Some(name),
            Expr::Member(target, _, _) | Expr::Index(target, _, _) => target.root_name(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Expr {
        Expr::Name(text.to_string(), Span::new(0, text.len() as u32))
    }

    #[test]
    fn static_expressions_are_recognised() {
        assert!(Expr::Int(1, Span::point(0)).is_static());
        assert!(Expr::Length(2.0, "rem".into(), Span::point(0)).is_static());
        assert!(name("center").is_static());
        assert!(Expr::List(vec![name("a"), name("b")], Span::point(0)).is_static());
        assert!(!Expr::Member(Box::new(name("user")), "name".into(), Span::point(0)).is_static());
    }

    #[test]
    fn root_name_walks_chains() {
        let chain = Expr::Index(
            Box::new(Expr::Member(
                Box::new(name("user")),
                "tags".into(),
                Span::point(0),
            )),
            Box::new(Expr::Int(0, Span::point(0))),
            Span::point(0),
        );
        assert_eq!(chain.root_name(), Some("user"));
        assert_eq!(Expr::Int(1, Span::point(0)).root_name(), None);
    }

    #[test]
    fn type_ref_describes_itself() {
        let ty = TypeRef::Optional(
            Box::new(TypeRef::List(
                Box::new(TypeRef::Named("string".into(), Span::point(0))),
                Span::point(0),
            )),
            Span::point(0),
        );
        assert_eq!(ty.describe(), "[string]?");
    }
}
