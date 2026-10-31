//! The Plain lexer.
//!
//! Plain is line oriented and strings cannot span lines in v0.1, so the lexer
//! works a line at a time. That keeps indentation handling obvious: leading
//! whitespace is measured once per line, never in the middle of an expression.
//!
//! Lexical rules that resolve ambiguities in the original draft:
//!
//! * A hyphen directly attached to a name continues that name (`max-width` is
//!   one token). Binary minus must therefore be written with spaces around it:
//!   `count - 1`, never `count-1`.
//! * `--` always starts a comment, so no name may contain two hyphens in a row.
//! * `{` and `}` are hard errors: interpolation is `~(expr)` and records are
//!   `record(key: value)`, so Plain source never contains a curly brace.
//! * A leading `-` immediately followed by a digit is a negative number.

use plain_core::{Diagnostic, FileId, Span};

/// Token classes. Names carry their text; numbers carry a value and a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Newline,
    Indent,
    Dedent,
    Eof,
    Name,
    /// Opens a string literal. The literal's pieces follow inline so that
    /// interpolations keep real spans into the original file.
    StrStart,
    /// A decoded run of literal text inside a string.
    StrText,
    /// Closes a string literal.
    StrEnd,
    /// Opens an interpolation `~(`.
    InterpStart,
    /// Closes an interpolation `)`.
    InterpEnd,
    Int,
    Float,
    Length,
    Color,
    Eq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Pipe,
    Arrow,
    Dot,
    DotDot,
    /// `...`, used for spreads inside lists and records.
    Ellipsis,
    Comma,
    Colon,
    Question,
    At,
    LParen,
    RParen,
    LBracket,
    RBracket,
}

impl TokenKind {
    /// Human-readable name for diagnostics.
    pub fn describe(self) -> &'static str {
        match self {
            TokenKind::Newline => "end of line",
            TokenKind::Indent => "an indented block",
            TokenKind::Dedent => "end of block",
            TokenKind::Eof => "end of file",
            TokenKind::Name => "a name",
            TokenKind::StrStart => "a string",
            TokenKind::StrText => "string text",
            TokenKind::StrEnd => "the end of a string",
            TokenKind::InterpStart => "the start of an interpolation",
            TokenKind::InterpEnd => "the end of an interpolation",
            TokenKind::Int | TokenKind::Float => "a number",
            TokenKind::Length => "a measurement",
            TokenKind::Color => "a colour",
            TokenKind::Eq => "`=`",
            TokenKind::EqEq => "`==`",
            TokenKind::BangEq => "`!=`",
            TokenKind::Lt => "`<`",
            TokenKind::LtEq => "`<=`",
            TokenKind::Gt => "`>`",
            TokenKind::GtEq => "`>=`",
            TokenKind::Plus => "`+`",
            TokenKind::Minus => "`-`",
            TokenKind::Star => "`*`",
            TokenKind::Slash => "`/`",
            TokenKind::Percent => "`%`",
            TokenKind::Pipe => "`|`",
            TokenKind::Arrow => "`=>`",
            TokenKind::Dot => "`.`",
            TokenKind::DotDot => "`..`",
            TokenKind::Ellipsis => "`...`",
            TokenKind::Comma => "`,`",
            TokenKind::Colon => "`:`",
            TokenKind::Question => "`?`",
            TokenKind::At => "`@`",
            TokenKind::LParen => "`(`",
            TokenKind::RParen => "`)`",
            TokenKind::LBracket => "`[`",
            TokenKind::RBracket => "`]`",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// Name text, or the raw source of a literal.
    pub text: String,
    /// Numeric value for `Int`, `Float` and `Length`.
    pub value: f64,
    /// CSS unit for `Length`, e.g. `px`. Empty when there is none.
    pub unit: String,
}

impl Token {
    fn simple(kind: TokenKind, span: Span, text: impl Into<String>) -> Self {
        Token {
            kind,
            span,
            text: text.into(),
            value: 0.0,
            unit: String::new(),
        }
    }

