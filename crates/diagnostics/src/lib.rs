#![doc = include_str!("../README.md")]

//! oxide-fw-diagnostics — one diagnostic format shared by Rust, tooling workers and isolates (spec §19.4).
//!
//! Every error has: a stable code, a severity, a source-mapped code frame, the cause chain,
//! and a fix hint when one is known. Serialized as JSON over the JS boundary.

use serde::{Deserialize, Serialize};
// `write!` into a String for the code frame (clippy format_push_string).
use std::fmt::Write as _;

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

/// A single stack frame on the §13.4 error overlay (§V2-E08 item 1's
/// "source-mapped stack frames"): where the error points, in ONE coordinate
/// system named by `mapped`. `true` — the coordinates are the ORIGINAL
/// source's (the frame went through the module's source map, or the
/// producer knew its provenance). `false` — the generated output's, the
/// honest no-vouch shape when no map speaks for the file or the lookup
/// misses. `line`/`column` are 1-based, the crate's [`Location`]
/// convention, whatever `mapped` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFrame {
    /// The file the frame names: the original source when `mapped`, the
    /// generated module path when not.
    pub file: String,
    /// 1-based line in `file`.
    pub line: u32,
    /// 1-based column in `file`.
    pub column: u32,
    /// The function the stack reported, when the host carried one.
    pub function: Option<String>,
    /// The coordinate system: `true` = original source coordinates, `false`
    /// = the generated output's (unmapped or unmappable).
    pub mapped: bool,
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
    /// The stack the error came with (§V2-E08 item 1; §13.4's
    /// "source-mapped stacks"), outermost first. Additive: absent on
    /// pre-frames payloads (decoded as empty) and skipped while empty, so
    /// every existing payload keeps its bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<StackFrame>,
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
            frames: Vec::new(),
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
    /// Attach the source location (builder style: `Diagnostic::error(..).at(loc)`).
    #[must_use]
    pub fn at(mut self, loc: Location) -> Self {
        self.location = Some(loc);
        self
    }
    /// Attach the known fix (the miette help text).
    #[must_use]
    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
    /// Append a cause (the diagnostic's why-chain).
    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.causes.push(cause.into());
        self
    }
    /// Attach the rendered code frame.
    #[must_use]
    pub fn with_code_frame(mut self, frame: impl Into<String>) -> Self {
        self.code_frame = Some(frame.into());
        self
    }
    /// Attach the stack (outermost first; §V2-E08 item 1).
    #[must_use]
    pub fn with_frames(mut self, frames: Vec<StackFrame>) -> Self {
        self.frames = frames;
        self
    }
    /// Append one frame to the stack (§V2-E08 item 1).
    pub fn push_frame(&mut self, frame: StackFrame) {
        self.frames.push(frame);
    }
    /// Serialize the diagnostic to JSON (the §19.4 wire shape).
    ///
    /// # Panics
    ///
    /// Panics if serialization fails, which cannot happen for this
    /// plain-data struct (string fields only — no maps with non-string
    /// keys, no custom serializers).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("diagnostic serializes")
    }
    /// Deserialize a diagnostic from JSON (the §19.4 wire shape).
    ///
    /// # Errors
    ///
    /// Returns the underlying `serde_json` error when `s` is not a
    /// diagnostic document (bad shape, wrong types).
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

/// SPEC-19E: render a code frame for a diagnostic. Returns a snippet with a
/// marker (`^`) under the offending line + a 1-based line number gutter. The
/// `source` is the whole module; the `line` is 1-based. `col` is optional (a
/// diagnostic with only a line produces a line gutter with no underline).
#[must_use]
pub fn render_code_frame(source: &str, line: usize, col: Option<usize>) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() || line == 0 || line > lines.len() {
        return String::new();
    }
    let target = lines[line - 1];
    let gutter_width = line.to_string().len();
    let gutter = " ".repeat(gutter_width);
    let mut out = String::new();
    let _ = writeln!(out, "{line:gutter_width$} | {target}");
    if let Some(c) = col {
        // Column is 1-based; underline the character at that column.
        let prefix_width = c.saturating_sub(1);
        // Count characters (not bytes) up to the col for the space prefix.
        let spaces: String = target
            .chars()
            .take(prefix_width)
            .map(|ch| if ch == '\t' { '\t' } else { ' ' })
            .collect();
        let _ = writeln!(out, "{gutter} | {spaces}^");
    }
    out
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if let Some(loc) = &self.location
            && let Some(file) = &loc.file
        {
            write!(f, " at {file}:{}", loc.line)?;
        }
        // §V2-E08 item 1: the stack renders as one `at file:line:col` line
        // per frame (outermost first), the function name in parentheses
        // when the host carried one. Unmapped frames are not marked — the
        // mapped flag is the payload's contract, the terminal line just
        // names where the frame points.
        for frame in &self.frames {
            write!(f, "\n  at {}:{}:{}", frame.file, frame.line, frame.column)?;
            if let Some(name) = &frame.function {
                write!(f, " ({name})")?;
            }
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
///
/// `Clone` (V2-C09): the image-job single-flight shares one awaited outcome
/// across N concurrent requests, so the error half must be cloneable too.
#[derive(Debug, Clone, thiserror::Error)]
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
    assert!(frame.contains('^'), "caret present: {frame}");
}

