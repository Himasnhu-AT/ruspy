//! The script call stack, kept by both execution engines: feeds error traces and the
//! `get_stack()` / `print_stack()` builtins.

use crate::diagnostics::{Diag, LineIndex, Span, TraceFrame};
use crate::value::Value;
use std::rc::Rc;

#[derive(Default)]
pub struct CallStack {
    frames: Vec<(String, Span)>,
    file: String,
    lines: Option<Rc<LineIndex>>,
}

impl CallStack {
    /// Names the source so stacks can show `file:line:col`.
    pub fn set_source(&mut self, file: &str, lines: Rc<LineIndex>) {
        if self.file != file {
            self.file = file.to_string();
        }
        self.lines = Some(lines);
    }

    /// Copies the source naming from `other` (a fresh stack for a resumed coroutine).
    pub fn adopt_source(&mut self, other: &CallStack) {
        self.file = other.file.clone();
        self.lines = other.lines.clone();
    }

    pub fn push(&mut self, func: &str) {
        self.frames.push((func.to_string(), Span::new(0, 0)));
    }

    pub fn pop(&mut self) {
        self.frames.pop();
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// The running function is now at `span` (a call site).
    pub fn mark(&mut self, span: Span) {
        if let Some(top) = self.frames.last_mut() {
            top.1 = span;
        }
    }

    /// Attaches the current stack to `d` the first time an error crosses a function boundary;
    /// the innermost frame points at the failing expression.
    pub fn attach(&self, d: &mut Diag) {
        if !d.trace.is_empty() || self.frames.is_empty() {
            return;
        }
        d.trace = self.frames.iter().rev().map(|(f, s)| TraceFrame { func: f.clone(), span: *s }).collect();
        d.trace[0].span = d.span;
    }

    fn located(&self, s: Span) -> (usize, usize) {
        self.lines.as_ref().map_or((0, 0), |l| l.locate(s.start))
    }

    fn lines_text(&self) -> String {
        let mut out = String::new();
        for (f, s) in self.frames.iter().rev() {
            let (l, c) = self.located(*s);
            out.push_str(&format!("  at {f} ({}:{l}:{c})\n", self.file));
        }
        out
    }

    /// `get_stack()` / `print_stack()`: `Some(result)` when `name` is one of them.
    /// `print_stack` is returned as `Err(text)` inside `Some(Ok(..))`'s sibling: callers
    /// route the text to the host's `print`.
    pub fn builtin(&self, name: &str) -> Option<Builtin> {
        match name {
            "get_stack" => Some(Builtin::Value(Value::array(
                self.frames
                    .iter()
                    .rev()
                    .map(|(f, s)| {
                        let (l, c) = self.located(*s);
                        Value::dict(vec![
                            (Value::Str("function".into()), Value::Str(f.clone())),
                            (Value::Str("file".into()), Value::Str(self.file.clone())),
                            (Value::Str("line".into()), Value::Int(l as i64)),
                            (Value::Str("col".into()), Value::Int(c as i64)),
                        ])
                    })
                    .collect(),
            ))),
            "print_stack" => Some(Builtin::Print(format!("stack:\n{}", self.lines_text().trim_end()))),
            _ => None,
        }
    }
}

pub enum Builtin {
    Value(Value),
    /// Text for the host's `print`.
    Print(String),
}
