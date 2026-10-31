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
    "scroll",
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
    ("pointer", "pointer-events"),
    ("select-text", "user-select"),
    ("backdrop", "backdrop-filter"),
    ("blend", "mix-blend-mode"),
    ("clip", "clip-path"),
    ("scroll", "overflow"),
    ("svg-fill", "fill"),
    ("svg-stroke", "stroke"),
    ("columns", "grid-template-columns"),
    ("rows", "grid-template-rows"),
    ("span", "grid-column"),
    ("area", "grid-area"),
    ("size", "width"),
];

/// Single-word property names Plain accepts. Kebab-case names fall through to
/// CSS unchanged, which keeps the whole of CSS reachable.
pub const SINGLE_WORD_PROPERTIES: &[&str] = &[
    "type",
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
    "wrap",
    "order",
    "columns",
    "rows",
    "span",
    "area",
    "z",
    "aspect",
    "decoration",
    "pointer",
    "select-text",
    "backdrop",
    "blend",
    "clip",
    "scroll",
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

/// True when `name` can be used as a style property.
pub fn is_property(name: &str) -> bool {
    if SINGLE_WORD_PROPERTIES.contains(&name) {
        return true;
    }
    PROPERTY_ALIASES.iter().any(|(alias, _)| *alias == name) || name.contains('-')
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

/// True when `property` is an HTML attribute on `element`.
///
/// A few names mean different things on different elements. That is resolved
/// from the element, which the compiler always knows statically, so it is never
/// ambiguous at runtime: `rows` is `grid-template-rows` on a `grid` and the
/// `rows` attribute on a `textarea`.
pub fn is_attribute(element: &str, property: &str) -> bool {
    if property == "rows" || property == "cols" {
        return matches!(element, "textarea");
    }
    if property == "value" {
        return matches!(
            element,
            "option" | "input" | "check" | "radio" | "slider" | "progress"
        );
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
        assert_eq!(css_property("radius"), "border-radius");
        assert_eq!(css_property("font-size"), "font-size");
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
    fn type_and_transition_are_properties_not_keywords() {
        assert!(is_property("type"));
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
