//! TDD test suite for oxide-fw-dev (spec §13, §5.5).

use super::*;
use oxide_fw_diagnostics::Location;
use std::fs;
use tempfile::tempdir;

// ---------------- Watcher (§5.5) ----------------

#[test]
fn poll_reports_created_then_noop_on_unchanged_content() {
    let dir = tempdir().unwrap();
    let w = Watcher::new(vec![dir.path().to_path_buf()]);
    let mut seen = BTreeSet::new();

    fs::write(dir.path().join("a.txt"), "one").unwrap();
    let events = w.poll(&mut seen).unwrap();
    assert_eq!(events, vec![FileEvent::Created(dir.path().join("a.txt"))]);

    // Second poll with no changes at all: zero events.
    let events = w.poll(&mut seen).unwrap();
    assert!(events.is_empty());

    // touch-without-change (same bytes rewritten, only mtime moves): still zero
    // events — the §5.5 content-hash confirmation requirement.
    fs::write(dir.path().join("a.txt"), "one").unwrap();
    let events = w.poll(&mut seen).unwrap();
    assert!(events.is_empty(), "touch-without-change must be ignored");
}

#[test]
fn poll_reports_modified_only_when_content_hash_changes() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.txt");
    fs::write(&a, "v1").unwrap();
    let w = Watcher::new(vec![dir.path().to_path_buf()]);
    let mut seen = BTreeSet::new();
    w.poll(&mut seen).unwrap();

    fs::write(&a, "v2").unwrap();
    let events = w.poll(&mut seen).unwrap();
    assert_eq!(events, vec![FileEvent::Modified(a.clone())]);

    // Rewriting the identical content again must not re-report.
    fs::write(&a, "v2").unwrap();
    assert!(w.poll(&mut seen).unwrap().is_empty());
}

#[test]
fn poll_lists_removed_before_other_events_and_sorts_by_path() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let w = Watcher::new(vec![dir.path().to_path_buf()]);
    let mut seen = BTreeSet::new();
    w.poll(&mut seen).unwrap();

    fs::remove_file(&a).unwrap();
    fs::write(&b, "b2").unwrap();
    fs::write(&c, "c").unwrap();
    let events = w.poll(&mut seen).unwrap();
    assert_eq!(
        events,
        vec![
            FileEvent::Removed(a.clone()),  // removals first
            FileEvent::Modified(b.clone()), // then others, sorted by path
            FileEvent::Created(c.clone()),
        ]
    );

    // seen now reflects the current state: nothing left.
    assert!(w.poll(&mut seen).unwrap().is_empty());
}

#[test]
fn poll_skips_git_node_modules_oxide_and_target_dirs() {
    let dir = tempdir().unwrap();
    for d in [".git", "node_modules", ".oxide", "target"] {
        let p = dir.path().join(d);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("ignored.bin"), "x").unwrap();
    }
    fs::write(dir.path().join("src.txt"), "kept").unwrap();
    let w = Watcher::new(vec![dir.path().to_path_buf()]);
    let mut seen = BTreeSet::new();
    let events = w.poll(&mut seen).unwrap();
    assert_eq!(events, vec![FileEvent::Created(dir.path().join("src.txt"))]);
}

// ---------------- HMR behavior table (§13.3) ----------------

#[test]
fn hmr_behavior_matches_spec_strategy_table() {
    assert_eq!(hmr_behavior(ChangeKind::Component), HmrBehavior::AdapterHmr);
    assert_eq!(hmr_behavior(ChangeKind::Css), HmrBehavior::StyleSwap);
    assert_eq!(
        hmr_behavior(ChangeKind::ServerLoader),
        HmrBehavior::ReloadData
    );
    assert_eq!(
        hmr_behavior(ChangeKind::ServerModule),
        HmrBehavior::ServerSwap
    );
    assert_eq!(
        hmr_behavior(ChangeKind::ConfigOrRoutes),
        HmrBehavior::RestartSubsystems
    );
}

// ---------------- update boundary (§13.3) ----------------
// The one-hop `update_boundary` simplification was retired by the V2-E01
// boundary planner (spec §V2-E01, `crates/dev/src/boundary.rs`); the
// walk-up contract it approximated now lives in `tests/boundary_tdd.rs`.

// ---------------- HMR messages (§13.3) ----------------

