//! The §V2-F01 islands config surface: the registry entry union
//! (`string | { src, framework? }`) and the inference order the spec
//! pins. The boot planner that consumes the resolved entries lives in
//! `oxide_fw_compiler::islands`; the CLI reads the island files and
//! applies [`island_framework`] — the pure rule lives here (the
//! config crate never touches the filesystem).

use serde::{Deserialize, Serialize};

/// §V2-F01: one island registry entry. The plain string form is
/// today's shape (the source path relative to the project root); the
/// object form adds an optional explicit `framework` so a `.tsx` Solid
/// island can declare itself without a pragma.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IslandSpec {
    /// The string form: the source path relative to the project root.
    Plain(String),
    /// The object form: the source plus an optional explicit framework.
    Object {
        /// The source path relative to the project root.
        src: String,
        /// The explicit framework (the inference order's first arm).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        framework: Option<String>,
    },
}

impl IslandSpec {
    /// The source path (relative to the project root), either form.
    #[must_use]
    pub fn src(&self) -> &str {
        match self {
            Self::Plain(src) | Self::Object { src, .. } => src,
        }
    }

    /// The explicit framework, when the entry declared one.
    #[must_use]
    pub fn framework(&self) -> Option<&str> {
        match self {
            Self::Plain(_) => None,
            Self::Object { framework, .. } => framework.as_deref(),
        }
    }
}

/// The §V2-F01 inference order for an island's framework, verbatim:
/// an explicit `framework`; the file extension (`.vue` is vue,
/// `.svelte` is svelte); a first-line pragma `// @oxide-framework
/// solid` for `.tsx`; else the project `framework`. Pure — the caller
/// supplies the file's first line (the build side's read).
///
/// The spec's needle carries a trailing space, so a bare
/// `// @oxide-framework` line is not a pragma (the project framework
/// answers); an empty pragma rest yields the empty string, which the
/// caller surfaces as the unknown-framework case.
#[must_use]
pub fn island_framework(
    src: &str,
    explicit: Option<&str>,
    project: &str,
    first_line: &str,
) -> String {
    if let Some(f) = explicit {
        return f.to_string();
    }
    match std::path::Path::new(src)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("vue") => return "vue".into(),
        Some("svelte") => return "svelte".into(),
        _ => {}
    }
    if let Some(rest) = first_line.trim().strip_prefix("// @oxide-framework ") {
        return rest.trim().to_string();
    }
    project.to_string()
}
