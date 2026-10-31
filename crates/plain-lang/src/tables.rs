//! The language's built-in vocabulary: elements, style properties,
//! pseudo-states, events, modifiers and breakpoints.
//!
//! Everything here is a *table lookup*, not a hard-coded match in the parser,
//! so adding a keyword never requires touching the grammar.

/// What an element's first positional argument means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgRole {
    /// The argument is the element's text content.
    Content,
    /// The argument is a resource URL (`image`, `video`, `audio`).
    Src,
    /// The argument is a navigation target (`link`).
    Href,
    /// The element takes no positional argument.
    None,
}

/// One built-in element.
#[derive(Debug, Clone, Copy)]
pub struct ElementDef {
    pub name: &'static str,
    /// Emitted HTML tag.
    pub tag: &'static str,
    pub role: ArgRole,
    /// Emits no closing tag.
    pub void: bool,
    /// Renders inline (affects whether `text` becomes `<span>` or `<p>`).
    pub inline: bool,
    /// The compiler can produce a sensible accessible name automatically.
    pub needs_name: bool,
    /// Layout applied by the generated reset class.
    pub layout: Layout,
}

/// Layout the compiler installs for a container element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    None,
    Stack,
    Row,
    Grid,
    Layer,
}

/// Builds an element definition. Kept as a function rather than a `const`
/// table so the vocabulary stays readable and easy to extend.
#[allow(clippy::too_many_arguments)]
fn def(
    name: &'static str,
    tag: &'static str,
    role: ArgRole,
    void: bool,
    inline: bool,
    needs_name: bool,
    layout: Layout,
) -> ElementDef {
    ElementDef {
        name,
        tag,
        role,
        void,
        inline,
        needs_name,
        layout,
    }
}

const TEXT: ArgRole = ArgRole::Content;
const NONE: ArgRole = ArgRole::None;
const SRC: ArgRole = ArgRole::Src;
const HREF: ArgRole = ArgRole::Href;

/// Every element name Plain v0.1 knows, in documentation order.
pub const ELEMENT_NAMES: &[&str] = &[
    "text",
    "heading",
    "strong",
    "emphasis",
    "code",
    "badge",
    "time",
    "quote",
    "pre",
    "caption",
    "label",
    "summary",
    "box",
    "stack",
    "row",
    "grid",
    "layer",
    "section",
    "nav",
    "header",
    "footer",
    "main",
    "aside",
    "article",
    "figure",
    "details",
    "divider",
    "break",
    "spacer",
    "image",
    "video",
    "audio",
    "canvas",
    "link",
    "button",
    "input",
    "check",
    "radio",
    "slider",
    "toggle",
    "select",
    "option",
    "textarea",
    "form",
    "progress",
    "skeleton",
    "modal",
    "list",
    "numbered-list",
    "item",
    "table",
    "table-row",
    "table-cell",
];

/// Looks an element up by name. `None` means the name is not a built-in, so
/// the parser can treat it as a component invocation or report an error.
pub fn element(name: &str) -> Option<ElementDef> {
    use Layout as L;
    let d = match name {
        // Text and inline content.
        "text" => def("text", "p", TEXT, false, false, false, L::None),
        "heading" => def("heading", "h1", TEXT, false, false, false, L::None),
        "strong" => def("strong", "strong", TEXT, false, true, false, L::None),
        "emphasis" => def("emphasis", "em", TEXT, false, true, false, L::None),
        "code" => def("code", "code", TEXT, false, true, false, L::None),
        "badge" => def("badge", "span", TEXT, false, true, false, L::None),
        "time" => def("time", "time", TEXT, false, true, false, L::None),
        "quote" => def("quote", "blockquote", TEXT, false, false, false, L::None),
        "pre" => def("pre", "pre", TEXT, false, false, false, L::None),
        "caption" => def("caption", "figcaption", TEXT, false, false, false, L::None),
        "label" => def("label", "label", TEXT, false, false, false, L::None),
        "summary" => def("summary", "summary", TEXT, false, false, false, L::None),
        // Structure and layout.
        "box" => def("box", "div", NONE, false, false, false, L::None),
        "stack" => def("stack", "div", NONE, false, false, false, L::Stack),
        "row" => def("row", "div", NONE, false, false, false, L::Row),
        "grid" => def("grid", "div", NONE, false, false, false, L::Grid),
        "layer" => def("layer", "div", NONE, false, false, false, L::Layer),
        "section" => def("section", "section", NONE, false, false, false, L::None),
        "nav" => def("nav", "nav", NONE, false, false, false, L::None),
        "header" => def("header", "header", NONE, false, false, false, L::None),
        "footer" => def("footer", "footer", NONE, false, false, false, L::None),
        "main" => def("main", "main", NONE, false, false, false, L::None),
        "aside" => def("aside", "aside", NONE, false, false, false, L::None),
        "article" => def("article", "article", NONE, false, false, false, L::None),
        "figure" => def("figure", "figure", NONE, false, false, false, L::None),
        "details" => def("details", "details", NONE, false, false, false, L::None),
        "divider" => def("divider", "hr", NONE, true, false, false, L::None),
        "break" => def("break", "br", NONE, true, false, false, L::None),
        "spacer" => def("spacer", "div", NONE, false, false, false, L::None),
        // Media.
        "image" => def("image", "img", SRC, true, false, true, L::None),
        "video" => def("video", "video", SRC, false, false, false, L::None),
        "audio" => def("audio", "audio", SRC, false, false, false, L::None),
        "canvas" => def("canvas", "canvas", NONE, false, false, false, L::None),
        // Interactive.
        "link" => def("link", "a", HREF, false, true, true, L::None),
        "button" => def("button", "button", TEXT, false, false, true, L::None),
        "input" => def("input", "input", NONE, true, false, true, L::None),
        "check" => def("check", "input", NONE, true, false, true, L::None),
        "radio" => def("radio", "input", NONE, true, false, true, L::None),
        "slider" => def("slider", "input", NONE, true, false, true, L::None),
        "toggle" => def("toggle", "button", NONE, false, false, true, L::None),
        "select" => def("select", "select", NONE, false, false, true, L::None),
        "option" => def("option", "option", TEXT, false, false, false, L::None),
        "textarea" => def("textarea", "textarea", NONE, false, false, true, L::None),
        "form" => def("form", "form", NONE, false, false, false, L::None),
        "progress" => def("progress", "progress", NONE, false, false, false, L::None),
        "skeleton" => def("skeleton", "div", NONE, false, false, false, L::None),
        "modal" => def("modal", "div", NONE, false, false, true, L::None),
        // Collections.
        "list" => def("list", "ul", NONE, false, false, false, L::None),
        "numbered-list" => def("numbered-list", "ol", NONE, false, false, false, L::None),
        "item" => def("item", "li", NONE, false, false, false, L::None),
        "table" => def("table", "table", NONE, false, false, false, L::None),
        "table-row" => def("table-row", "tr", NONE, false, false, false, L::None),
        "table-cell" => def("table-cell", "td", TEXT, false, false, false, L::None),
        _ => return None,
    };
    Some(d)
}