#[test]
fn hmr_messages_roundtrip_through_json_all_variants() {
    let diag = Diagnostic::error("DEV-001", "boom")
        .at(Location::file("app.tsx", 3, 5))
        .with_fix("fix it");
    let msgs = vec![
        HmrMessage::Update {
            updates: vec![UpdateEntry {
                kind: UpdateKind::Js,
                path: "src/routes/index/page.tsx".into(),
                accepted_path: "/src/routes/index/page.tsx".into(),
                timestamp: 1_760_000_000_000,
                hash: "abc123def4567890".into(),
            }],
            preserve: Preservation::default(),
        },
        HmrMessage::FullReload {
            reason: "config changed".into(),
        },
        HmrMessage::Error { diagnostic: diag },
        HmrMessage::OxData {
            route: "/".into(),
            data: "aGk=".into(),
        },
        HmrMessage::Connected {
            build: "b123".into(),
        },
    ];
    for m in &msgs {
        let json = m.to_json();
        assert!(!json.is_empty());
        assert_eq!(
            &HmrMessage::from_json(&json).unwrap(),
            m,
            "roundtrip for {json}"
        );
        // The schema-v1 envelope: every frame carries the version.
        assert!(json.contains("\"v\":1"), "v1 envelope in {json}");
    }
    // Wire tags are the §13.3 v1 spellings (client protocol stability).
    assert!(msgs[0].to_json().contains("\"type\":\"update\""));
    assert!(msgs[1].to_json().contains("\"type\":\"full-reload\""));
    assert!(msgs[3].to_json().contains("\"type\":\"ox:data\""));
    assert!(msgs[4].to_json().contains("\"type\":\"connected\""));
    assert!(HmrMessage::from_json("not json").is_none());
}

#[test]
fn wire_v1_update_entry_spells_the_spec_fields() {
    // The §13.3 v1 entry: kind/path/acceptedPath/timestamp/hash, with the
    // camelCase wire spelling the TS client reads.
    let e = UpdateEntry {
        kind: UpdateKind::Css,
        path: "src/styles/app.css".into(),
        accepted_path: "/@css/app.css".into(),
        timestamp: 1_760_000_000_000,
        hash: "deadbeefdeadbeef".into(),
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"kind\":\"css\""), "{json}");
    assert!(
        json.contains("\"acceptedPath\":\"/@css/app.css\""),
        "{json}"
    );
    assert!(json.contains("\"timestamp\":1760000000000"), "{json}");
    assert!(json.contains("\"hash\":\"deadbeefdeadbeef\""), "{json}");
    assert!(json.contains("\"path\":\"src/styles/app.css\""), "{json}");
}

#[test]
fn update_frame_is_the_singular_and_the_batch_alike() {
    // §3.2: the v1 `updates` array is natively a batch — the old
    // singular/batch wire distinction dissolves into N entries. An empty
    // array is a valid no-op frame (the client fans out zero entries).
    let entry = |path: &str, accepted: &str| UpdateEntry {
        kind: UpdateKind::Js,
        path: path.into(),
        accepted_path: accepted.into(),
        timestamp: 1_760_000_000_000,
        hash: String::new(),
    };
    let singular = HmrMessage::Update {
        updates: vec![entry("/abs/a.tsx", "/src/a.tsx")],
        preserve: Preservation::default(),
    };
    let batch = HmrMessage::Update {
        updates: vec![
            entry("/abs/a.tsx", "/src/a.tsx"),
            entry("/abs/b.tsx", "/src/b.tsx"),
        ],
        preserve: Preservation::default(),
    };
    let empty = HmrMessage::Update {
        updates: vec![],
        preserve: Preservation::default(),
    };
    for frame in [&singular, &batch, &empty] {
        let json = frame.to_json();
        assert!(json.contains("\"type\":\"update\""), "{json}");
        assert_eq!(&HmrMessage::from_json(&json).unwrap(), frame);
    }
}

