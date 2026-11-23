//! TDD pins for the M2 architectural seam `RegistryReport` — the
//! introspection API the dev overlay's observability reads (the
//! documented next step after the M1 consult seam landed; the
//! candidate deferred from the M1 turn and now shipped because M1 is
//! green end-to-end).
//!
//! [`SwappableHost`] already exposes `generation` / `ids` / `is_empty`
//! as individual accessors; the dev overlay (the `examples/server_lib_loop.rs`
//! shape) and any future observability arm want ONE snapshot of the
//! registry's posture to render, not N reads each taking the swap lock.
//! [`RegistryReport`] is that snapshot — a single-lock read that carries
//! the generation, the mount count, the id count, the sorted ids,
//! whether a types payload is mounted, the per-function timeout, and
//! the blocking-lane count.
//!
//! The same shape answers for the boot-time [`crate::host::RustFnHost`]
//! (`RustFnHost::report`), so the dev overlay introspects EITHER host
//! through the same type — the same "one consult surface" contract M1
//! landed for `RustFnRegistry::call`.
//!
//! Written BEFORE the implementation; every pin imports public API
//! only. The contracts pinned here:
//!
//! - `SwappableHost::report()` is a single-lock read; the fields are
//!   consistent with `generation()` / `ids()` / `is_empty()` /
//!   `RustFnRegistry::types_json` at the instant of the read.
//! - The `ids` are sorted (the host's `BTreeMap` order), so two reports
//!   over the same state are byte-equal (the dev overlay's diff).
//! - A swap changes the report: `generation` bumps, `mount_count`
//!   rises by one (the new mount), the `ids` reflect the new bindings.
//! - `retire_idle` drops `mount_count` when an orphan is reclaimed.
//! - `has_types` is `true` when ANY mounted library carries a types
//!   payload (the same consult `RustFnRegistry::types_json` answers).
//! - `timeout_secs` and `lanes` carry the host's budget and lane count
//!   (the substrate's config surface, exposed for the overlay's
//!   "is the budget set?" check).
//! - `RustFnHost::report()` answers the same shape — the boot-time
//!   host and the dev-loop host share ONE introspection type.
//! - The report is `Default`: the all-zero "no host attached" sentinel
//!   (`generation = 0`, `mount_count = 0`, `id_count = 0`, `ids = []`,
//!   `has_types = false`, `timeout_secs = 0`, `lanes = 0`), DISTINCT from
//!   an empty host's report (which carries the substrate's 30 s budget
//!   and 16 lanes — see `the_empty_host_report_carries_the_substrate_defaults`).

use std::sync::Arc;
use std::time::Duration;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, RegistryReport, SwappableHost};

/// Build a `&'static FnTable` over `entries` (the test-fixture pattern
/// the host pins use).
fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

fn entry_named(
    name: &'static str,
    call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32,
) -> FnEntry {
    FnEntry {
        name: leak_name(name),
        call,
    }
}

extern "C" fn echo_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        encode_odf(&format!("v1:{text}")).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

fn v1_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v1)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn v1_lib_with_types(types: &'static str) -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v1)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), Some(types)).expect("a current-ABI table loads")
}

fn echo_binding(id: &str) -> FnBinding {
    FnBinding::new(id, "echo")
}

/// The empty host's report carries the substrate's defaults: nothing
/// mounted, nothing registered, generation 0, the 30 s budget and 16
/// lanes (distinct from `RegistryReport::default()`, the all-zero "no
/// host attached" sentinel).
#[test]
fn the_empty_host_report_carries_the_substrate_defaults() {
    let host = SwappableHost::new();
    let report = host.report();
    assert_eq!(
        report,
        RegistryReport {
            generation: 0,
            mount_count: 0,
            id_count: 0,
            ids: Vec::new(),
            has_types: false,
            timeout_secs: 30,
            lanes: 16,
        }
    );
}

/// `report()` is consistent with the individual accessors at the
/// instant of the read — `generation()`, `ids()`, `is_empty()`, and
/// the `RustFnRegistry::types_json` consult all agree with the
/// snapshot. The dev overlay reads ONE report, not N locks.
#[test]
fn the_report_matches_the_individual_accessors() {
    let host = Arc::new(SwappableHost::new());
    host.swap(
        v1_lib_with_types("RFN-TYPES {}"),
        &[echo_binding("id-b"), echo_binding("id-a")],
    )
    .expect("two ids mount");
    let report = host.report();
    assert_eq!(report.generation, host.generation());
    assert_eq!(report.ids, host.ids());
    assert_eq!(report.id_count, host.ids().len());
    assert_eq!(report.is_empty(), host.is_empty());
    assert_eq!(report.has_types, host.types_json().is_some());
    // The ids are sorted (the host's BTreeMap order), so the report is
    // byte-stable across renders.
    assert_eq!(report.ids, vec!["id-a".to_string(), "id-b".to_string()]);
    assert_eq!(report.mount_count, 1);
}