/// Elements whose text argument is rendered inline.
pub fn is_text_element(name: &str) -> bool {
    matches!(
        name,
        "text" | "heading" | "strong" | "emphasis" | "code" | "badge" | "label" | "link" | "button"
    )
}

/// Elements that accept `bind`.
pub const BINDABLE: &[&str] = &[
    "input", "check", "radio", "slider", "select", "toggle", "textarea",
];

/// Boolean modifiers that may appear on an element line.
pub const MODIFIERS: &[&str] = &[
    "fill",
    "fill-x",
    "fill-y",
    "required",
    "optional",
    "disabled",
    "readonly",
    "checked",
    "selected",
    "multiple",
    "autofocus",
    "autoplay",
    "muted",
    "loop",
    "controls",
    "decorative",
    "head",
    "open",
    "hidden",
    "indeterminate",
];

/// CSS pseudo-classes and pseudo-elements, written `when hover`.
pub const PSEUDOS: &[(&str, &str)] = &[
    ("hover", ":hover"),
    ("focus", ":focus-visible"),
    ("focus-any", ":focus"),
    ("focus-within", ":focus-within"),
    ("active", ":active"),
    ("disabled", ":disabled"),
    ("enabled", ":enabled"),
    ("visited", ":visited"),
    ("checked", ":checked"),
    ("invalid", ":invalid"),
    ("valid", ":valid"),
    ("required", ":required"),
    ("read-only", ":read-only"),
    ("first", ":first-child"),
    ("last", ":last-child"),
    ("odd", ":nth-child(odd)"),
    ("even", ":nth-child(even)"),
    ("empty", ":empty"),
    ("target", ":target"),
    ("placeholder", "::placeholder"),
    ("before", "::before"),
    ("after", "::after"),
    ("selection", "::selection"),
    ("marker", "::marker"),
];

/// Resolves a breakpoint or media-feature name to its media query condition.
///
/// Returns `None` for a name Plain does not know. A `theme` block may declare
/// custom breakpoints, so the semantic pass — not the parser — decides whether
/// an unresolved name is an error.
pub fn media_query(name: &str) -> Option<&'static str> {
    if let Some((_, query)) = BREAKPOINTS.iter().find(|(key, _)| *key == name) {
        return Some(query);
    }
    MEDIA_FEATURES
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, query)| *query)
}

/// Looks a pseudo-state up.
pub fn pseudo(name: &str) -> Option<&'static str> {
    PSEUDOS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, selector)| *selector)
}

/// Built-in breakpoints. `mobile` is the only max-width query; the rest are
/// mobile-first `min-width` queries.
pub const BREAKPOINTS: &[(&str, &str)] = &[
    ("mobile", "(max-width: 639px)"),
    ("tablet", "(min-width: 640px)"),
    ("laptop", "(min-width: 1024px)"),
    ("desktop", "(min-width: 1280px)"),
    ("wide", "(min-width: 1536px)"),
];