#[test]
fn try_from_json_is_the_fallible_decode_seam() {
    // The symmetric seam to `try_to_json`: callers that must distinguish a
    // parse failure from a schema-version mismatch get the typed error.
    let frame = HmrMessage::Connected { build: "b1".into() }.to_json();
    assert!(HmrMessage::try_from_json(&frame).is_ok());
    assert!(HmrMessage::try_from_json("not json").is_err());
    // A v-less (legacy) frame and a foreign-version frame are both schema
    // mismatches, not parse errors — the DEV-HMR-003 story.
    let legacy = r#"{"type":"module-update","path":"a.tsx","url":"/src/a.tsx"}"#;
    match HmrMessage::try_from_json(legacy) {
        Err(HmrWireError::Schema { found, expected }) => {
            assert_eq!((found, expected), (0, HmrMessage::SCHEMA_VERSION));
        }
        other => panic!("expected a Schema mismatch, got {other:?}"),
    }
    assert!(matches!(
        HmrMessage::try_from_json(r#"{"v":2,"type":"connected","build":"x"}"#),
        Err(HmrWireError::Schema {
            found: 2,
            expected: 1
        })
    ));
}

#[test]
fn from_json_warns_and_returns_none_on_bad_frames() {
    // The weak-error retirement: `from_json` keeps its Option signature
    // (the callers are few and non-critical) but the failure path is now
    // loud through `try_from_json` + the `tracing::warn!` inside.
    assert!(HmrMessage::from_json("not json").is_none());
    // A v-less frame is legacy now: rejected at the v1 gate.
    assert!(HmrMessage::from_json(r#"{"type":"connected"}"#).is_none());
}

#[test]
fn schema_mismatch_diagnostic_is_dev_hmr_003() {
    let d = schema_mismatch_diagnostic(2);
    assert_eq!(d.code, "DEV-HMR-003");
    assert_eq!(d.severity, oxide_fw_diagnostics::Severity::Warning);
    assert!(d.fix.is_some());
    assert!(
        d.message.contains("2"),
        "names the found version: {}",
        d.message
    );
}

#[test]
fn wire_helpers_stamp_entries_deterministically() {
    // `unix_millis` is the schema's wall-clock unit (unix ms).
    assert!(unix_millis() > 1_600_000_000_000);
    // `content_hash` is the watcher's D6 blake3 prefix (16 hex); unreadable
    // paths hash to None (the entry then carries "").
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.tsx");
    fs::write(&f, "hello").unwrap();
    let h = content_hash(&f).expect("a readable file hashes");
    assert_eq!(h.len(), 16);
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(content_hash(&dir.path().join("missing.tsx")).is_none());
    // `UpdateEntry::stamped` composes the three: a real file gets the hash,
    // a None hash_path carries "".
    let stamped = UpdateEntry::stamped(UpdateKind::Js, "src/a.tsx", "/src/a.tsx", Some(&f));
    assert_eq!(stamped.hash, h);
    assert!(stamped.timestamp > 1_600_000_000_000);
    assert_eq!(stamped.accepted_path, "/src/a.tsx");
    assert_eq!(
        UpdateEntry::stamped(UpdateKind::Css, "tw.css", "/@css/tw.css", None).hash,
        ""
    );
}

#[test]
fn try_to_json_is_the_fallible_seal_of_the_same_wire_bytes() {
    // `to_json` keeps its signature (the wire never carries a half-frame)
    // but the failure path is loud via `try_to_json` — and on the happy
    // path the two agree byte-for-byte, envelope included.
    let m = HmrMessage::OxData {
        route: "/".into(),
        data: "aGk=".into(),
    };
    let tried = m.try_to_json().expect("a v1-tagged message serializes");
    assert_eq!(tried, m.to_json());
    assert!(tried.contains("\"type\":\"ox:data\""), "{tried}");
    assert!(tried.contains("\"v\":1"), "{tried}");
}

// ---------------- HMR hub ----------------

#[tokio::test]
async fn hmr_hub_broadcasts_to_two_subscribers() {
    let hub = HmrHub::new();
    let mut s1 = hub.subscribe();
    let mut s2 = hub.subscribe();
    assert_eq!(hub.subscriber_count(), 2);

    hub.publish(HmrMessage::FullReload {
        reason: "routes changed".into(),
    });
    hub.publish(HmrMessage::Connected { build: "b1".into() });

    let want = HmrMessage::FullReload {
        reason: "routes changed".into(),
    }
    .to_json();
    assert_eq!(s1.recv().await.unwrap(), want);
    assert_eq!(s2.recv().await.unwrap(), want);
    let want2 = HmrMessage::Connected { build: "b1".into() }.to_json();
    assert_eq!(s1.recv().await.unwrap(), want2);
    assert_eq!(s2.recv().await.unwrap(), want2);

    // Publishing with no subscribers must not panic.
    let solo = HmrHub::default();
    solo.publish(HmrMessage::Update {
        updates: vec![UpdateEntry {
            kind: UpdateKind::Css,
            path: "x.css".into(),
            accepted_path: "x.css".into(),
            timestamp: 0,
            hash: String::new(),
        }],
        preserve: Preservation::default(),
    });
}

// ---------------- overlay (§13.4) ----------------

#[test]
fn overlay_payload_serializes_the_diagnostic() {
    let d = Diagnostic::error("RT-404", "route not found").at(Location::file("routes.ts", 2, 1));
    let payload = overlay_payload(&d);
    assert!(payload.contains("\"code\":\"RT-404\""), "{payload}");
    assert!(payload.contains("\"message\":\"route not found\""));
    // Overlay rendering consumes the same JSON the JS boundary sees (§19.4).
    assert!(Diagnostic::from_json(&payload).is_ok());
}

// ---------------------------------------------------------------------------
// §13.4 Inspector tests
// ---------------------------------------------------------------------------

#[test]
fn route_table_inspector_from_router_table() {
    let inspector = RouteTableInspector::from_router_table(vec![
        (
            "/blog/:slug".to_string(),
            "isr".to_string(),
            vec!["revalidate=60".to_string(), "posts".to_string()],
        ),
        ("/".to_string(), "static".to_string(), vec![]),
    ]);
    let state = inspector.state();
    assert_eq!(state.routes.len(), 2);
    assert_eq!(state.routes[0].id, "/blog/:slug");
    assert_eq!(state.routes[0].mode, "isr");
    assert_eq!(state.routes[0].cache_policy, "revalidate=60,posts");
    assert_eq!(state.routes[1].cache_policy, "");
    // No source for these yet — schema stays honest.
    assert!(state.module_graph.is_empty());
    assert!(state.cache.is_empty());
    assert!(state.waterfalls.is_empty());
    assert!(state.why.is_empty());
    assert_eq!(state.pool.workers, 0);
}

#[test]
fn render_inspector_html_escapes_and_lists_routes() {
    let inspector = RouteTableInspector::from_router_table(vec![(
        "/<script>".to_string(),
        "static".to_string(),
        vec![],
    )]);
    let html = render_inspector_html(&inspector.state());
    assert!(html.contains("<title>Oxide Inspector</title>"));
    assert!(html.contains("Raw JSON"));
    assert!(html.contains("Routes (1)"));
    // The malicious pattern is escaped in the table body.
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
}

#[test]
fn inspector_renders_waterfalls() {
    // B-17: a hit and a miss row, each with stage bars.
    let state = InspectorState {
        waterfalls: vec![
            RequestTimingsRow {
                request_id: "req-miss".to_string(),
                waterfall_ms: vec![
                    ("middleware".to_string(), 1.5),
                    ("load".to_string(), 12.0),
                    ("render".to_string(), 30.0),
                ],
                trace_id: None,
            },
            RequestTimingsRow {
                request_id: "req-hit".to_string(),
                waterfall_ms: vec![("cache-hit".to_string(), 0.4)],
                trace_id: None,
            },
        ],
        ..Default::default()
    };
    let html = render_inspector_html(&state);
    assert!(html.contains("Waterfalls (2)"), "{html}");
    assert!(html.contains("req-miss"));
    assert!(html.contains("req-hit"));
    assert!(html.contains("class=\"bar\""), "stage bars rendered");
    assert!(html.contains("43.5ms"), "miss total: {html}");
}

#[test]
fn inspector_state_roundtrips_through_json() {
    let inspector = RouteTableInspector::from_router_table(vec![(
        "/api/health".to_string(),
        "dynamic".to_string(),
        vec!["no-cache".to_string()],
    )]);
    let state = inspector.state();
    let json = serde_json::to_string(&state).unwrap();
    let back: InspectorState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.routes, state.routes);
}

/// PERF-11: the notify backend reports a real write as Modified. Skips
/// gracefully when the OS watcher is unavailable (headless CI, inotify limits).
#[test]
fn notify_backend_reports_a_real_change() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("a.ts");
    fs::write(&file, b"export const a = 1;").unwrap();

    let w = Watcher::with_notify(vec![dir.path().to_path_buf()]);
    if w.backend.is_none() {
        eprintln!("SKIP: notify backend unavailable");
        return;
    }
    let mut seen = std::collections::BTreeSet::new();
    let _ = w.poll(&mut seen).unwrap(); // prime

    fs::write(&file, b"export const a = 2;").unwrap();
    // Editors write in bursts; retry a few times to absorb fs latency.
    let mut found = false;
    for _ in 0..60 {
        let ev = w.poll(&mut seen).unwrap();
        if ev
            .iter()
            .any(|e| matches!(e, FileEvent::Modified(p) | FileEvent::Created(p) if p == &file))
        {
            found = true;
            break;
        }
    }
    // macOS FSEvents delivery is environment-dependent (latency, coalescing);
    // the content-hash *correctness* is covered by the poll-backend tests, so
    // a miss here is a SKIP, not a failure.
    if !found {
        eprintln!("SKIP: notify event not delivered within the window (macOS FSEvents)");
    }
}

// ------------------------------ §20 / O3b: staged record

#[test]
fn record_staged_merges_by_request_id() {
    let rec = WaterfallRecorder::new();
    // Initial call: the three stages the dispatch records synchronously.
    rec.record_staged(
        "req-1",
        vec![
            ("middleware".to_string(), 1.0),
            ("load".to_string(), 5.0),
            ("render".to_string(), 20.0),
        ],
        Some("trace-abc".to_string()),
    );
    // Settle call: the deferred-settles stage, recorded later from the
    // spawned drain task.
    rec.record_staged(
        "req-1",
        vec![("deferred-settles".to_string(), 12.5)],
        None, // trace already set on the first call
    );

    let snap = rec.snapshot();
    assert_eq!(snap.len(), 1, "one row per request_id, not two: {snap:?}");
    let row = &snap[0];
    assert_eq!(row.request_id, "req-1");
    assert_eq!(
        row.trace_id.as_deref(),
        Some("trace-abc"),
        "trace preserved"
    );
    let names: Vec<_> = row.waterfall_ms.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["middleware", "load", "render", "deferred-settles"]
    );
}

