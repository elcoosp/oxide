//! TDD pins for the §20 cache-plane exposition slot
//! (`oxide_fw_server::Metrics::set_cache_plane`, spec v0.2 F37 / §20).
//!
//! The request plane's `oxide_cache_l0_*` rows answered the metrics
//! endpoint alone; the cache plane's `oxide_cache_plane_*` families (the
//! F30 exporter's text) had no seat at the scrape. The slot is the
//! server-crate half of that wire: the serve boot hands a closure
//! rendering the cache plane's families — the server crate stays
//! cache-agnostic (the closure is an `Fn() -> String`, no dependency on
//! `oxide-fw-cache`), and `render()` appends the rows beside the
//! request-plane rows. Unattached, the exposition is byte-identical.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `set_cache_plane` existed. The CLI boot attach (the exporter
//! over the boot's `TieredCache`) is the wire-up half — see
//! `docs/deviations.md`.

use oxide_fw_server::Metrics;

// The unattached default: byte-identical to the pre-slot render. Two
// fresh registries (one would have been attached in the wired boot, one
// never) render equal — the slot's absence must not bend a byte.
#[test]
fn the_unattached_registry_renders_byte_identical() {
    let a = Metrics::new().render();
    let b = Metrics::new().render();
    assert_eq!(a, b, "two fresh renders agree");
    assert!(
        !a.contains("cache_plane"),
        "the unattached render names no cache-plane family"
    );
    // The request-plane rows the endpoint already served are untouched.
    assert!(a.contains("# TYPE oxide_requests_total counter"));
    assert!(a.contains("# TYPE oxide_cache_l0_hits_total counter"));
}

// The attached plane appends its closure's text after the request-plane
// rows — one scrape reads BOTH planes, the recorded next half of the F30
// attach story.
#[test]
fn the_attached_plane_appends_its_rows() {
    let m = Metrics::new();
    m.set_cache_plane(|| {
        "# HELP oxide_cache_plane_memory_entries Entries resident in the L0 memory tier\n\
         # TYPE oxide_cache_plane_memory_entries gauge\n\
         oxide_cache_plane_memory_entries 3\n"
            .to_string()
    });
    let text = m.render();
    assert!(
        text.contains("oxide_cache_plane_memory_entries 3"),
        "the cache-plane rows ride the scrape: {text}"
    );
    // Beside, not instead: the request-plane rows still render.
    assert!(text.contains("oxide_requests_total 0"));
    // Appended at the end — the request-plane exposition stays
    // byte-stable above the seam.
    assert!(
        text.ends_with("oxide_cache_plane_memory_entries 3\n"),
        "the cache plane renders last: …{}",
        &text[text.len().saturating_sub(120)..]
    );
}

// The closure runs per render: a scrape is a PULL — every scrape reads
// the posture as it stands NOW (the F32 gauge posture), never a value
// frozen at attach time.
#[test]
fn the_closure_runs_per_scrape() {
    let m = Metrics::new();
    let renders = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = std::sync::Arc::clone(&renders);
    m.set_cache_plane(move || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("oxide_cache_plane_scrape {n}\n")
    });
    let first = m.render();
    let second = m.render();
    assert!(first.contains("oxide_cache_plane_scrape 0"));
    assert!(second.contains("oxide_cache_plane_scrape 1"));
    assert_eq!(
        renders.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "one closure run per render"
    );
}

// A live-posture closure: the value the closure returns changes between
// scrapes without re-attaching — the dashboard diffs postures, not
// renderers (the F32 render contract, on the endpoint).
#[test]
fn a_second_attach_replaces_the_plane() {
    let m = Metrics::new();
    m.set_cache_plane(|| "oxide_cache_plane_from_first 1\n".to_string());
    m.set_cache_plane(|| "oxide_cache_plane_from_second 2\n".to_string());
    let text = m.render();
    assert!(
        text.contains("oxide_cache_plane_from_second 2"),
        "the last attach wins: {text}"
    );
    assert!(
        !text.contains("oxide_cache_plane_from_first"),
        "the replaced closure renders no rows: {text}"
    );
}

// The slot survives the registry's own concurrent traffic: an observe on
// another thread during a render with an attached plane neither deadlocks
// nor drops the plane's rows (the render takes the slot's read lock only).
// The raced route is polled up to a deadline, not read once: on a loaded
// box the render can beat the observing thread's first landing, and the
// pin's intent is "the raced route renders", not a scheduling bet.
#[test]
fn observe_and_render_with_an_attached_plane_coexist() {
    let m = std::sync::Arc::new(Metrics::new());
    m.set_cache_plane(|| "oxide_cache_plane_alive 1\n".to_string());
    let bg = std::sync::Arc::clone(&m);
    let handle = std::thread::spawn(move || {
        for _ in 0..200 {
            bg.observe("/raced", std::time::Duration::from_millis(1), 200);
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let text = loop {
        let rendered = m.render();
        if rendered.contains("oxide_request_duration_ms_bucket")
            || std::time::Instant::now() >= deadline
        {
            break rendered;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    handle.join().expect("the observing thread joins");
    assert!(
        text.contains("oxide_cache_plane_alive 1"),
        "the plane renders under concurrent traffic"
    );
    assert!(
        text.contains("oxide_request_duration_ms_bucket"),
        "the raced route renders"
    );
}