#[test]
fn render_code_frame_line_only_has_no_caret() {
    let src = "a\nb\n";
    let frame = render_code_frame(src, 1, None);
    assert!(frame.contains("1 | a"));
    assert!(!frame.contains('^'));
}

#[test]
fn diagnostic_display_includes_code_frame() {
    let d = Diagnostic::error("TEST-E", "boom").with_code_frame("2 | error here\n  |     ^");
    let rendered = format!("{d}");
    assert!(rendered.contains("[TEST-E] boom"));
    assert!(rendered.contains("2 | error here"), "frame: {rendered}");
}

// §V2-E08 item 1 (the overlay's source-mapped stack frames; spec §13.4's
// "source-mapped stacks"): the diagnostic carries a stack — one frame per
// wire step — and the frames ride the JSON payload additively (`serde`
// default, skipped while empty, so every pre-frames payload keeps its
// bytes). Red-first: these pins failed to compile before `StackFrame`
// existed.

#[test]
fn stack_frame_roundtrips_through_json() {
    let f = StackFrame {
        file: "dist/.oxide/pool/comp.tsx".to_string(),
        line: 4,
        column: 1,
        function: Some("Counter".to_string()),
        mapped: false,
    };
    let json = serde_json::to_string(&f).unwrap();
    assert!(json.contains("\"line\":4"), "plain field spelling: {json}");
    let back: StackFrame = serde_json::from_str(&json).unwrap();
    assert_eq!(back, f);
}

#[test]
fn diagnostic_frames_are_additive_and_skip_while_empty() {
    let d = Diagnostic::error("CFG-001", "old shape");
    let json = d.to_json();
    assert!(!json.contains("frames"), "empty stack: no field, no bytes");
    let with = d.with_frames(vec![StackFrame {
        file: "src/comp.tsx".to_string(),
        line: 3,
        column: 8,
        function: None,
        mapped: true,
    }]);
    let json = with.to_json();
    assert!(
        json.contains("\"frames\":[{"),
        "present stack rides: {json}"
    );
    let back = Diagnostic::from_json(&json).unwrap();
    assert_eq!(back.frames.len(), 1);
    assert_eq!(back.frames[0].file, "src/comp.tsx");
    assert!(back.frames[0].mapped);
}

#[test]
fn push_frame_builds_the_stack_in_order() {
    let mut d = Diagnostic::error("RT-POOL", "boom");
    d.push_frame(StackFrame {
        file: "a.ts".to_string(),
        line: 1,
        column: 1,
        function: None,
        mapped: false,
    });
    d.push_frame(StackFrame {
        file: "b.ts".to_string(),
        line: 2,
        column: 1,
        function: Some("helper".to_string()),
        mapped: true,
    });
    assert_eq!(d.frames.len(), 2);
    assert_eq!(d.frames[0].file, "a.ts");
    assert_eq!(d.frames[1].function.as_deref(), Some("helper"));
}

#[test]
fn display_renders_the_stack_frames() {
    let d = Diagnostic::error("RT-POOL", "boom").with_frames(vec![
        StackFrame {
            file: "src/comp.tsx".to_string(),
            line: 7,
            column: 5,
            function: Some("Counter".to_string()),
            mapped: true,
        },
        StackFrame {
            file: "pool/comp.tsx".to_string(),
            line: 4,
            column: 1,
            function: None,
            mapped: false,
        },
    ]);
    let rendered = format!("{d}");
    assert!(
        rendered.contains("at src/comp.tsx:7:5"),
        "frame line: {rendered}"
    );
    assert!(rendered.contains("Counter"), "the function name renders");
    assert!(
        rendered.contains("at pool/comp.tsx:4:1"),
        "second frame: {rendered}"
    );
}
