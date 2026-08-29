//! Spans + diagnostics — the shared error currency of every ruspy pass.
//!
//! Replaces the old model where the lexer *panicked* on bad input and the parser
//! returned bare `String`s with no location (RUSPY_SCHEMA.md §7). Every pass now
//! produces `Diag`s carrying a `Span`, so an editor can point at the offending code.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    /// A zero-width span at `at` (used for EOF and insertion-point errors).
    pub fn point(at: usize) -> Self {
        Self { start: at, end: at }
    }
    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// A value tagged with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A compile- or run-time diagnostic: what went wrong, where, and how to fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct Diag {
    pub severity: Severity,
    pub span: Span,
    /// A stable machine code, e.g. `"E0001"`, so the editor + tests can match on it.
    pub code: &'static str,
    pub message: String,
    pub help: Option<String>,
}

impl Diag {
    pub fn error(span: Span, code: &'static str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, span, code, message: message.into(), help: None }
    }
    pub fn warning(span: Span, code: &'static str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, span, code, message: message.into(), help: None }
    }
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev}[{}]: {}", self.code, self.message)?;
        if let Some(h) = &self.help {
            write!(f, " (help: {h})")?;
        }
        Ok(())
    }
}

/// Maps byte offsets to 1-based `(line, column)` for display.
pub struct LineIndex {
    /// Byte offset of the start of each line (index 0 = line 1).
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts }
    }

    /// 1-based `(line, column)` for a byte offset (column counts bytes; fine for ASCII).
    pub fn locate(&self, offset: usize) -> (usize, usize) {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => (line + 1, 1),
            Err(next) => {
                let line = next - 1;
                (line + 1, offset - self.line_starts[line] + 1)
            }
        }
    }

    /// A compact `"line:col"` for a span's start.
    pub fn label(&self, span: Span) -> String {
        let (l, c) = self.locate(span.start);
        format!("{l}:{c}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_merge_and_point() {
        assert_eq!(Span::new(2, 5).to(Span::new(4, 9)), Span::new(2, 9));
        assert!(Span::point(3).is_empty());
        assert_eq!(Span::new(3, 7).len(), 4);
    }

    #[test]
    fn line_index_locates_offsets() {
        let src = "ab\ncde\nf";
        let li = LineIndex::new(src);
        assert_eq!(li.locate(0), (1, 1)); // 'a'
        assert_eq!(li.locate(1), (1, 2)); // 'b'
        assert_eq!(li.locate(3), (2, 1)); // 'c'
        assert_eq!(li.locate(5), (2, 3)); // 'e'
        assert_eq!(li.locate(7), (3, 1)); // 'f'
        assert_eq!(li.label(Span::new(5, 6)), "2:3");
    }

    #[test]
    fn diag_display_includes_code_and_help() {
        let d = Diag::error(Span::new(0, 1), "E0001", "bad char").with_help("remove it");
        assert_eq!(format!("{d}"), "error[E0001]: bad char (help: remove it)");
        assert!(d.is_error());
    }
}