/// Media features usable in place of a breakpoint name.
pub const MEDIA_FEATURES: &[(&str, &str)] = &[
    ("reduced-motion", "(prefers-reduced-motion: reduce)"),
    ("motion", "(prefers-reduced-motion: no-preference)"),
    ("dark", "(prefers-color-scheme: dark)"),
    ("light", "(prefers-color-scheme: light)"),
    ("print", "print"),
    ("portrait", "(orientation: portrait)"),
    ("landscape", "(orientation: landscape)"),
];

/// DOM events addressable with `on`.
pub const EVENTS: &[&str] = &[
    "click",
    "dblclick",
    "input",
    "change",
    "submit",
    "reset",
    "keydown",
    "keyup",
    "keypress",
    "focus",
    "blur",
    "focusin",
    "focusout",
    "mouseenter",
    "mouseleave",
    "mouseover",
    "mouseout",
    "mousedown",
    "mouseup",
    "mousemove",
    "wheel",
    "drag",
    "dragstart",
    "dragend",
    "dragenter",
    "dragover",
    "dragleave",
    "drop",
    "touchstart",
    "touchend",
    "touchmove",
    "touchcancel",
    "pointerdown",
    "pointerup",
    "pointermove",
    "pointerenter",
    "pointerleave",
    "contextmenu",
    "copy",
    "cut",
    "paste",
    "load",
    "error",
    "toggle",
    "animationend",
    "animationstart",
    "transitionend",
    "select",
    "invalid",
    "play",
    "pause",
    "ended",
    "timeupdate",
];

/// Event handler modifiers.
pub const EVENT_MODIFIERS: &[&str] = &["prevent", "stop", "self", "once", "passive", "capture"];

/// Key filters, written `on keydown.enter`.
pub const KEY_FILTERS: &[(&str, &str)] = &[
    ("enter", "Enter"),
    ("escape", "Escape"),
    ("esc", "Escape"),
    ("tab", "Tab"),
    ("space", " "),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("home", "Home"),
    ("end", "End"),
    ("page-up", "PageUp"),
    ("page-down", "PageDown"),
    ("delete", "Delete"),
    ("backspace", "Backspace"),
    ("insert", "Insert"),
];

/// Modifier-key filters, written `on keydown.ctrl.s`.
pub const KEY_MODIFIERS: &[&str] = &["ctrl", "shift", "alt", "meta"];

/// Reserved words. A user identifier may not be one of these.
pub const RESERVED: &[&str] = &[
    "let",
    "derive",
    "fn",
    "component",
    "route",
    "layout",
    "guard",
    "theme",
    "import",
    "export",
    "if",
    "else",
    "each",
    "in",
    "match",
    "when",
    "on",
    "bind",
    "show",
    "key",
    "persist",
    "global",
    "shared",
    "slot",
    "children",
    "true",
    "false",
    "none",
    "and",
    "or",
    "not",
    "then",
    "return",
    "await",
    "async",
    "validate",
    "fetch",
    "stream",
    "debug",
    "assert",
    "catch",
    "fallback",
    "enter",
    "exit",
];

/// Language features that are specified but not implemented in v0.1. The
/// compiler reports a precise "not yet" diagnostic instead of a parse error.
pub const DEFERRED: &[(&str, &str)] = &[
    ("route", "routing lands in v0.2 (see docs/11-roadmap.md)"),
    ("layout", "shared layouts land with routing in v0.2"),
    ("guard", "route guards land with routing in v0.2"),
    ("fetch", "data fetching lands in v0.2"),
    ("stream", "real-time data lands in v0.2"),
    ("validate", "declarative validation lands in v0.2"),
    (
        "match",
        "pattern matching lands in v0.2; use if/else for now",
    ),
    ("import", "multi-file imports land in v0.2"),
    ("export", "multi-file imports land in v0.2"),
    ("global", "cross-component state lands in v0.2"),
    ("shared", "context state lands in v0.2"),
    ("type", "type aliases land in v0.2"),
    ("debug", "debug blocks land in v0.2"),
    ("assert", "assertions land in v0.2"),
    ("catch", "error boundaries land in v0.2"),
    ("enter", "motion lands in v0.2"),
    ("exit", "motion lands in v0.2"),
    ("await", "async lands with data fetching in v0.2"),
    ("async", "async lands with data fetching in v0.2"),
];

/// Returns the "not yet" message for a deferred keyword, if it is one.
pub fn deferred(name: &str) -> Option<&'static str> {
    DEFERRED
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, message)| *message)
}

/// Theme token categories, in the order they are searched when a property does
/// not name one explicitly.
pub const THEME_CATEGORIES: &[&str] = &[
    "colors",
    "fonts",
    "sizes",
    "spacing",
    "radius",
    "shadows",
    "breakpoints",
];

/// Which theme category a CSS property draws its tokens from.
pub fn token_category(property: &str) -> &'static str {
    match property {
        "color"
        | "background"
        | "background-color"
        | "border-color"
        | "outline-color"
        | "svg-fill"
        | "svg-stroke"
        | "caret-color"
        | "text-decoration-color"
        | "accent-color" => "colors",
        "font-family" | "font" => "fonts",
        "font-size" => "sizes",
        "border-radius" | "radius" => "radius",
        "box-shadow" | "shadow" | "text-shadow" => "shadows",
        "padding" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left"
        | "margin" | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "gap"
        | "row-gap" | "column-gap" | "top" | "right" | "bottom" | "left" | "inset"
        | "letter-spacing" | "text-indent" => "spacing",
        _ => "",
    }
}

