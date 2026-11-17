//! V2-E08 item 1 (the server half): `GET /__oxide/open?file=&line=` — the
//! open-in-editor dev-plane endpoint the §V2.J map advertises next to the
//! inspector (`/__oxide/open, /__oxide/cost` — dev plane, dev-token
//! guarded, D41).
//!
//! The endpoint is the dev overlay's "jump to the file" half: the browser
//! cannot spawn a process, so the dev server does it — on a request the
//! dev server's own token authorized. The spec's constraints are the
//! module's contract:
//!
//! - **The path is confined to the project root.** The `file` parameter
//!   (relative, or absolute-under-root) is joined to the root,
//!   canonicalized and required to stay inside the canonical root AND be
//!   a real file. A `../` escape, an absolute path outside the root, a
//!   directory or a missing file refuse with the coded [`DEV_OPEN_001`]
//!   verdict (422) — the endpoint never opens a file the project does not
//!   own, and `..` spellings that STAY inside the root normalize honestly
//!   instead of being banned outright.
//! - **The editor comes from the environment, resolved once per request**
//!   (a dev session can reasonably export it late): `OXIDE_EDITOR`, then
//!   `EDITOR`, then the documented default `code -g`. The value is split
//!   on whitespace (glob-lite, no shell — the recorded delta) and may
//!   carry the `{file}`/`{line}` placeholders for editors whose goto
//!   spelling is not `path:line` (`vim +{line} {file}`); without any
//!   placeholder the target is appended as one `path:line` argument (the
//!   VS Code `-g` convention the default spells).
//! - **The launch is a seam.** [`EditorOpener`] receives the resolved
//!   argv-shaped [`EditorCommand`] and does the spawning; the production
//!   [`CommandOpener`] spawns detached with null stdio (the same shape the
//!   dev crate's `open_browser` uses). Tests and embedded hosts inject a
//!   fake and never spawn.
//! - **The endpoint exists only when attached** (the inspector's
//!   posture): [`crate::Pipeline::open_editor`] is `None` in the default
//!   pipeline, the unattached shape falls through to the normal dispatch,
//!   and production is byte-identical. The dev boot attaches it via
//!   [`crate::Pipeline::with_editor_opener`]; the dev-token guard (the
//!   same constant-time compare the inspector arm runs) rides the
//!   pipeline arm, not the endpoint.
//!
//! Observability follows the crate's endpoint convention: every refusal
//! warns with the code and the offending path before the response rides
//! out, and a launch logs the editor, the root-relative file and the line.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxide_fw_diagnostics::OxideError;

use crate::{OxideResponse, percent_decode};

/// The §V2.J dev-plane path the endpoint serves.
pub const OPEN_EDITOR_ENDPOINT_PATH: &str = "/__oxide/open";

/// The path-confinement refusal (`422`): the requested file escapes the
/// project root, is a directory or does not exist. The fix is the path
/// itself — the overlay only ever links files the project serves.
pub const DEV_OPEN_001: &str = "DEV-OPEN-001";

/// The editor-launch refusal (`500`): the resolved editor process could
/// not be spawned (the binary is missing from the PATH, most commonly).
pub const DEV_OPEN_002: &str = "DEV-OPEN-002";

/// The resolved editor: the program and the argument template its env
/// value spelled (the placeholders still in place — [`Self::build_argv`]
/// fills them per request).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    /// The program the launcher spawns.
    pub program: String,
    /// The template arguments (possibly carrying `{file}`/`{line}`).
    pub args: Vec<String>,
}

impl EditorCommand {
    /// Fill the placeholders for one request: every `{file}` becomes the
    /// canonical absolute path, every `{line}` the line number. A template
    /// that names NO placeholder appends one `<path>:<line>` argument —
    /// the goto shape the documented default (`code -g`) needs. The
    /// program itself is not included (the struct carries it).
    #[must_use]
    pub fn build_args(&self, path: &Path, line: u32) -> Vec<String> {
        let file = path.display().to_string();
        let line_s = line.to_string();
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        let mut substituted = false;
        for arg in &self.args {
            let has = arg.contains("{file}") || arg.contains("{line}");
            substituted |= has;
            if has {
                argv.push(arg.replace("{file}", &file).replace("{line}", &line_s));
            } else {
                argv.push(arg.clone());
            }
        }
        if !substituted {
            argv.push(format!("{file}:{line_s}"));
        }
        argv
    }

