//! Walkthrough: the V2-E08 open-in-editor contract, end to end — the
//! consumer `oxide-fw-server` never had for this seam.
//!
//! The dev overlay's "jump to the file" half: `GET
//! /__oxide/open?file=&line=` (the §V2.J map's dev-plane row next to the
//! inspector) launches the project's editor on the requested file — but
//! only after the dev-token guard, and only on a file the project root
//! owns. The endpoint is attached to the pipeline like the inspector:
//!
//! ```ignore
//! let ep = OpenEditorEndpoint::new(app_root, Arc::new(CommandOpener));
//! let pipeline = Pipeline::default().with_editor_opener(Arc::new(ep));
//! pipeline.dev_token = Some(oxide_fw_server::dev_token());
//! ```
//!
//! This example walks the full story against a PRINTING launcher (no
//! real editor spawns): the editor resolution chain, the confinement
//! refusals (`../`, outside-root, directory, missing), the param and
//! method contracts, and the happy path the overlay rides.
//!
//! Run: `cargo run -p oxide-fw-server --example open_editor`

use std::sync::{Arc, Mutex};

use oxide_fw_server::{
    EditorCommand, EditorOpener, OpenEditorEndpoint, OxideRequest, OxideResponse, Pipeline,
    resolve_editor,
};

/// A launcher that prints the argv instead of spawning (the story runs
/// anywhere — headless CI included).
struct PrintingOpener {
    launches: Mutex<Vec<String>>,
}

impl EditorOpener for PrintingOpener {
    fn open(&self, cmd: &EditorCommand) -> Result<(), oxide_fw_diagnostics::OxideError> {
        let mut argv = vec![cmd.program.clone()];
        argv.extend(cmd.args.iter().cloned());
        let line = argv.join(" ");
        println!("  launch: {line}");
        self.launches.lock().expect("launches").push(line);
        Ok(())
    }
}

/// A minimal request helper (the `navgraph_tdd.rs` shape).
fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: std::sync::OnceLock::default(),
        body: bytes::Bytes::new(),
        request_id: "ex-e08".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — an attached endpoint must never need it (the
/// `serve_request` boxed-future shape).
fn dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

fn main() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
    std::fs::write(
        root.path().join("src").join("page.tsx"),
        "export default () => null;\n",
    )
    .expect("write");

    // 1. The editor chain: OXIDE_EDITOR, then EDITOR, then the documented
    //    default (`code -g`). The value splits on whitespace and may carry
    //    the {file}/{line} placeholders for non-`path:line` goto spellings.
    let getenv = |k: &str| (k == "OXIDE_EDITOR").then(|| "vim +{line} {file}".to_string());
    let editor = resolve_editor(&getenv).expect("the env resolves");
    println!(
        "resolve: OXIDE_EDITOR={:?} -> program {:?}, args {:?} (default without env: code -g)",
        "vim +{line} {file}", editor.program, editor.args
    );

    // 2. The endpoint over the fixture root with the printing launcher.
    let opener = Arc::new(PrintingOpener {
        launches: Mutex::new(Vec::new()),
    });
    let ep = OpenEditorEndpoint::new(root.path(), Arc::clone(&opener) as Arc<dyn EditorOpener>);

    // 3. The happy path: the file is canonicalized inside the root and the
    //    launcher receives the concrete argv.
    println!("\nsuccess: GET /__oxide/open?file=src/page.tsx&line=42");
    let resp = ep.serve("GET", "file=src/page.tsx&line=42");
    println!(
        "  -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    // 4. The refusals — each is a coded verdict, nothing launches:
    println!("\nrefusals:");
    for (label, query) in [
        ("the ../ escape", "file=../../etc/passwd"),
        ("an absolute path outside the root", "file=/etc/passwd"),
        ("a directory", "file=src"),
        ("a missing file", "file=src/ghost.tsx"),
        ("a garbage line", "file=src/page.tsx&line=zero"),
        ("a missing file param", ""),
    ] {
        let resp = ep.serve("GET", query);
        println!(
            "  {label}: {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
                .lines()
                .next()
                .unwrap_or("")
        );
    }

    // 5. The method contract (405 + allow) — the nav endpoint's shape.
    let resp = ep.serve("POST", "file=src/page.tsx");
    println!(
        "\nmethod: POST -> {} (allow: {})",
        resp.status,
        resp.header("allow").unwrap_or("?")
    );

    // 6. The pipeline posture: attached answers with the token, a wrong
    //    token is a 403 BEFORE anything else, and the unattached pipeline
    //    falls through (production byte-identical).
    println!("\npipeline:");
    let mut p = Pipeline::default().with_editor_opener(Arc::new(OpenEditorEndpoint::new(
        root.path(),
        Arc::clone(&opener) as Arc<dyn EditorOpener>,
    )));
    p.dev_token = Some("tok".into());
    for (label, url) in [
        (
            "with the token",
            "/__oxide/open?file=src/page.tsx&line=1&token=tok",
        ),
        ("wrong token", "/__oxide/open?file=src/page.tsx&token=nope"),
        ("missing token", "/__oxide/open?file=src/page.tsx"),
    ] {
        let resp = futures::executor::block_on(p.serve_request(get(url), &dispatch()));
        println!("  {label}: {}", resp.status);
    }
    let unattached = Pipeline::default();
    let resp = futures::executor::block_on(unattached.serve_request(
        get("/__oxide/open?file=src/page.tsx&token=tok"),
        &dispatch(),
    ));
    println!(
        "  unattached pipeline: {} (the dispatch ran — fall-through)",
        resp.status
    );

    let launches = opener.launches.lock().expect("launches").len();
    println!("\nlaunches total: {launches} (the refusals never reached the launcher)");
}
