//! TDD pins for the V2-E08 item 1 open-in-editor endpoint (the server
//! half): the `GET /__oxide/open?file=&line=` dev-plane surface and the
//! `open_editor` module behind it.
//!
//! The spec contract (§V2-E08 item 1 + the §V2.J dev-plane row):
//! - `GET /__oxide/open?file=<path>&line=<n>` opens the file in the
//!   project's editor — `OXIDE_EDITOR`, then `$EDITOR`, then `code -g`.
//! - Dev-token guarded (D41) like the inspector: a wrong or missing token
//!   is a 403 before anything else is decided.
//! - The path is confined to the project root: a `../` escape, an
//!   absolute path outside the root, a directory or a missing file are a
//!   coded refusal (`DEV-OPEN-001`) — the endpoint never opens a file the
//!   project does not own.
//! - The editor launch is an injected [`EditorOpener`] seam: the pipeline
//!   never spawns a process the caller did not choose, and the tests run
//!   the whole story against a recording fake.
//! - The endpoint only exists when attached (`open_editor` is `None` in
//!   the default pipeline — production never attaches it, and the
//!   unattached shape falls through to the normal dispatch, exactly the
//!   inspector's posture).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    DEV_OPEN_001, DEV_OPEN_002, EditorCommand, EditorOpener, OpenEditorEndpoint, OxideRequest,
    OxideResponse, Pipeline, resolve_editor,
};

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` (the `navgraph_tdd.rs` shape).
fn get_with(url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<Vec<_>>(),
        ),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-e08".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate the pipeline pins use — the endpoint arms under
/// test must answer the request themselves, never delegate.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// A recording fake: every launch lands here; `fail` flips the outcome so
/// the failure pin can run without spawning anything.
#[derive(Default)]
struct RecordingOpener {
    calls: Mutex<Vec<EditorCommand>>,
    fail: bool,
}

impl RecordingOpener {
    fn argv(&self) -> Vec<String> {
        let calls = self.calls.lock().expect("opener calls");
        calls
            .last()
            .map(|c| {
                let mut a = vec![c.program.clone()];
                a.extend(c.args.iter().cloned());
                a
            })
            .unwrap_or_default()
    }
}

impl EditorOpener for RecordingOpener {
    fn open(&self, cmd: &EditorCommand) -> Result<(), oxide_fw_diagnostics::OxideError> {
        if self.fail {
            return Err(oxide_fw_diagnostics::OxideError::new(
                DEV_OPEN_002,
                "fake launcher refused (from the pin)",
            ));
        }
        self.calls.lock().expect("opener calls").push(cmd.clone());
        Ok(())
    }
}

/// A project root with one real source file (the canonicalize target) and
/// one nested directory the directory-refusal pin probes.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src").join("a.rs"), "export const a = 1;\n").expect("write");
        Self { _dir: dir, root }
    }

    fn endpoint(&self, fail: bool) -> (OpenEditorEndpoint, Arc<RecordingOpener>) {
        let opener = Arc::new(RecordingOpener {
            fail,
            ..RecordingOpener::default()
        });
        (
            OpenEditorEndpoint::new(
                self.root.clone(),
                Arc::clone(&opener) as Arc<dyn EditorOpener>,
            ),
            opener,
        )
    }
}

// ------------------------------------------------------- the editor resolver

#[test]
fn resolve_editor_prefers_oxide_editor_over_editor() {
    let getenv = |k: &str| match k {
        "OXIDE_EDITOR" => Some("vim -R".into()),
        "EDITOR" => Some("nano".into()),
        _ => None,
    };
    let cmd = resolve_editor(&getenv).expect("an editor resolves");
    assert_eq!(cmd.program, "vim");
    assert_eq!(cmd.args, vec!["-R".to_string()]);
}

#[test]
fn resolve_editor_falls_back_to_editor_then_code_goto() {
    let getenv = |k: &str| (k == "EDITOR").then(|| "nano".to_string());
    let cmd = resolve_editor(&getenv).expect("EDITOR resolves");
    assert_eq!(cmd.program, "nano");
    let none = resolve_editor(&|_| None).expect("the default resolves");
    assert_eq!(none.program, "code");
    assert_eq!(none.args, vec!["-g".to_string()]);
}

#[test]
fn resolve_editor_skips_empty_env_values() {
    // An env var set to whitespace is not an editor choice: the chain
    // falls through instead of spawning an empty program name.
    let getenv = |k: &str| match k {
        "OXIDE_EDITOR" => Some("   ".into()),
        "EDITOR" => Some("".into()),
        _ => None,
    };
    let cmd = resolve_editor(&getenv).expect("the default resolves");
    assert_eq!(cmd.program, "code");
}

// ------------------------------------------------------------- the argv rule

#[test]
fn args_without_placeholders_append_the_goto_shape() {
    // `code -g` (the default) takes the target as one `path:line` arg —
    // the VS Code goto convention the documented default spells.
    let cmd = EditorCommand {
        program: "code".into(),
        args: vec!["-g".into()],
    };
    let args = cmd.build_args(Path::new("/proj/src/a.rs"), 12);
    assert_eq!(
        args,
        vec!["-g".to_string(), "/proj/src/a.rs:12".to_string()]
    );
}

#[test]
fn args_substitute_file_and_line_placeholders() {
    // `vim +{line} {file}` — the documented escape hatch for editors whose
    // goto spelling is not `path:line`.
    let cmd = EditorCommand {
        program: "vim".into(),
        args: vec!["+{line}".into(), "{file}".into()],
    };
    let args = cmd.build_args(Path::new("/proj/src/a.rs"), 7);
    assert_eq!(args, vec!["+7".to_string(), "/proj/src/a.rs".to_string()]);
}

#[test]
fn args_substitute_every_occurrence() {
    let cmd = EditorCommand {
        program: "edit".into(),
        args: vec!["{file}".into(), "--line={line}".into(), "{file}".into()],
    };
    let args = cmd.build_args(Path::new("/p/a.rs"), 3);
    assert_eq!(args[0], "/p/a.rs");
    assert_eq!(args[1], "--line=3");
    assert_eq!(args[2], "/p/a.rs");
}

// --------------------------------------------------------- the pure refusals

#[test]
fn missing_file_param_is_400_with_the_usage_line() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "");
    assert_eq!(resp.status, 400);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains("`file`"), "{body}");
    assert!(opener.argv().is_empty(), "nothing launched");
}

#[test]
fn garbage_line_param_is_400() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/a.rs&line=twelve");
    assert_eq!(resp.status, 400);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains("`line`"), "{body}");
    assert!(opener.argv().is_empty(), "nothing launched");
}

#[test]
fn zero_line_param_is_400() {
    let fx = Fixture::new();
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/a.rs&line=0");
    assert_eq!(resp.status, 400, "editors have no line 0");
}

#[test]
fn dotdot_escape_outside_root_is_422_with_the_code() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=../../etc/passwd");
    assert_eq!(resp.status, 422);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains(DEV_OPEN_001), "{body}");
    assert!(opener.argv().is_empty(), "nothing launched");
}

#[test]
fn absolute_path_outside_root_is_422() {
    let fx = Fixture::new();
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=/etc/passwd");
    assert_eq!(resp.status, 422);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains(DEV_OPEN_001), "{body}");
}

#[test]
fn missing_file_is_422() {
    let fx = Fixture::new();
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/ghost.rs");
    assert_eq!(resp.status, 422);
}

#[test]
fn directory_is_422() {
    let fx = Fixture::new();
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src");
    assert_eq!(resp.status, 422, "a directory is not an editor target");
}

#[test]
fn inside_root_dotdot_normalizes_and_answers() {
    // `..` that STAYS inside the root is a normal path spelling, not an
    // escape: `src/../src/a.rs` opens the same file as `src/a.rs`.
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/../src/a.rs&line=1");
    assert_eq!(resp.status, 200);
    assert_eq!(opener.argv().len(), 3, "code -g <target>");
}

#[test]
fn non_get_is_405_with_allow() {
    let fx = Fixture::new();
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("POST", "file=src/a.rs");
    assert_eq!(resp.status, 405);
    assert_eq!(resp.header("allow"), Some("GET"));
}

// ---------------------------------------------------------------- the happy arm

#[test]
fn success_launches_the_resolved_editor_and_answers_200() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/a.rs&line=42");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let argv = opener.argv();
    assert_eq!(argv[0], "code", "the default editor");
    assert_eq!(argv[1], "-g");
    assert!(
        argv[2].ends_with(&format!(
            "{}:42",
            fx.root.join("src").join("a.rs").display()
        )),
        "the goto arg carries the canonical absolute path and the line: {argv:?}"
    );
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("the body is JSON");
    assert_eq!(body["ok"], serde_json::Value::Bool(true));
    assert_eq!(body["editor"], "code");
    assert_eq!(body["line"], 42);
    let file = body["file"].as_str().expect("file in the body");
    assert!(
        !Path::new(file).is_absolute(),
        "the body names the root-relative path"
    );
    assert_eq!(file, "src/a.rs");
}

#[test]
fn line_defaults_to_one() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/a.rs");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("the body is JSON");
    assert_eq!(body["line"], 1);
    let argv = opener.argv();
    assert!(argv[2].ends_with(":1"), "{argv:?}");
}

#[test]
fn file_param_is_percent_decoded() {
    let fx = Fixture::new();
    std::fs::write(fx.root.join("src").join("my file.rs"), "x\n").expect("write");
    let (ep, _opener) = fx.endpoint(false);
    let resp = ep.serve("GET", "file=src/my%20file.rs");
    assert_eq!(
        resp.status, 200,
        "the encoded space decoded to the real file"
    );
}

#[test]
fn opener_failure_is_500_with_the_code() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(true);
    let resp = ep.serve("GET", "file=src/a.rs");
    assert_eq!(resp.status, 500);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains(DEV_OPEN_002), "{body}");
    assert!(
        opener.argv().is_empty(),
        "the failure is not recorded as a launch"
    );
}

// ------------------------------------------------------------- the pipeline arm

#[tokio::test]
async fn pipeline_without_the_endpoint_falls_through() {
    // The default pipeline never attaches the endpoint: the request is
    // NOT answered by a 404-shaped arm — it falls through to the normal
    // dispatch (the inspector's posture; production is byte-identical).
    let fx = Fixture::new();
    let p = Pipeline::default();
    let resp = p
        .serve_request(
            get_with("/__oxide/open?file=src/a.rs", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 500, "the dispatch delegate ran (fall-through)");
    let _ = fx.root.clone();
}

#[tokio::test]
async fn pipeline_answers_200_with_the_token() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let mut p = Pipeline::default().with_editor_opener(Arc::new(ep));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get_with("/__oxide/open?file=src/a.rs&line=3&token=tok", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200, "the token rides the query");
    assert!(!opener.argv().is_empty(), "the launch happened");
}

#[tokio::test]
async fn pipeline_refuses_a_wrong_or_missing_token_with_403() {
    let fx = Fixture::new();
    let (ep, opener) = fx.endpoint(false);
    let mut p = Pipeline::default().with_editor_opener(Arc::new(ep));
    p.dev_token = Some("tok".into());
    let missing = p
        .serve_request(
            get_with("/__oxide/open?file=src/a.rs", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(missing.status, 403);
    let wrong = p
        .serve_request(
            get_with(
                "/__oxide/open?file=src/a.rs",
                &[("x-oxide-fw-dev-token", "nope")],
            ),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(wrong.status, 403);
    assert!(opener.argv().is_empty(), "no launch on either refusal");
}