    /// The concrete argv-shaped command one launch runs.
    #[must_use]
    pub fn build_for(&self, path: &Path, line: u32) -> EditorCommand {
        EditorCommand {
            program: self.program.clone(),
            args: self.build_args(path, line),
        }
    }
}

/// The launch seam: turn a resolved [`EditorCommand`] into a running
/// editor (or a coded failure). The production impl spawns detached;
/// fakes record.
pub trait EditorOpener: Send + Sync {
    /// Spawn the editor for one request.
    ///
    /// # Errors
    ///
    /// The coded launch failure ([`DEV_OPEN_002`] for the production
    /// impl) — the endpoint answers 500 and the overlay shows the reason.
    fn open(&self, cmd: &EditorCommand) -> Result<(), OxideError>;
}

/// The production launcher: `std::process::Command` with null stdio, the
/// child handle dropped (fire-and-forget — the same detached shape the dev
/// crate's `open_browser` uses; the editor outlives the request).
pub struct CommandOpener;

impl EditorOpener for CommandOpener {
    fn open(&self, cmd: &EditorCommand) -> Result<(), OxideError> {
        std::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                OxideError::new(
                    DEV_OPEN_002,
                    format!(
                        "the editor process could not be spawned (program {:?}, is it on the PATH?): {e}",
                        cmd.program
                    ),
                )
            })
    }
}

/// The editor chain the spec names, in order: `OXIDE_EDITOR`, `EDITOR`,
/// the documented default `code -g`. Values that trim to empty are not
/// editor choices — the chain falls through. The value splits on
/// whitespace (glob-lite, no shell: a quoted editor path with spaces is
/// not supported — the recorded delta; the placeholder rule below is the
/// escape hatch for editors whose goto spelling is not `path:line`).
#[must_use]
pub fn resolve_editor(getenv: &dyn Fn(&str) -> Option<String>) -> Option<EditorCommand> {
    for key in ["OXIDE_EDITOR", "EDITOR"] {
        if let Some(v) = getenv(key) {
            let mut parts = v.split_whitespace();
            if let Some(program) = parts.next() {
                return Some(EditorCommand {
                    program: program.to_string(),
                    args: parts.map(str::to_string).collect(),
                });
            }
        }
    }
    Some(EditorCommand {
        program: "code".into(),
        args: vec!["-g".into()],
    })
}

/// The endpoint's state: the project root the confinement runs against
/// and the injected launcher. The dev boot attaches one via
/// [`crate::Pipeline::with_editor_opener`].
pub struct OpenEditorEndpoint {
    root: PathBuf,
    opener: Arc<dyn EditorOpener>,
}