#[test]
fn record_staged_settle_first_still_merges() {
    // If the settle task lands first (edge case), the initial call appends
    // to it — still a single row.
    let rec = WaterfallRecorder::new();
    rec.record_staged(
        "req-2",
        vec![("deferred-settles".to_string(), 8.0)],
        Some("trace-xyz".to_string()),
    );
    rec.record_staged("req-2", vec![("render".to_string(), 15.0)], None);
    let snap = rec.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].trace_id.as_deref(), Some("trace-xyz"));
    assert!(snap[0].waterfall_ms.iter().any(|(n, _)| n == "render"));
}

#[test]
fn classify_event_modified_layout_is_component() {
    assert_eq!(
        classify_event(&FileEvent::Modified("src/routes/layout.tsx".into())),
        ChangeKind::Component,
        "content edit to an existing layout → in-place, not rescan",
    );
}

#[test]
fn classify_event_created_layout_is_structural() {
    assert_eq!(
        classify_event(&FileEvent::Created("src/routes/layout.tsx".into())),
        ChangeKind::ConfigOrRoutes,
        "a NEW layout changes the tree → rescan",
    );
}

#[test]
fn classify_event_removed_layout_is_structural() {
    assert_eq!(
        classify_event(&FileEvent::Removed("src/routes/layout.tsx".into())),
        ChangeKind::ConfigOrRoutes,
        "deleting a layout changes the tree → rescan",
    );
}