    /// How this token should be described in an error message.
    pub fn describe(&self) -> String {
        match self.kind {
            TokenKind::Name => format!("`{}`", self.text),
            TokenKind::StrText => format!("the text \"{}\"", self.text),
            TokenKind::Int | TokenKind::Float | TokenKind::Length => format!("`{}`", self.text),
            TokenKind::Color => format!("the colour `{}`", self.text),
            other => other.describe().to_string(),
        }
    }

    /// True when this token can start a value (used to decide whether `-` is a
    /// unary minus or a binary operator).
    pub fn ends_a_value(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::Name
                | TokenKind::StrText
                | TokenKind::StrEnd
                | TokenKind::Int
                | TokenKind::Float
                | TokenKind::Length
                | TokenKind::Color
                | TokenKind::RParen
                | TokenKind::RBracket
        )
    }
}

/// A physical line, in character indices.
#[derive(Debug, Clone, Copy)]
struct Line {
    /// Index of the first character of the line.
    start: usize,
    /// Index just past the last character, excluding the newline.
    end: usize,
    /// Number of leading spaces (tabs are excluded and reported instead).
    indent: usize,
    /// Index of the first character that is not leading whitespace.
    content: usize,
    /// True when the leading whitespace contained a tab.
    tab: bool,
    /// Index of the first tab in the leading whitespace.
    tab_at: usize,
}

/// Turns Plain source into tokens.
pub struct Lexer {
    file: FileId,
    chars: Vec<char>,
    /// Byte offset of every character index, plus one entry for the end.
    offsets: Vec<u32>,
    pos: usize,
    indents: Vec<usize>,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
    /// True once a content token has been emitted, so `Newline` separators are
    /// only inserted *between* lines.
    started: bool,
}

/// Lexes `source`, returning tokens plus any lexical diagnostics. The token
/// stream is always usable: on error the lexer skips the offending character
/// and keeps going, so one bad line does not hide the rest of the file.
pub fn lex(file: FileId, source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(file, source);
    lexer.run();
    (lexer.tokens, lexer.errors)
}

impl Lexer {
    fn new(file: FileId, source: &str) -> Self {
        let chars: Vec<char> = source.chars().collect();
        let mut offsets = Vec::with_capacity(chars.len() + 1);
        let mut byte = 0u32;
        for ch in &chars {
            offsets.push(byte);
            byte += ch.len_utf8() as u32;
        }
        offsets.push(byte);
        Lexer {
            file,
            chars,
            offsets,
            pos: 0,
            indents: vec![0],
            tokens: Vec::new(),
            errors: Vec::new(),
            started: false,
        }
    }

    fn run(&mut self) {
        for line in self.lines() {
            self.lex_line(&line);
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push_simple(TokenKind::Dedent, self.span_at(self.chars.len()));
        }
        self.push_simple(TokenKind::Eof, self.span_at(self.chars.len()));
    }