/// A swap changes the report: `generation` bumps, `mount_count`
/// rises by one (the new mount the loop loads), and the `ids` reflect
/// the new bindings (the dropped id leaves, the added id joins).
#[test]
fn a_swap_changes_the_report() {
    let host = SwappableHost::new();
    host.swap(
        v1_lib(),
        &[echo_binding("id-keep"), echo_binding("id-drop")],
    )
    .expect("the first swap mounts");
    let before = host.report();
    assert_eq!(before.generation, 1);
    assert_eq!(before.mount_count, 1);
    assert_eq!(before.id_count, 2);

    host.swap(v1_lib(), &[echo_binding("id-keep"), echo_binding("id-add")])
        .expect("the second swap re-points and adds");
    let after = host.report();
    assert_eq!(after.generation, 2);
    assert_eq!(
        after.mount_count, 2,
        "the old mount stays until retire_idle"
    );
    assert_eq!(after.id_count, 2);
    assert_eq!(after.ids, vec!["id-add".to_string(), "id-keep".to_string()]);
}

/// `retire_idle` drops `mount_count` when an orphan is reclaimed —
/// the report reflects the post-retire mount list, not the pre-retire
/// one (the dev overlay's "is the old mount gone yet?" check).
#[test]
fn retire_idle_drops_the_mount_count_in_the_report() {
    let host = SwappableHost::new();
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the first swap mounts");
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the second swap orphans the first mount");
    let before = host.report();
    assert_eq!(before.mount_count, 2);
    let retire = host.retire_idle();
    assert_eq!(retire.retired, 1);
    let after = host.report();
    assert_eq!(after.mount_count, 1);
    assert_eq!(after.id_count, 1);
}

/// `has_types` is `true` when ANY mounted library carries a types
/// payload — the same consult `RustFnRegistry::types_json` answers
/// (the first library that carries one). A swap that replaces a
/// typed library with an untyped one flips `has_types` back to
/// `false` after the retire.
#[test]
fn has_types_tracks_the_types_payload_consult() {
    let host = SwappableHost::new();
    host.swap(
        v1_lib_with_types("RFN-TYPES {}"),
        &[echo_binding("id-echo")],
    )
    .expect("the typed lib mounts");
    assert!(host.report().has_types);
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the untyped lib mounts, orphaning the typed one");
    // Before retire: the typed mount still owns the types consult.
    assert!(host.report().has_types);
    let _ = host.retire_idle();
    // After retire: the untyped mount owns the consult — no payload.
    assert!(!host.report().has_types);
}

/// `timeout_secs` and `lanes` carry the host's budget and lane count —
/// the substrate's config surface, exposed for the overlay's "is the
/// budget set?" check. The host with `with_timeout` and `with_lanes`
/// reports the configured values.
#[test]
fn the_report_carries_the_budget_and_lanes() {
    let host =
        SwappableHost::with_host(RustFnHost::with_lanes(4).with_timeout(Duration::from_secs(5)));
    let report = host.report();
    assert_eq!(report.timeout_secs, 5);
    assert_eq!(report.lanes, 4);
}

/// `RustFnHost::report()` answers the same shape — the boot-time host
/// and the dev-loop host share ONE introspection type (the same "one
/// consult surface" contract M1 landed for `RustFnRegistry::call`).
/// The dev overlay introspects EITHER host through `RegistryReport`.
#[test]
fn the_boot_time_host_reports_the_same_shape() {
    let mut host = RustFnHost::new();
    let report = host.report();
    assert_eq!(report.generation, 0);
    assert_eq!(report.mount_count, 0);
    assert_eq!(report.id_count, 0);
    assert_eq!(report.ids, Vec::<String>::new());
    assert!(!report.has_types);
    assert_eq!(report.timeout_secs, 30);
    assert_eq!(report.lanes, 16);

    let index = host.mount(v1_lib_with_types("RFN-TYPES {}"));
    host.register("id-echo", index, "echo");
    let report = host.report();
    assert_eq!(report.mount_count, 1);
    assert_eq!(report.id_count, 1);
    assert_eq!(report.ids, vec!["id-echo".to_string()]);
    assert!(report.has_types);
}

/// The report is `Clone` + `PartialEq` + `Default` — the dev overlay
/// diffs two snapshots and constructs an empty one for the "no host
/// attached yet" state (the all-zero `Default`, distinct from an empty
/// host's report which carries the substrate's 30 s / 16-lane defaults).
#[test]
fn the_report_is_clone_eq_and_default() {
    fn assert_clone_eq_default<T: Clone + PartialEq + Default>() {}
    assert_clone_eq_default::<RegistryReport>();
    let host = SwappableHost::new();
    let a = host.report();
    let b = a.clone();
    assert_eq!(a, b);
    // The all-zero Default is the "no host attached" sentinel, NOT the
    // empty host's report (the empty host carries the substrate's
    // 30 s budget and 16 lanes — a real host, just unmounted).
    let no_host = RegistryReport::default();
    assert_eq!(no_host.timeout_secs, 0);
    assert_eq!(no_host.lanes, 0);
    assert_ne!(no_host, a);
}