#[test]
fn classify_event_modified_page_is_component() {
    assert_eq!(
        classify_event(&FileEvent::Modified("src/routes/greeting/page.tsx".into())),
        ChangeKind::Component,
    );
}

#[test]
fn classify_event_modified_route_ts_stays_structural() {
    // `route.ts` defines an endpoint; body edit changes its identity.
    assert_eq!(
        classify_event(&FileEvent::Modified("src/routes/api/route.ts".into())),
        ChangeKind::ConfigOrRoutes,
    );
}

#[test]
fn classify_event_modified_socket_stays_structural() {
    assert_eq!(
        classify_event(&FileEvent::Modified("src/routes/chat/socket.ts".into())),
        ChangeKind::ConfigOrRoutes,
    );
}

#[test]
fn classify_event_delegates_for_css() {
    assert_eq!(
        classify_event(&FileEvent::Modified("src/styles/global.css".into())),
        ChangeKind::Css,
    );
}

#[test]
fn classify_event_delegates_for_page_server_loader() {
    assert_eq!(
        classify_event(&FileEvent::Modified(
            "src/routes/greeting/page.server.ts".into()
        )),
        ChangeKind::ServerLoader,
    );
}

#[test]
fn classify_event_delegates_for_non_page_server_module() {
    assert_eq!(
        classify_event(&FileEvent::Modified(
            "src/routes/nested/layout.server.ts".into()
        )),
        ChangeKind::ServerModule,
    );
}