/// Plain-friendly aliases for CSS properties. Anything not listed here is
/// passed through verbatim when it is kebab-case, so no CSS is unreachable.
pub const PROPERTY_ALIASES: &[(&str, &str)] = &[
    // `fill` on its own is the "take the remaining space" modifier; `fill` with
    // a value is a colour. SVG fill is spelled `svg-fill` to keep the two apart.
    ("fill", "background"),
    ("radius", "border-radius"),
    ("shadow", "box-shadow"),
    ("background", "background"),
    ("weight", "font-weight"),
    ("font", "font-family"),
    ("align", "align-items"),
    ("justify", "justify-content"),
    ("grow", "flex-grow"),
    ("shrink", "flex-shrink"),
    ("basis", "flex-basis"),
    ("wrap", "flex-wrap"),
    ("z", "z-index"),
    ("aspect", "aspect-ratio"),
    ("decoration", "text-decoration"),
    ("transform-text", "text-transform"),
    ("select-text", "user-select"),
    ("backdrop", "backdrop-filter"),
    ("blend", "mix-blend-mode"),
    ("clip", "clip-path"),
    ("svg-fill", "fill"),
    ("svg-stroke", "stroke"),
    ("columns", "grid-template-columns"),
    ("rows", "grid-template-rows"),
    ("size", "width"),
];

/// Single-word property names Plain accepts. Kebab-case names fall through to
/// CSS unchanged, which keeps the whole of CSS reachable.
pub const SINGLE_WORD_PROPERTIES: &[&str] = &[
    "color",
    "background",
    "width",
    "height",
    "size",
    "padding",
    "margin",
    "border",
    "outline",
    "radius",
    "shadow",
    "opacity",
    "weight",
    "font",
    "align",
    "justify",
    "gap",
    "overflow",
    "cursor",
    "position",
    "top",
    "right",
    "bottom",
    "left",
    "transform",
    "transition",
    "filter",
    "display",
    "grow",
    "shrink",
    "basis",
    "order",
    "columns",
    "rows",
    "z",
    "aspect",
    "decoration",
    "select-text",
    "backdrop",
    "blend",
    "clip",
    "visibility",
    "isolation",
    "resize",
    "appearance",
    "contain",
    "content",
    "grid",
    "flex",
    "inset",
    "place-items",
    "will-change",
];

