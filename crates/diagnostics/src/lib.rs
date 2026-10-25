//! oxide-fw-diagnostics — one diagnostic format shared by Rust, tooling workers and isolates (spec §19.4).
//!
//! Every error has: a stable code, a severity, a source-mapped code frame, the cause chain,
//! and a fix hint when one is known. Serialized as JSON over the JS boundary.

use serde::{Deserialize, Serialize};

/// Stable diagnostic codes, namespaced per subsystem (`ODF-*`, `RT-*`, `CFG-*`, ...).
pub type Code = &'static str;

/// Severity of a diagnostic (spec §19.4 error design rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Fatal: the operation failed.
    Error,
    /// Non-fatal: reported but work continues.
    Warning,
    /// Informational (e.g. inferred render mode reports).
    Info,
}

/// A source location: 1-based line/column, byte offset, optional file path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub file: Option<String>,
    pub line: u32,
    pub column: u32,
    pub offset: u32,
    pub length: u32,
}

impl Location {
    pub fn unknown() -> Self {
        Self {
            file: None,
            line: 0,
            column: 0,
            offset: 0,
            length: 0,
        }
    }
    pub fn file(file: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            file: Some(file.into()),
            line,
            column,
            offset: 0,
            length: 0,
        }
    }
}

/// A single diagnostic: stable code, source-mapped frame, cause, fix (spec §19.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub location: Option<Location>,
    /// Source lines surrounding `location` for the code frame display.
    pub code_frame: Option<String>,
    pub causes: Vec<String>,
    /// The fix when one is known, e.g. "make the connection lazy so the app snapshot succeeds".
    pub fix: Option<String>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            message: message.into(),
            location: None,
            code_frame: None,
            causes: Vec::new(),
            fix: None,
        }
    }
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }
    pub fn info(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            ..Self::error(code, message)
        }
    }
    pub fn at(mut self, loc: Location) -> Self {
        self.location = Some(loc);
        self
    }
    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.causes.push(cause.into());
        self
    }
    pub fn with_code_frame(mut self, frame: impl Into<String>) -> Self {
        self.code_frame = Some(frame.into());
        self
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("diagnostic serializes")
    }
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

/// SPEC-19E: render a code frame for a diagnostic. Returns a snippet with a
/// marker (`^`) under the offending line + a 1-based line number gutter. The
/// `source` is the whole module; the `line` is 1-based. `col` is optional (a
/// diagnostic with only a line produces a line gutter with no underline).
pub fn render_code_frame(source: &str, line: usize, col: Option<usize>) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() || line == 0 || line > lines.len() {
        return String::new();
    }
    let target = lines[line - 1];
    let gutter_width = line.to_string().len();
    let mut out = String::new();
    out.push_str(&format!(
        "{:width$} | {target}\n",
        line,
        width = gutter_width
    ));
    if let Some(c) = col {
        // Column is 1-based; underline the character at that column.
        let prefix_width = c.saturating_sub(1);
        // Count characters (not bytes) up to the col for the space prefix.
        let spaces: String = target
            .chars()
            .take(prefix_width)
            .map(|ch| if ch == '\t' { '\t' } else { ' ' })
            .collect();
        out.push_str(&format!(
            "{:width$} | {spaces}^\n",
            "",
            width = gutter_width
        ));
    }
    out
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if let Some(loc) = &self.location
            && let Some(file) = &loc.file {
                write!(f, " at {file}:{}", loc.line)?;
            }
        // SPEC-19E: a code frame (source snippet with a marker) is displayed
        // when the diagnostic carries one. Producers populate this when the
        // upstream span is available (Oxc compile diagnostics already flow
        // through `Location`; this formatter uses the pre-rendered frame
        // rather than re-reading the file, so it works for a diagnostic
        // replayed from a worker).
        if let Some(frame) = &self.code_frame {
            write!(f, "\n{frame}")?;
        }
        if let Some(fix) = &self.fix {
            write!(f, "\n  fix: {fix}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

/// An error carrying a `Diagnostic`, so every subsystem error converts losslessly.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OxideError(pub Box<Diagnostic>);

impl OxideError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self(Box::new(Diagnostic::error(code, message)))
    }
}

impl From<Diagnostic> for OxideError {
    fn from(d: Diagnostic) -> Self {
        OxideError(Box::new(d))
    }
}

impl From<Box<Diagnostic>> for OxideError {
    fn from(d: Box<Diagnostic>) -> Self {
        OxideError(d)
    }
}

/// Convenience alias used across crates.
pub type Result<T> = std::result::Result<T, OxideError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_roundtrips_through_json() {
        let d = Diagnostic::error("CFG-001", "unknown config key `framwork`")
            .at(Location::file("oxide.config.ts", 3, 3))
            .with_fix("did you mean `framework`?")
            .with_cause("static extraction rejected the key");
        let json = d.to_json();
        let back = Diagnostic::from_json(&json).unwrap();
        assert_eq!(back.code, "CFG-001");
        assert_eq!(back.fix.as_deref(), Some("did you mean `framework`?"));
        assert_eq!(back.location.as_ref().unwrap().line, 3);
    }

    #[test]
    fn display_includes_code_and_fix() {
        let d = Diagnostic::error("SNAP-002", "top-level side effect")
            .with_fix("create connections lazily");
        let s = format!("{d}");
        assert!(s.contains("SNAP-002"));
        assert!(s.contains("create connections lazily"));
    }
}

#[test]
fn render_code_frame_formats_with_gutter() {
    let src = "line one\nline two\nline three\n";
    let frame = render_code_frame(src, 2, Some(5));
    assert!(frame.contains("2 | line two"), "gutter line: {frame}");
    assert!(frame.contains("^"), "caret present: {frame}");
}

#[test]
fn render_code_frame_line_only_has_no_caret() {
    let src = "a\nb\n";
    let frame = render_code_frame(src, 1, None);
    assert!(frame.contains("1 | a"));
    assert!(!frame.contains("^"));
}

#[test]
fn diagnostic_display_includes_code_frame() {
    let d = Diagnostic::error("TEST-E", "boom").with_code_frame("2 | error here\n  |     ^");
    let rendered = format!("{d}");
    assert!(rendered.contains("[TEST-E] boom"));
    assert!(rendered.contains("2 | error here"), "frame: {rendered}");
}