impl OpenEditorEndpoint {
    /// An endpoint over one project root and launcher.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, opener: Arc<dyn EditorOpener>) -> Self {
        Self {
            root: root.into(),
            opener,
        }
    }

    /// The project root (the examples and the dev boot echo it).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The total request → response function (everything but the dev-token
    /// guard, which is the pipeline arm's — the token never reaches this
    /// module). `method` is the request method; `query` the raw query
    /// string (no leading `?`).
    ///
    /// - non-`GET` → `405` + `allow: GET` (the nav endpoint's contract).
    /// - a missing/empty `file` or a non-positive/`line` → `400` with the
    ///   usage line.
    /// - a confined-path refusal → `422` + [`DEV_OPEN_001`].
    /// - a launch failure → `500` + [`DEV_OPEN_002`].
    /// - success → `200` JSON `{ ok, editor, file, line }` — `file` is the
    ///   root-relative spelling the overlay renders.
    #[must_use]
    pub fn serve(&self, method: &str, query: &str) -> OxideResponse {
        if !method.eq_ignore_ascii_case("GET") {
            return error_response(405, "the open endpoint answers GET", Some(("allow", "GET")));
        }
        let file_raw = param(query, "file");
        let file = file_raw.as_deref().unwrap_or_default();
        if file.is_empty() {
            return error_response(
                400,
                "missing or empty `file`: the endpoint is `GET /__oxide/open?file=<path>&line=<n>`",
                None,
            );
        }
        let line = match param(query, "line") {
            None => 1,
            Some(raw) => match raw.parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    return error_response(
                        400,
                        "the `line` parameter must be a line number of 1 or more",
                        None,
                    );
                }
            },
        };
        let canon_root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let Some(target) = self.confine(file, &canon_root) else {
            // The refusal already warned with the code; the response is a
            // 422 without repeating the path twice.
            return error_response(
                422,
                &format!(
                    "{DEV_OPEN_001}: {file} is not a file the project root owns (the endpoint opens files under the project root only — the path must exist and be a file)"
                ),
                None,
            );
        };
        let Some(editor) = resolve_editor(&|k| std::env::var(k).ok()) else {
            // Unreachable today (the chain always falls back to `code -g`);
            // kept total so a future no-default resolution cannot panic.
            return error_response(
                500,
                "no editor resolved: set `OXIDE_EDITOR` (or `EDITOR`) to the editor command",
                None,
            );
        };
        let cmd = editor.build_for(&target, line);
        let rel = target
            .strip_prefix(&canon_root)
            .unwrap_or(&target)
            .display()
            .to_string();
        if let Err(e) = self.opener.open(&cmd) {
            tracing::warn!(
                code = DEV_OPEN_002,
                file = %rel,
                line,
                error = %e,
                "the editor launch failed"
            );
            return error_response(
                500,
                &format!("{DEV_OPEN_002}: the editor launch failed: {e}"),
                None,
            );
        }
        tracing::info!(
            editor = %cmd.program,
            file = %rel,
            line,
            "opened in editor"
        );
        OxideResponse::json(
            200,
            serde_json::json!({
                "ok": true,
                "editor": cmd.program,
                "file": rel,
                "line": line,
            }),
        )
    }

    /// The confinement half: `file` (relative or absolute) canonicalized
    /// under the canonical root, required to be an existing regular file
    /// inside it. Refusals warn with [`DEV_OPEN_001`] and the offending
    /// path, then return `None`.
    fn confine(&self, file: &str, canon_root: &Path) -> Option<PathBuf> {
        // `Path::join` passes an absolute right-hand side through, so an
        // absolute `file` outside the root lands here unchanged and the
        // starts_with check refuses it below (CONVENTIONS.md's join rule,
        // in the refusal direction).
        let joined = self.root.join(file);
        let canon = match std::fs::canonicalize(&joined) {
            Ok(c) => c,
            Err(_) => {
                tracing::warn!(
                    code = DEV_OPEN_001,
                    file = %file,
                    "the open request named a path that does not exist under the project root"
                );
                return None;
            }
        };
        if !canon.starts_with(canon_root) {
            tracing::warn!(
                code = DEV_OPEN_001,
                file = %file,
                root = %canon_root.display(),
                "the open request escaped the project root"
            );
            return None;
        }
        if !canon.is_file() {
            tracing::warn!(
                code = DEV_OPEN_001,
                file = %file,
                "the open request named a directory, not a file"
            );
            return None;
        }
        Some(canon)
    }
}

/// The first `key=` value of the query string, percent-decoded (the nav
/// endpoint's parse shape).
fn param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .map(percent_decode)
}

/// The endpoint's error spelling: a plain-text body with the status (the
/// nav endpoint's rejection shape).
fn error_response(status: u16, msg: &str, extra: Option<(&str, &str)>) -> OxideResponse {
    let mut resp = OxideResponse::new(status, bytes::Bytes::from(msg.to_string()))
        .with_header("content-type", "text/plain; charset=utf-8");
    if let Some((k, v)) = extra {
        resp = resp.with_header(k, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_const_is_the_dev_plane_spelling() {
        assert_eq!(OPEN_EDITOR_ENDPOINT_PATH, "/__oxide/open");
    }

    #[test]
    fn build_args_default_shape_matches_the_documented_default() {
        let cmd = resolve_editor(&|_| None).expect("the default resolves");
        let args = cmd.build_args(Path::new("/r/a.rs"), 2);
        assert_eq!(args, vec!["-g", "/r/a.rs:2"]);
    }
}