/// The CSS property allowlist. Plain's own vocabulary is the alias table above;
/// everything else in CSS is reachable by writing its real name, and this list
/// is what lets the compiler tell a property from a value.
///
/// It is a whitespace-separated blob rather than a slice literal so it stays
/// readable and easy to extend.
pub const CSS_PROPERTIES: &str = r#"
accent-color align-content align-items align-self alignment-baseline all animation
animation-composition animation-delay animation-direction animation-duration
animation-fill-mode animation-iteration-count animation-name animation-play-state
animation-timing-function appearance aspect-ratio backdrop-filter backface-visibility
background background-attachment background-blend-mode background-clip background-color
background-image background-origin background-position background-position-x
background-position-y background-repeat background-size baseline-shift block-size
border border-block border-block-color border-block-end border-block-end-color
border-block-end-style border-block-end-width border-block-start
border-block-start-color border-block-start-style border-block-start-width
border-block-style border-block-width border-bottom border-bottom-color
border-bottom-left-radius border-bottom-right-radius border-bottom-style
border-bottom-width border-collapse border-color border-end-end-radius
border-end-start-radius border-image border-image-outset border-image-repeat
border-image-slice border-image-source border-image-width border-inline
border-inline-color border-inline-end border-inline-end-color
border-inline-end-style border-inline-end-width border-inline-start
border-inline-start-color border-inline-start-style border-inline-start-width
border-inline-style border-inline-width border-left border-left-color
border-left-style border-left-width border-radius border-right border-right-color
border-right-style border-right-width border-spacing border-start-end-radius
border-start-start-radius border-style border-top border-top-color
border-top-left-radius border-top-right-radius border-top-style border-top-width
border-width bottom box-decoration-break box-shadow box-sizing break-after
break-before break-inside caption-side caret-color clear clip clip-path
clip-rule color color-interpolation-filters color-scheme column-count column-fill
column-gap column-rule column-rule-color column-rule-style column-rule-width
column-span column-width columns contain contain-intrinsic-size container
container-name container-type content counter-increment counter-reset counter-set
cursor cx cy direction display dominant-baseline empty-cells fill fill-opacity
fill-rule filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink
flex-wrap float flood-color flood-opacity font font-family font-feature-settings
font-kerning font-language-override font-optical-sizing font-palette
font-size font-size-adjust font-stretch font-style font-synthesis
font-variant font-variant-alternates font-variant-caps font-variant-east-asian
font-variant-emoji font-variant-ligatures font-variant-numeric
font-variant-position font-variation-settings font-weight gap grid grid-area
grid-auto-columns grid-auto-flow grid-auto-rows grid-column grid-column-end
grid-column-start grid-row grid-row-end grid-row-start grid-template
grid-template-areas grid-template-columns grid-template-rows height hyphens
image-orientation image-rendering inline-size inset inset-block inset-block-end
inset-block-start inset-inline inset-inline-end inset-inline-start isolation
justify-content justify-items justify-self left letter-spacing line-break
line-height list-style list-style-image list-style-position list-style-type
margin margin-block margin-block-end margin-block-start margin-bottom
margin-inline margin-inline-end margin-inline-start margin-left margin-right
margin-top mask mask-clip mask-composite mask-image mask-mode mask-origin
mask-position mask-repeat mask-size mask-type max-block-size max-height
max-inline-size max-width min-block-size min-height min-inline-size min-width
mix-blend-mode object-fit object-position object-view-box offset offset-distance
offset-path offset-rotate opacity order orphans outline outline-color
outline-offset outline-style outline-width overflow overflow-anchor
overflow-block overflow-clip-margin overflow-inline overflow-wrap overflow-x
overflow-y overscroll-behavior overscroll-behavior-block overscroll-behavior-inline
overscroll-behavior-x overscroll-behavior-y padding padding-block
padding-block-end padding-block-start padding-bottom padding-inline
padding-inline-end padding-inline-start padding-left padding-right padding-top
page-break-after page-break-before page-break-inside paint-order perspective
perspective-origin place-content place-items place-self pointer-events position
quotes r resize right rotate row-gap ruby-align ruby-position scale scroll-behavior
scroll-margin scroll-margin-block scroll-margin-bottom scroll-margin-inline
scroll-margin-left scroll-margin-right scroll-margin-top scroll-padding
scroll-padding-block scroll-padding-bottom scroll-padding-inline
scroll-padding-left scroll-padding-right scroll-padding-top scroll-snap-align
scroll-snap-stop scroll-snap-type scrollbar-color scrollbar-gutter scrollbar-width
shape-image-threshold shape-margin shape-outside shape-rendering stop-color
stop-opacity stroke stroke-dasharray stroke-dashoffset stroke-linecap
stroke-linejoin stroke-miterlimit stroke-opacity stroke-width tab-size
table-layout text-align text-align-last text-anchor text-combine-upright
text-decoration text-decoration-color text-decoration-line text-decoration-skip-ink
text-decoration-style text-decoration-thickness text-emphasis text-emphasis-color
text-emphasis-position text-emphasis-style text-indent text-justify
text-orientation text-overflow text-rendering text-shadow text-transform
text-underline-offset text-underline-position text-wrap text-wrap-mode
text-wrap-style top touch-action transform transform-box transform-origin
transform-style transition transition-behavior transition-delay
transition-duration transition-property transition-timing-function translate
unicode-bidi user-select vector-effect vertical-align view-transition-class
view-transition-name visibility white-space widows width will-change word-break
word-spacing writing-mode x y z-index zoom
"#;

/// CSS property names that are also very common *values*. They still work as
/// properties at the start of a line, but they do not end an inline value list,
/// so `display flex` reads as one property with one value rather than two
/// properties with none.
pub const VALUE_UNSAFE_PROPERTIES: &[&str] =
    &["flex", "grid", "content", "contain", "table", "list"];

/// Looks a CSS property up in the allowlist.
pub fn is_css_property(name: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let set = SET.get_or_init(|| CSS_PROPERTIES.split_whitespace().collect());
    set.contains(name)
}

/// True when `name` can be used as a style property.
pub fn is_property(name: &str) -> bool {
    SINGLE_WORD_PROPERTIES.contains(&name)
        || PROPERTY_ALIASES.iter().any(|(alias, _)| *alias == name)
        || is_css_property(name)
}

/// Words that always end an inline value list, because they can only introduce a
/// new construct.
///
/// This is *not* the same as [`RESERVED`]: `none`, `true`, `false`, `and`, `or`
/// and `not` are reserved but are also ordinary values and operators, so they
/// must never terminate a value list.
pub const ITEM_KEYWORDS: &[&str] = &[
    "on",
    "when",
    "bind",
    "show",
    "key",
    "if",
    "else",
    "each",
    "in",
    "then",
    "let",
    "derive",
    "fn",
    "children",
    "slot",
    "return",
    "persist",
    "match",
    "route",
    "layout",
    "guard",
    "fetch",
    "stream",
    "validate",
    "global",
    "shared",
    "import",
    "export",
    "debug",
    "assert",
    "catch",
    "fallback",
    "enter",
    "exit",
    "await",
    "async",
    "component",
    "theme",
];

/// True when `name` always ends a value list and starts a new item.
pub fn hard_item(name: &str) -> bool {
    ITEM_KEYWORDS.contains(&name)
}