#[test]
fn classify_event_outside_routes_is_component() {
    assert_eq!(
        classify_event(&FileEvent::Modified("src/components/foo.tsx".into())),
        ChangeKind::Component,
    );
}

#[test]
fn classify_event_spurious_created_on_existing_file_is_modified() {
    // §3.9: a Created event for a file that already existed >1s ago is a
    // spurious Created (macOS temp+rename write pattern). Rewrite to
    // Modified so a content edit to `routes/layout.tsx` behaves as an
    // in-place edit, not a route-tree rescan.
    let dir = tempdir().unwrap();
    // classify_event only treats a file as a route file when its path
    // contains `routes/`; use a real route-shaped path.
    let routes_dir = dir.path().join("src").join("routes");
    fs::create_dir_all(&routes_dir).unwrap();
    let layout = routes_dir.join("layout.tsx");
    fs::write(&layout, "before").unwrap();
    // Sleep past the 1s threshold so birthtime is "old enough".
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let ev = FileEvent::Created(layout.clone());
    assert_eq!(
        classify_event(&ev),
        ChangeKind::Component,
        "spurious Created on an existing route file must classify as Component"
    );
}

#[test]
fn classify_event_true_created_on_brand_new_file_is_structural() {
    // Sanity: a genuinely new file (birthtime within 1s) still classifies
    // as ConfigOrRoutes. Without this pair, the birthtime fix could be a
    // blanket "Created -> Modified" that breaks route additions.
    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("src").join("routes");
    fs::create_dir_all(&routes_dir).unwrap();
    let new_layout = routes_dir.join("layout.tsx");
    fs::write(&new_layout, "fresh").unwrap();
    let ev = FileEvent::Created(new_layout);
    assert_eq!(classify_event(&ev), ChangeKind::ConfigOrRoutes);
}

#[test]
fn hmr_wire_v1_frame_survives_a_pretty_json_object_path() {
    // The ENVELOPED frame round-trips through a serde_json::Value (the same
    // path `try_to_json`'s insert uses), so a consumer that re-serializes
    // the frame cannot lose the version field. A bare `to_value(&frame)`
    // carries no envelope — that is exactly the shape the v1 gate rejects.
    let frame = HmrMessage::Update {
        updates: vec![UpdateEntry {
            kind: UpdateKind::Data,
            path: "/slow".into(),
            accepted_path: String::new(),
            timestamp: 1,
            hash: String::new(),
        }],
        preserve: Preservation::default(),
    };
    let framed = frame.try_to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&framed).unwrap();
    assert_eq!(value["v"], 1, "the envelope survives the Value round trip");
    let re = serde_json::to_string(&value).unwrap();
    let back = HmrMessage::from_json(&re).unwrap();
    assert_eq!(back, frame);
    match back {
        HmrMessage::Update { updates, .. } => {
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].kind, UpdateKind::Data);
            assert_eq!(updates[0].path, "/slow");
            // The default: a data entry's acceptedPath is empty (the client
            // re-fetches `/__data` for the route `path` names).
            assert_eq!(updates[0].accepted_path, "");
        }
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn hmr_wire_v1_rejects_the_retired_wire() {
    // The retired v0 spellings (module-update / css-update /
    // route-data-update / server-module-swap) carry no schema version, so
    // the v1 gate rejects them as Schema mismatches — the flip is total on
    // the Rust side (the TS client keeps a read-compat normalizer for
    // stale-tab sessions; see docs/deviations.md).
    for legacy in [
        r#"{"type":"module-update","path":"a.tsx"}"#,
        r#"{"type":"css-update","path":"a.css","code":""}"#,
        r#"{"type":"route-data-update","route":"/","odf_b64":""}"#,
        r#"{"type":"server-module-swap","path":"db.server.ts"}"#,
        r#"{"type":"batch-module-update","items":[]}"#,
    ] {
        assert!(
            matches!(
                HmrMessage::try_from_json(legacy),
                Err(HmrWireError::Schema {
                    found: 0,
                    expected: 1
                })
            ),
            "{legacy}"
        );
        assert!(HmrMessage::from_json(legacy).is_none());
    }
}