    /// Splits the source into physical lines with their indentation measured.
    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut index = 0usize;
        while index <= self.chars.len() {
            let at_end = index == self.chars.len();
            if at_end || self.chars[index] == '\n' {
                let mut end = index;
                if end > start && self.chars[end - 1] == '\r' {
                    end -= 1;
                }
                let mut content = start;
                let mut indent = 0usize;
                let mut tab = false;
                let mut tab_at = start;
                while content < end && (self.chars[content] == ' ' || self.chars[content] == '\t') {
                    if self.chars[content] == '\t' {
                        if !tab {
                            tab_at = content;
                        }
                        tab = true;
                    } else {
                        indent += 1;
                    }
                    content += 1;
                }
                out.push(Line {
                    start,
                    end,
                    indent,
                    content,
                    tab,
                    tab_at,
                });
                start = index + 1;
            }
            if at_end {
                break;
            }
            index += 1;
        }
        out
    }

    fn lex_line(&mut self, line: &Line) {
        // Blank lines and comment-only lines carry no structure at all.
        if line.content >= line.end {
            return;
        }
        if self.at_comment(line.content, line.end) {
            return;
        }

        if self.started {
            self.push_simple(TokenKind::Newline, self.span_at(line.start));
        }

        // Tabs in indentation are an error, not a silently different width. The
        // line is then treated as belonging to the current level so one tab does
        // not also produce a bogus dedent error.
        if line.tab {
            self.error(
                "E001",
                "Tabs cannot be used for indentation",
                self.span_at(line.tab_at),
            )
            .note("Plain measures nesting in spaces so every reader sees the same tree.")
            .suggest("Run `plain fmt`, or replace the tab with two spaces.");
        } else {
            self.adjust_indent(line);
        }
        self.started = true;
        self.pos = line.content;
        self.tokenize(line.end);
    }

    fn adjust_indent(&mut self, line: &Line) {
        let current = *self.indents.last().unwrap_or(&0);
        if line.indent > current {
            self.indents.push(line.indent);
            self.push_simple(TokenKind::Indent, self.span_at(line.start));
            return;
        }
        if line.indent == current {
            return;
        }
        while self.indents.len() > 1 && line.indent < *self.indents.last().unwrap_or(&0) {
            self.indents.pop();
            self.push_simple(TokenKind::Dedent, self.span_at(line.start));
        }
        if *self.indents.last().unwrap_or(&0) != line.indent {
            self.error(
                "E002",
                "This line does not line up with any previous indentation level",
                self.span_at(line.start),
            )
            .note("Dedenting has to land exactly on an earlier level, or the tree is ambiguous.")
            .suggest("Align this line with the block it belongs to (two spaces per level).");
            self.indents.push(line.indent);
        }
    }

    /// True when `index` starts a `--` comment.
    fn at_comment(&self, index: usize, end: usize) -> bool {
        index + 1 < end && self.chars[index] == '-' && self.chars[index + 1] == '-'
    }

    /// Tokenizes one line's content, stopping at a comment or at `end`.
    fn tokenize(&mut self, end: usize) {
        self.tokenize_range(end, true)
    }

    /// `comments` is false inside an interpolation, where a `--` is part of the
    /// expression rather than a comment.
    fn tokenize_range(&mut self, end: usize, comments: bool) {
        while self.pos < end {
            if comments && self.at_comment(self.pos, end) {
                return;
            }
            let ch = self.chars[self.pos];
            match ch {
                ' ' | '\t' => self.pos += 1,
                '"' => self.lex_string(end),
                '#' => self.lex_color(end),
                '~' => {
                    self.error(
                        "E005",
                        "`~` outside a string is not valid Plain",
                        self.span_at(self.pos),
                    )
                    .note("Interpolation lives inside string literals only.")
                    .suggest("Write `\"~(name)\"` inside a string.");
                    self.pos += 1;
                }
                '{' | '}' => {
                    self.error(
                        "E006",
                        "Plain has no curly braces",
                        self.span_at(self.pos),
                    )
                    .note("Structure comes from indentation, interpolation from `~( )`, and records from `record( )`.")
                    .suggest("Replace `{value}` inside a string with `~(value)`.");
                    self.pos += 1;
                }
                ';' => {
                    self.error("E007", "Plain has no semicolons", self.span_at(self.pos))
                        .suggest("Delete the `;` — a newline ends the statement.");
                    self.pos += 1;
                }
                '<' if self.pos + 1 < end
                    && (self.chars[self.pos + 1].is_ascii_alphabetic()
                        || self.chars[self.pos + 1] == '/') =>
                {
                    self.error(
                        "E008",
                        "Plain does not use HTML tags",
                        self.span_at(self.pos),
                    )
                    .note("Elements are words: `text`, `box`, `button`, `image`.")
                    .suggest("Write `text \"hello\"` instead of `<p>hello</p>`.");
                    self.pos += 1;
                }
                _ if ch.is_ascii_digit() => self.lex_number(end),
                '.' if self.pos + 1 < end && self.chars[self.pos + 1].is_ascii_digit() => {
                    self.lex_number(end)
                }
                _ if is_name_start(ch) => self.lex_name(end),
                _ => self.lex_operator(end),
            }
        }
    }

    fn lex_name(&mut self, end: usize) {
        let start = self.pos;
        self.pos += 1;
        while self.pos < end {
            let ch = self.chars[self.pos];
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.pos += 1;
                continue;
            }
            // A hyphen continues a name only when a name character follows it,
            // which is what keeps `max-width` one token and `a - b` three.
            if ch == '-'
                && self.pos + 1 < end
                && (self.chars[self.pos + 1].is_ascii_alphanumeric()
                    || self.chars[self.pos + 1] == '_')
            {
                self.pos += 1;
                continue;
            }
            break;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        self.push(Token::simple(
            TokenKind::Name,
            self.span(start, self.pos),
            text,
        ));
    }

    fn lex_number(&mut self, end: usize) {
        let start = self.pos;
        if self.pos < end && self.chars[self.pos] == '.' {
            self.pos += 1;
        }
        while self.pos < end && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let mut is_float = self.chars[start] == '.';
        if self.pos + 1 < end
            && self.chars[self.pos] == '.'
            && self.chars[self.pos + 1].is_ascii_digit()
        {
            is_float = true;
            self.pos += 1;
            while self.pos < end && self.chars[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let digits_end = self.pos;
        let unit_start = self.pos;
        if self.pos < end
            && (self.chars[self.pos] == '%' || self.chars[self.pos].is_ascii_alphabetic())
        {
            while self.pos < end
                && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '%')
            {
                self.pos += 1;
            }
        }
        let digits: String = self.chars[start..digits_end].iter().collect();
        let unit: String = self.chars[unit_start..self.pos].iter().collect();
        let value: f64 = digits.parse().unwrap_or(0.0);
        let span = self.span(start, self.pos);
        let raw: String = self.chars[start..self.pos].iter().collect();

        let kind = if !unit.is_empty() {
            if !is_known_unit(&unit) {
                self.error("E009", format!("`{unit}` is not a unit Plain knows"), span)
                    .note("Known units: px, rem, em, %, ch, ex, vh, vw, vmin, vmax, pt, cm, mm, in, pc, s, ms, deg, rad, turn, fr.")
                    .suggest(format!("If `{raw}` was meant to be a name, names cannot start with a digit — try `size-{raw}`."));
            }
            TokenKind::Length
        } else if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };

        let mut token = Token::simple(kind, span, raw);
        token.value = value;
        token.unit = unit;
        self.push(token);
    }

    fn lex_color(&mut self, end: usize) {
        let start = self.pos;
        self.pos += 1;
        while self.pos < end && self.chars[self.pos].is_ascii_hexdigit() {
            self.pos += 1;
        }
        let digits = self.pos - start - 1;
        let raw: String = self.chars[start..self.pos].iter().collect();
        let span = self.span(start, self.pos);
        if !matches!(digits, 3 | 4 | 6 | 8) {
            self.error(
                "E009",
                format!("`{raw}` is not a valid colour literal"),
                span,
            )
            .note("Hex colours are 3, 4, 6 or 8 digits: #fff, #fffa, #ffffff, #ffffffaa.")
            .suggest("Use `rgb(255, 87, 51)` or `hsl(14, 100%, 60%)` for other forms.");
        }
        self.push(Token::simple(TokenKind::Color, span, raw));
    }

    fn lex_string(&mut self, end: usize) {
        let start = self.pos;
        self.push_simple(TokenKind::StrStart, self.span_at(start));
        self.pos += 1;
        let mut buf = String::new();
        let mut chunk_start = self.pos;
        let mut closed = false;

        // Emits the literal text accumulated so far.
        macro_rules! emit {
            () => {
                if !buf.is_empty() {
                    let text = std::mem::take(&mut buf);
                    let span = self.span(chunk_start, self.pos);
                    self.push(Token::simple(TokenKind::StrText, span, text));
                }
            };
        }
        // Emits and then starts a new run of text here. The two are separate so
        // that the final emit does not assign to `chunk_start` for nobody.
        macro_rules! flush {
            () => {
                emit!();
                chunk_start = self.pos;
            };
        }

        while self.pos < end {
            let ch = self.chars[self.pos];
            if ch == '"' {
                emit!();
                self.pos += 1;
                closed = true;
                break;
            }
            if ch == '\\' {
                self.pos += 1;
                if self.pos >= end {
                    break;
                }
                let escape = self.chars[self.pos];
                let decoded = match escape {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    '0' => Some('\0'),
                    '\\' => Some('\\'),
                    '"' => Some('"'),
                    '\'' => Some('\''),
                    '~' => Some('~'),
                    _ => None,
                };
                match decoded {
                    Some(character) => {
                        buf.push(character);
                        self.pos += 1;
                    }
                    None => {
                        self.error(
                            "E004",
                            format!("`\\{escape}` is not an escape sequence Plain knows"),
                            self.span(self.pos - 1, self.pos + 1),
                        )
                        .note("Supported escapes: \\n \\t \\r \\0 \\\\ \\\" \\' \\~")
                        .suggest("Write the character without the backslash, or use one of the escapes above.");
                        buf.push(escape);
                        self.pos += 1;
                    }
                }
                continue;
            }
            if ch == '~' && self.pos + 1 < end && self.chars[self.pos + 1] == '(' {
                flush!();
                let open = self.pos;
                self.pos += 2;
                self.push_simple(TokenKind::InterpStart, self.span(open, self.pos));
                let expr_start = self.pos;
                match self.find_matching_paren(expr_start, end) {
                    Some(expr_end) => {
                        if expr_end == expr_start {
                            self.error(
                                "E005",
                                "This interpolation is empty",
                                self.span(open, expr_end + 1),
                            )
                            .suggest("Put an expression inside: `~(count + 1)`.");
                        }
                        self.pos = expr_start;
                        self.tokenize_range(expr_end, false);
                        self.pos = expr_end + 1;
                        self.push_simple(TokenKind::InterpEnd, self.span(expr_end, self.pos));
                    }
                    None => {
                        self.error(
                            "E005",
                            "This interpolation is missing its closing `)`",
                            self.span(open, end),
                        )
                        .suggest("Close it: `\"~(user.name)\"`.");
                        break;
                    }
                }
                chunk_start = self.pos;
                continue;
            }
            if ch == '~' {
                self.error(
                    "E005",
                    "`~` must be followed by `(` to interpolate",
                    self.span_at(self.pos),
                )
                .suggest("Write `\"~(value)\"`.");
                buf.push(ch);
                self.pos += 1;
                continue;
            }
            buf.push(ch);
            self.pos += 1;
        }

        if !closed {
            self.error(
                "E003",
                "This string is missing its closing quote",
                self.span(start, end),
            )
            .note("Strings run to the end of the line; multi-line strings are planned for v0.2.")
            .suggest("Add a `\"` at the end of the line.");
            emit!();
        }
        self.push_simple(
            TokenKind::StrEnd,
            self.span_at(self.pos.min(self.chars.len())),
        );
    }

    /// Finds the `)` matching an already-consumed `(`, skipping over strings.
    fn find_matching_paren(&self, from: usize, end: usize) -> Option<usize> {
        let mut depth = 1usize;
        let mut index = from;
        while index < end {
            match self.chars[index] {
                '"' => {
                    index += 1;
                    while index < end && self.chars[index] != '"' {
                        if self.chars[index] == '\\' {
                            index += 1;
                        }
                        index += 1;
                    }
                }
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn lex_operator(&mut self, end: usize) {
        let start = self.pos;
        let ch = self.chars[self.pos];
        let next = if self.pos + 1 < end {
            Some(self.chars[self.pos + 1])
        } else {
            None
        };
        let (kind, length) = match (ch, next) {
            ('=', Some('>')) => (TokenKind::Arrow, 2),
            ('=', Some('=')) => (TokenKind::EqEq, 2),
            ('=', _) => (TokenKind::Eq, 1),
            ('!', Some('=')) => (TokenKind::BangEq, 2),
            ('!', _) => {
                self.error("E010", "Use `not` instead of `!`", self.span_at(start))
                    .suggest("Write `not done`, not `!done`.");
                self.pos += 1;
                return;
            }
            ('<', Some('=')) => (TokenKind::LtEq, 2),
            ('<', _) => (TokenKind::Lt, 1),
            ('>', Some('=')) => (TokenKind::GtEq, 2),
            ('>', _) => (TokenKind::Gt, 1),
            ('.', Some('.')) => {
                if self.pos + 2 < end && self.chars[self.pos + 2] == '.' {
                    (TokenKind::Ellipsis, 3)
                } else {
                    (TokenKind::DotDot, 2)
                }
            }
            ('.', _) => (TokenKind::Dot, 1),
            ('+', _) => (TokenKind::Plus, 1),
            ('-', _) => (TokenKind::Minus, 1),
            ('*', _) => (TokenKind::Star, 1),
            ('/', _) => (TokenKind::Slash, 1),
            ('%', _) => (TokenKind::Percent, 1),
            // `&&` and `||` are accepted as spellings of `and`/`or` so that
            // pasted JavaScript conditions still work. They must be tested
            // before the single-character `|` arm.
            ('&', Some('&')) | ('|', Some('|')) => {
                let name = if ch == '&' { "and" } else { "or" };
                self.pos += 2;
                self.push(Token::simple(
                    TokenKind::Name,
                    self.span(start, self.pos),
                    name,
                ));
                return;
            }
            ('|', _) => (TokenKind::Pipe, 1),
            (',', _) => (TokenKind::Comma, 1),
            (':', _) => (TokenKind::Colon, 1),
            ('?', _) => (TokenKind::Question, 1),
            ('@', _) => (TokenKind::At, 1),
            ('(', _) => (TokenKind::LParen, 1),
            (')', _) => (TokenKind::RParen, 1),
            ('[', _) => (TokenKind::LBracket, 1),
            (']', _) => (TokenKind::RBracket, 1),
            _ => {
                self.error(
                    "E010",
                    format!("`{ch}` cannot appear in Plain source"),
                    self.span_at(start),
                );
                self.pos += 1;
                return;
            }
        };
        self.pos += length;
        let text: String = self.chars[start..self.pos].iter().collect();
        self.push(Token::simple(kind, self.span(start, self.pos), text));
    }

    /* -- helpers ---------------------------------------------------------- */

    fn push(&mut self, token: Token) {
        self.tokens.push(token);
    }

    fn push_simple(&mut self, kind: TokenKind, span: Span) {
        self.tokens.push(Token::simple(kind, span, ""));
    }

    fn span(&self, from: usize, to: usize) -> Span {
        let from = from.min(self.offsets.len() - 1);
        let to = to.min(self.offsets.len() - 1);
        Span::new(self.offsets[from], self.offsets[to.max(from)])
    }

    fn span_at(&self, index: usize) -> Span {
        let index = index.min(self.offsets.len() - 1);
        let end = (index + 1).min(self.offsets.len() - 1);
        Span::new(self.offsets[index], self.offsets[end])
    }

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
}

fn is_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Units Plain understands. Anything else is reported so that `12prx` does not
/// silently become an invalid CSS declaration.
fn is_known_unit(unit: &str) -> bool {
    matches!(
        unit,
        "%" | "px"
            | "rem"
            | "em"
            | "ch"
            | "ex"
            | "vh"
            | "vw"
            | "vmin"
            | "vmax"
            | "dvh"
            | "dvw"
            | "svh"
            | "svw"
            | "lvh"
            | "lvw"
            | "pt"
            | "cm"
            | "mm"
            | "in"
            | "pc"
            | "s"
            | "ms"
            | "deg"
            | "rad"
            | "turn"
            | "grad"
            | "fr"
            | "dpi"
            | "dppx"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use plain_core::SourceMap;

    fn lex_source(source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
        lex(FileId(0), source)
    }

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex_source(source)
            .0
            .iter()
            .map(|token| token.kind)
            .filter(|kind| *kind != TokenKind::Eof)
            .collect()
    }

    fn texts(source: &str) -> Vec<String> {
        lex_source(source)
            .0
            .iter()
            .filter(|token| token.kind == TokenKind::Name)
            .map(|token| token.text.clone())
            .collect()
    }

    #[test]
    fn indentation_becomes_indent_and_dedent() {
        let tokens = kinds("box\n  text \"hi\"\n  text \"there\"\nbox\n");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Name,
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Name,
                TokenKind::StrStart,
                TokenKind::StrText,
                TokenKind::StrEnd,
                TokenKind::Newline,
                TokenKind::Name,
                TokenKind::StrStart,
                TokenKind::StrText,
                TokenKind::StrEnd,
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Name,
            ]
        );
    }

    #[test]
    fn blank_and_comment_lines_carry_no_structure() {
        let tokens = kinds("box\n\n  -- a note\n  text \"hi\"\n");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Name,
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Name,
                TokenKind::StrStart,
                TokenKind::StrText,
                TokenKind::StrEnd,
                TokenKind::Dedent,
            ]
        );
    }

    #[test]
    fn nested_dedents_are_one_token_per_level() {
        let tokens = kinds("a\n  b\n    c\nd\n");
        let dedents = tokens.iter().filter(|k| **k == TokenKind::Dedent).count();
        assert_eq!(dedents, 2);
    }

    #[test]
    fn hyphens_attach_to_names_but_minus_needs_spaces() {
        assert_eq!(texts("max-width 800px"), vec!["max-width"]);
        assert_eq!(texts("count - 1"), vec!["count"]);
        let (_, errors) = lex_source("count-1");
        assert!(errors.is_empty());
        assert_eq!(
            texts("count-1"),
            vec!["count-1"],
            "a tight hyphen continues the name"
        );
    }

    #[test]
    fn double_hyphen_starts_a_comment() {
        assert_eq!(texts("text \"hi\" -- done"), vec!["text"]);
        assert_eq!(texts("a--b"), vec!["a"]);
    }

    #[test]
    fn numbers_units_and_colours() {
        let (tokens, errors) = lex_source("12px 1.5rem 100% 42 2.75 #ff5733 #fff");
        assert!(errors.is_empty(), "{errors:?}");
        let lengths: Vec<(f64, String)> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Length)
            .map(|t| (t.value, t.unit.clone()))
            .collect();
        assert_eq!(
            lengths,
            vec![
                (12.0, "px".into()),
                (1.5, "rem".into()),
                (100.0, "%".into())
            ]
        );
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Int && t.value == 42.0));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Float && t.value == 2.75));
        assert_eq!(
            tokens.iter().filter(|t| t.kind == TokenKind::Color).count(),
            2
        );
    }

    #[test]
    fn ranges_do_not_swallow_the_dot() {
        let (tokens, errors) = lex_source("1..5");
        assert!(errors.is_empty());
        assert_eq!(
            tokens.iter().map(|t| t.kind).collect::<Vec<_>>()[..3],
            [TokenKind::Int, TokenKind::DotDot, TokenKind::Int]
        );
    }

    #[test]
    fn unknown_units_are_reported() {
        let (_, errors) = lex_source("12prx");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].title.contains("not a unit"));
    }

    fn string_parts(source: &str) -> Vec<String> {
        let (tokens, errors) = lex_source(source);
        assert!(errors.is_empty(), "{errors:?}");
        tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::StrText | TokenKind::InterpStart))
            .map(|t| match t.kind {
                TokenKind::StrText => format!("text:{}", t.text),
                _ => "interp".to_string(),
            })
            .collect()
    }

    fn interpolation_source(source: &str) -> Vec<String> {
        let (tokens, errors) = lex_source(source);
        assert!(errors.is_empty(), "{errors:?}");
        let mut out = Vec::new();
        let mut inside = false;
        let mut current = String::new();
        for token in &tokens {
            match token.kind {
                TokenKind::InterpStart => {
                    inside = true;
                    current.clear();
                }
                TokenKind::InterpEnd => {
                    inside = false;
                    out.push(current.trim().to_string());
                }
                _ if inside => {
                    if !current.is_empty() && needs_space_before(&token.text, &current) {
                        current.push(' ');
                    }
                    current.push_str(&token.text);
                }
                _ => {}
            }
        }
        out
    }

    fn needs_space_before(text: &str, so_far: &str) -> bool {
        let last = so_far.chars().last().unwrap_or(' ');
        let first = text.chars().next().unwrap_or(' ');
        !(matches!(last, '(' | '[' | '.' | '~') || matches!(first, ')' | ']' | ',' | '.'))
    }

    #[test]
    fn strings_decode_escapes() {
        // The Plain source is: text "a\nb\"c" — a newline escape and a quote
        // escape. `r##"..."##` keeps the backslashes exactly as written.
        let source = r##"text "a\nb\"c""##;
        assert_eq!(string_parts(source), vec!["text:a\nb\"c"]);
    }

    #[test]
    fn strings_split_interpolations() {
        let source = r##"text "Count: ~(count + 1) items""##;
        assert_eq!(
            string_parts(source),
            vec!["text:Count: ", "interp", "text: items"]
        );
        assert_eq!(interpolation_source(source), vec!["count + 1"]);
    }

    #[test]
    fn interpolation_spans_point_into_the_real_file() {
        let source = r##"text "~(count)""##;
        let (tokens, errors) = lex_source(source);
        assert!(errors.is_empty(), "{errors:?}");
        let name = tokens.iter().find(|token| token.text == "count").unwrap();
        assert_eq!(
            &source[name.span.start as usize..name.span.end as usize],
            "count"
        );
    }

    #[test]
    fn interpolation_can_contain_a_string_and_nested_parens() {
        let source = r##"text "~(if n == 1 then "item" else "items")""##;
        let (tokens, errors) = lex_source(source);
        assert!(errors.is_empty(), "{errors:?}");
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
        let count = |wanted: TokenKind| kinds.iter().filter(|kind| **kind == wanted).count();
        // The inner string is lexed inline, so it gets its own StrStart/StrEnd
        // rather than being re-lexed from a copied substring.
        // Three strings: the outer literal plus the two inside the interpolation.
        assert_eq!(count(TokenKind::StrStart), 3);
        assert_eq!(count(TokenKind::StrEnd), 3);
        assert_eq!(count(TokenKind::InterpStart), 1);
        assert_eq!(count(TokenKind::InterpEnd), 1);
        assert!(tokens.iter().any(|token| token.text == "item"));
    }

    #[test]
    fn a_double_hyphen_inside_a_string_is_not_a_comment() {
        assert_eq!(string_parts(r##"text "a--b""##), vec!["text:a--b"]);
    }

    #[test]
    fn ellipsis_and_range_are_distinct() {
        let (tokens, errors) = lex_source("[...items] 1..5");
        assert!(errors.is_empty(), "{errors:?}");
        let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::Ellipsis));
        assert!(kinds.contains(&TokenKind::DotDot));
    }

    #[test]
    fn curly_braces_semicolons_and_tags_are_rejected_with_guidance() {
        let (_, errors) = lex_source("text {name};\n<div>hi</div>");
        let codes: Vec<&str> = errors.iter().map(|e| e.code.as_str()).collect();
        assert!(codes.contains(&"E006"), "{codes:?}");
        assert!(codes.contains(&"E007"), "{codes:?}");
        assert!(codes.contains(&"E008"), "{codes:?}");
    }

    #[test]
    fn tabs_in_indentation_are_rejected() {
        let (_, errors) = lex_source("box\n\ttext \"hi\"\n");
        assert!(errors.iter().any(|e| e.code == "E001"), "{errors:?}");
    }

    #[test]
    fn unterminated_string_is_recovered() {
        let (tokens, errors) = lex_source("text \"oops\nbox\n");
        assert!(errors.iter().any(|e| e.code == "E003"), "{errors:?}");
        // Lexing continues, so the next line still produces tokens.
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Name && t.text == "box"));
    }

    #[test]
    fn and_and_or_aliases_lex_to_names() {
        assert_eq!(texts("a && b || c"), vec!["a", "and", "b", "or", "c"]);
    }

    #[test]
    fn spans_point_at_the_right_bytes() {
        let mut sources = SourceMap::new();
        let file = sources.add("app.pln", "text \"hi\"\n  size 2rem\n");
        let (tokens, _) = lex(file, "text \"hi\"\n  size 2rem\n");
        let size = tokens.iter().find(|t| t.text == "size").unwrap();
        let (line, col) = sources.get(file).unwrap().line_col(size.span.start);
        assert_eq!((line, col), (2, 3));
        assert_eq!(sources.get(file).unwrap().slice(size.span), "size");
    }
}