/// True when `name` ends a value list once the property already has a value.
///
/// Splitting terminators into "hard" and "soft" is what lets both of these read
/// correctly, even though `left` is a property and `hidden` is a modifier:
///
/// ```text
/// text-align left        -- one property, one value
/// position absolute left 0   -- two properties
/// visibility hidden      -- one property, one value
/// box hidden             -- one modifier
/// ```
pub fn soft_item(name: &str) -> bool {
    if VALUE_UNSAFE_PROPERTIES.contains(&name) {
        return false;
    }
    MODIFIERS.contains(&name)
        || ATTRIBUTES.contains(&name)
        || pseudo(name).is_some()
        || is_property(name)
}

/// True when `name` can start a new item on the same line.
pub fn starts_a_new_item(name: &str) -> bool {
    hard_item(name) || soft_item(name)
}

/// Maps a Plain property name to the CSS property it emits.
pub fn css_property(name: &str) -> String {
    for (alias, css) in PROPERTY_ALIASES {
        if *alias == name {
            return (*css).to_string();
        }
    }
    name.to_string()
}

/// Suggests the closest known property name, for diagnostics.
pub fn suggest_property(name: &str) -> Option<&'static str> {
    let target: Vec<char> = name.chars().collect();
    let mut best: Option<(&'static str, usize)> = None;
    for candidate in SINGLE_WORD_PROPERTIES.iter().chain(MODIFIERS.iter()) {
        let distance = edit_distance(&target, &candidate.chars().collect::<Vec<char>>());
        let better = match best {
            Some((_, best_distance)) => distance < best_distance,
            None => true,
        };
        if distance <= 2 && better {
            best = Some((*candidate, distance));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// Small Levenshtein distance, used only to make "did you mean" suggestions.
pub fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Properties that become HTML attributes rather than CSS declarations.
pub const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "role",
    "title",
    "name",
    "for",
    "action",
    "method",
    "target",
    "rel",
    "lang",
    "dir",
    "tabindex",
    "colspan",
    "rowspan",
    "scope",
    "datetime",
    "loading",
    "decoding",
    "srcset",
    "sizes",
    "poster",
    "preload",
    "autocomplete",
    "pattern",
    "maxlength",
    "minlength",
    "min",
    "max",
    "step",
    "placeholder",
    "label",
    "alt",
    "href",
    "to",
    "src",
    "value",
    "live",
    "describe",
    "test",
    "level",
    "download",
    "draggable",
    "spellcheck",
    "enterkeyhint",
    "inputmode",
];

/// Attributes that only mean something on some elements.
///
/// Everything in [`ATTRIBUTES`] that is not listed here is global: `id`,
/// `class`, `role`, `title`, `lang`, `tabindex` and friends are valid anywhere.
/// Restricting the rest is what lets the parser tell `label "Name"` on a
/// `check` (an accessible name) apart from the same two words on a `box` (a
/// mistake), and it is what makes `placeholder` on a `box` an error instead of
/// dead markup that a browser quietly ignores.
pub const RESTRICTED_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("for", &["label"]),
    ("action", &["form"]),
    ("method", &["form"]),
    ("autocomplete", &["form", "input", "textarea"]),
    ("placeholder", &["input", "textarea"]),
    ("pattern", &["input"]),
    ("maxlength", &["input", "textarea"]),
    ("minlength", &["input", "textarea"]),
    ("inputmode", &["input", "textarea"]),
    ("enterkeyhint", &["input", "textarea"]),
    ("min", &["input", "slider", "progress"]),
    ("max", &["input", "slider", "progress"]),
    ("step", &["input", "slider"]),
    ("multiple", &["select"]),
    (
        "value",
        &["option", "input", "check", "radio", "slider", "progress"],
    ),
    ("rows", &["textarea"]),
    ("cols", &["textarea"]),
    (
        "label",
        &[
            "check", "radio", "toggle", "slider", "select", "option", "input", "textarea",
        ],
    ),
    ("alt", &["image"]),
    ("srcset", &["image"]),
    ("sizes", &["image"]),
    ("loading", &["image"]),
    ("decoding", &["image"]),
    ("src", &["image", "video", "audio"]),
    ("poster", &["video"]),
    ("preload", &["video", "audio"]),
    ("href", &["link"]),
    ("to", &["link"]),
    ("download", &["link"]),
    ("target", &["link", "form"]),
    ("rel", &["link"]),
    ("colspan", &["table-cell"]),
    ("rowspan", &["table-cell"]),
    ("scope", &["table-cell"]),
    ("datetime", &["time"]),
    ("level", &["heading"]),
    ("type", &["input", "button", "link", "list", "item"]),
];

/// True when `property` is an HTML attribute on `element`.
///
/// A few names mean different things on different elements, and the compiler
/// always knows the element statically, so this is never ambiguous at runtime:
/// `rows` is `grid-template-rows` on a `grid` and the `rows` attribute on a
/// `textarea`; `label` is an accessible name on a `check` and nothing at all on
/// a `box`.
pub fn is_attribute(element: &str, property: &str) -> bool {
    if let Some((_, allowed)) = RESTRICTED_ATTRIBUTES
        .iter()
        .find(|(name, _)| *name == property)
    {
        return allowed.contains(&element);
    }
    ATTRIBUTES.contains(&property)
}

/// Accessible-name properties. Each one produces a real label or ARIA
/// attribute; the compiler never invents one.
pub const NAME_PROPERTIES: &[&str] = &["label", "alt", "describe", "title"];

/// Standard library functions, mapped from Plain's kebab-case spelling to the
/// runtime's camelCase member path.
pub const BUILTINS: &[(&str, &str)] = &[
    ("format-date", "formatDate"),
    ("format-number", "formatNumber"),
    ("format-currency", "formatCurrency"),
    ("format-percent", "formatPercent"),
    ("format-bytes", "formatBytes"),
    ("format-ordinal", "formatOrdinal"),
    ("format-compact", "formatCompact"),
    ("relative-time", "relativeTime"),
    ("slug", "slug"),
    ("truncate", "truncate"),
    ("parse-url", "parseUrl"),
    ("build-url", "buildUrl"),
    ("encode-uri", "encodeUri"),
    ("decode-uri", "decodeUri"),
    ("lighten", "color.lighten"),
    ("darken", "color.darken"),
    ("saturate", "color.saturate"),
    ("desaturate", "color.desaturate"),
    ("alpha", "color.alpha"),
    ("mix", "color.mix"),
    ("complement", "color.complement"),
    ("grayscale", "color.grayscale"),
    ("contrast-color", "contrastColor"),
    ("contrast-ratio", "contrastRatio"),
    ("clamp", "num.clamp"),
    ("lerp", "num.lerp"),
    ("map-range", "num.mapRange"),
    ("round", "num.round"),
    ("floor", "num.floor"),
    ("ceil", "num.ceil"),
    ("abs", "num.abs"),
    ("min", "num.min"),
    ("max", "num.max"),
    ("pow", "num.pow"),
    ("sqrt", "num.sqrt"),
    ("random", "num.random"),
    ("distance", "num.distance"),
    ("focus-first", "focusFirst"),
    ("focus", "focus"),
];

/// Resolves a built-in function name.
pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(plain, _)| *plain == name)
        .map(|(_, path)| *path)
}

