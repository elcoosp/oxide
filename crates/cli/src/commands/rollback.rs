//! V2-F10 step 4: `oxide rollback` — the served-dist rollback over the
//! retained generations the build's emit tail keeps
//! (`.oxide/generations/<id>/dist`, the spec's "last N builds").
//!
//! The spec's own words: "`oxide start --generation previous` or
//! `oxide rollback` repoints `dist` and sends `SIGHUP`. No rebuild." The
//! swap is the generations module's [`repoint_dist_to_generation`] (the
//! same one `oxide start --generation` performs before its boot); the
//! SIGHUP receiver is the pid file the serve boot writes
//! (`.oxide/serve.pid`). A non-unix host cannot signal — the repoint
//! still lands and the output says a restart is required instead of
//! pretending the signal was delivered.

use crate::*;

/// `oxide rollback` — repoint `dist/` at a retained generation (§V2-F10
/// step 4) and hang the running serve up so its promote picks the
/// rolled-back id up.
///
/// The selector defaults to `previous` — the newest retained build that
/// is not the one `dist/` currently serves. A serve that is not running
/// (or a pid file that names a dead process) still rolls the tree over:
/// the repoint is the durable half, the signal only wakes a live
/// receiver. Exit codes: 0 when the tree swapped (signalled or not),
/// 1 when nothing was retained to roll back to (an operational failure,
/// not a usage error — the command was spelled correctly).
pub(crate) async fn cmd_rollback(
    root: &str,
    generation: Option<&str>,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let dist = rootp.join("dist");
    let selector = generation.unwrap_or("previous");
    let current = oxide_fw_cache::read_build_id(&dist);
    let (gen_dist, swapped_from) =
        match crate::serve::generations::resolve_generation(&rootp, selector, current.as_deref()) {
            Ok(path) => (path, current),
            Err(e) => {
                outln!(w, "rollback failed: {e}");
                return Ok(1);
            }
        };
    if let Err(e) = crate::serve::generations::repoint_dist_to_generation(&rootp, &gen_dist) {
        outln!(w, "rollback failed: {e}");
        return Ok(1);
    }
    let Some(served_id) = oxide_fw_cache::read_build_id(&dist) else {
        // Unreachable through the swap (the target carries a build id by
        // contract), but the honest arm: the printed verdict must name a
        // real id, never a guess.
        outln!(w, "rollback failed: the swapped dist carries no build id");
        return Ok(1);
    };
    outln!(
        w,
        "rollback ok — dist serves build-id {served_id}{}",
        match swapped_from {
            Some(from) => format!(" (was {from})"),
            None => String::new(),
        }
    );
    signal_running_serve(&rootp, w)?;
    Ok(0)
}

/// The §V2-F10 step-4 signal half: hang the running serve up (its
/// promote re-reads the repointed dist and demotes the pre-rollback
/// generation into its skew window). The receiver is the pid file the
/// serve boot writes; a missing or dead receiver is reported, never
/// guessed at.
#[cfg(unix)]
fn signal_running_serve(rootp: &std::path::Path, w: Writer<'_>) -> anyhow::Result<()> {
    let pid_path = rootp.join(".oxide").join("serve.pid");
    let Ok(pid) = std::fs::read_to_string(&pid_path) else {
        outln!(
            w,
            "rollback: no serve pid file — dist repointed; start to serve it"
        );
        return Ok(());
    };
    let pid = pid.trim().to_string();
    if pid.is_empty() {
        outln!(
            w,
            "rollback: the serve pid file is empty — dist repointed; start to serve it"
        );
        return Ok(());
    }
    let quiet = |mut cmd: std::process::Command| {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd.status().map(|s| s.success()).unwrap_or(false)
    };
    let mut probe = std::process::Command::new("kill");
    probe.arg("-0").arg(&pid);
    let alive = quiet(probe);
    if !alive {
        outln!(
            w,
            "rollback: the pid file names {pid} but nothing answers — dist repointed; start to serve it"
        );
        return Ok(());
    }
    let mut hup = std::process::Command::new("kill");
    hup.arg("-HUP").arg(&pid);
    if quiet(hup) {
        outln!(
            w,
            "rollback: SIGHUP delivered to serve pid {pid} — its promote picks the rolled-back id up"
        );
    } else {
        outln!(
            w,
            "rollback: the serve pid {pid} refused the signal — restart it to serve the rolled-back dist"
        );
    }
    Ok(())
}

/// The non-unix arm: no signals exist to send, so the output says the
/// restart word instead of pretending the promote was woken.
#[cfg(not(unix))]
fn signal_running_serve(rootp: &std::path::Path, w: Writer<'_>) -> anyhow::Result<()> {
    let _ = rootp;
    outln!(
        w,
        "rollback: dist repointed — restart the server to serve it (signals are unix-only)"
    );
    Ok(())
}
