//! Byte-offset spans into a [`SourceFile`](crate::source::SourceFile).

/// A half-open byte range `start..end` inside one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// Creates a span covering `start..end`.
    pub const fn new(start: u32, end: u32) -> Self {
        Span { start, end }
    }

    /// An empty span anchored at `offset`.
    pub const fn point(offset: u32) -> Self {
        Span {
            start: offset,
            end: offset,
        }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Number of bytes covered.
    pub fn len(self) -> usize {
        (self.end.saturating_sub(self.start)) as usize
    }

    /// True when the span covers no bytes.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// Identifies one file inside a [`SourceMap`](crate::source::SourceMap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);