/// Methods callable on a value, mapped to a runtime helper. `x.upper()` becomes
/// `P.str.upper(x)`.
pub const METHODS: &[(&str, &str)] = &[
    // strings
    ("upper", "str.upper"),
    ("lower", "str.lower"),
    ("capitalize", "str.capitalize"),
    ("title", "str.title"),
    ("trim", "str.trim"),
    ("contains", "str.contains"),
    ("starts-with", "str.startsWith"),
    ("ends-with", "str.endsWith"),
    ("replace", "str.replace"),
    ("replace-all", "str.replaceAll"),
    ("slice", "str.slice"),
    ("split", "str.split"),
    ("reverse", "str.reverse"),
    ("repeat", "str.repeat"),
    ("pad-left", "str.padLeft"),
    ("pad-right", "str.padRight"),
    // lists
    ("filter", "list.filter"),
    ("map", "list.map"),
    ("flat-map", "list.flatMap"),
    ("sort", "list.sortBy"),
    ("sort-by", "list.sortBy"),
    ("unique", "list.unique"),
    ("flatten", "list.flatten"),
    ("chunk", "list.chunk"),
    ("group", "list.group"),
    ("take", "list.take"),
    ("skip", "list.skip"),
    ("paginate", "list.paginate"),
    ("find", "list.find"),
    ("find-index", "list.findIndex"),
    ("index-of", "list.indexOf"),
    ("includes", "list.includes"),
    ("any", "list.any"),
    ("all", "list.all"),
    ("none", "list.none"),
    ("count", "list.count"),
    ("sum", "list.sum"),
    ("average", "list.average"),
    ("min-by", "list.minBy"),
    ("max-by", "list.maxBy"),
    ("reduce", "list.reduce"),
    ("zip", "list.zip"),
    ("insert", "list.insert"),
    ("remove", "list.remove"),
    ("remove-at", "list.removeAt"),
    ("move", "list.move"),
    ("update", "list.update"),
    ("first", "list.first"),
    ("last", "list.last"),
    ("join", "str.join"),
    // dates
    ("add", "datetime.add"),
    ("diff", "datetime.diff"),
    ("start-of", "datetime.startOf"),
    ("is-today", "datetime.isToday"),
    ("is-before", "datetime.isBefore"),
    ("is-after", "datetime.isAfter"),
];

/// Resolves a method name to its runtime path.
pub fn method(name: &str) -> Option<&'static str> {
    METHODS
        .iter()
        .find(|(plain, _)| *plain == name)
        .map(|(_, path)| *path)
}

/// Pipe operators that are *not* method calls, e.g. `items | filter(f)`.
/// They resolve to the same helpers as methods.
pub fn pipe_function(name: &str) -> Option<&'static str> {
    method(name).or_else(|| builtin(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_table_is_consistent() {
        for name in ELEMENT_NAMES {
            let def = element(name).unwrap_or_else(|| panic!("{name} is listed but not defined"));
            assert_eq!(def.name, *name);
            assert!(!def.tag.is_empty());
            assert!(
                def.name.chars().next().unwrap().is_ascii_lowercase(),
                "element {} must be lowercase so it cannot collide with a component name",
                def.name
            );
        }
        assert!(element("nope").is_none());
    }

    #[test]
    fn layout_row_is_not_a_table_row() {
        // The old draft used `row` for both a horizontal layout and a table
        // row. They are separate vocabulary entries now.
        assert_eq!(element("row").unwrap().tag, "div");
        assert_eq!(element("table-row").unwrap().tag, "tr");
    }

    #[test]
    fn pseudo_states_resolve() {
        assert_eq!(pseudo("hover"), Some(":hover"));
        assert_eq!(pseudo("focus"), Some(":focus-visible"));
        assert_eq!(pseudo("nope"), None);
    }

    #[test]
    fn property_recognition() {
        assert!(is_property("color"));
        assert!(is_property("font-size"));
        assert!(is_property("border-bottom-width"));
        assert!(!is_property("blorb"));
        assert!(!is_property("system-ui"), "a font name is not a property");
        assert_eq!(css_property("radius"), "border-radius");
        assert_eq!(css_property("font-size"), "font-size");
    }

    #[test]
    fn css_values_do_not_look_like_items_at_all() {
        // `display flex` and `flex-direction row` must read as one property each.
        for value in ["flex", "grid", "row", "column", "content", "table", "list"] {
            assert!(!starts_a_new_item(value), "{value} should stay a value");
        }
        // Ordinary CSS values that are not property names.
        for value in [
            "line-through",
            "space-between",
            "system-ui",
            "flex-start",
            "solid",
        ] {
            assert!(!starts_a_new_item(value), "{value} should stay a value");
        }
    }

    #[test]
    fn hard_items_always_end_a_value_list() {
        for name in ["on", "when", "bind", "show", "if", "each", "let"] {
            assert!(hard_item(name), "{name} introduces a construct");
        }
        // Values and operators are reserved words but never item starters.
        for name in ["none", "true", "false", "and", "or", "not"] {
            assert!(!hard_item(name), "{name} is a value, not an item");
        }
    }

    #[test]
    fn soft_items_end_a_value_list_only_after_one_value() {
        // `text-align left` -> left is a value; `position absolute left 0` -> two items.
        assert!(soft_item("left"));
        assert!(soft_item("top"));
        assert!(soft_item("color"));
        assert!(soft_item("padding"));
        assert!(soft_item("hidden"), "a modifier is a soft item");
        assert!(!soft_item("center"));
    }

    #[test]
    fn suggestions_are_close_but_not_wild() {
        assert_eq!(suggest_property("colr"), Some("color"));
        assert_eq!(suggest_property("pading"), Some("padding"));
        assert_eq!(suggest_property("zzzzzzzzz"), None);
    }

    #[test]
    fn token_categories() {
        assert_eq!(token_category("color"), "colors");
        assert_eq!(token_category("padding"), "spacing");
        assert_eq!(token_category("font-size"), "sizes");
        assert_eq!(token_category("display"), "");
    }

    #[test]
    fn reserved_words_are_not_elements() {
        for word in RESERVED {
            assert!(
                element(word).is_none(),
                "{word} is both reserved and an element"
            );
        }
        for word in MODIFIERS {
            assert!(
                element(word).is_none(),
                "{word} is both a modifier and an element"
            );
        }
    }

    #[test]
    fn deferred_features_have_messages() {
        assert!(deferred("route").is_some());
        assert!(deferred("stack").is_none());
    }

    #[test]
    fn attribute_names_are_not_style_properties() {
        assert!(is_attribute("input", "placeholder"));
        assert!(!is_attribute("box", "placeholder"));
        assert!(is_attribute("textarea", "rows"));
        assert!(
            !is_attribute("grid", "rows"),
            "rows on a grid is grid-template-rows"
        );
        assert!(is_attribute("option", "value"));
    }

    #[test]
    fn type_is_an_attribute_and_transition_is_a_property() {
        // `type` was first drafted as a style property. There is no CSS property
        // by that name, and `input type email` is how a field gets its keyboard
        // and its validation, so it is an attribute — on the elements that have
        // one, and nowhere else.
        assert!(is_attribute("input", "type"));
        assert!(is_attribute("button", "type"));
        assert!(!is_attribute("box", "type"));
        assert!(!is_property("type"));
        assert!(is_property("transition"));
        assert!(!RESERVED.contains(&"type"));
        assert!(!RESERVED.contains(&"transition"));
    }

    #[test]
    fn svg_paint_has_its_own_spelling() {
        assert_eq!(css_property("svg-fill"), "fill");
        assert_eq!(token_category("svg-fill"), "colors");
    }

    #[test]
    fn builtins_and_methods_resolve() {
        assert_eq!(builtin("format-date"), Some("formatDate"));
        assert_eq!(method("starts-with"), Some("str.startsWith"));
        assert_eq!(pipe_function("filter"), Some("list.filter"));
    }
}
