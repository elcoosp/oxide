//! V2-F19: the per-route capability permission surface, walked
//! (`cargo run -p oxide-fw-jshost --example per_route_permissions --features v8`).
//!
//! A route declares `permissions: { net, env, fs }` in `page.server.ts`;
//! the dispatcher registers the grant; the `PermGuard` seeds the
//! `PERM_CTX` thread-local per call; the `op_oxide_fetch` /
//! `op_oxide_env_get` / `op_oxide_fs_read` ops check the grant at the
//! boundary. A denied host / env name / fs path raises `PRM-001`.
//!
//! This example walks the three seams without spawning a worker — the
//! registry, the guard cycle and the predicate are plain Rust; the real
//! end-to-end enforcement rides the V8 host (CI's `jshost-e2e` job). The
//! hostile fixture (a `setTimeout` created outside the request context)
//! is pinned in `tests/perm_tdd.rs`.

#![cfg(feature = "v8")]

use oxide_fw_jshost::perm::{PermissionDefault, PermissionSet, host_allowed, url_host};
use oxide_fw_jshost::v8_ops::{PermGuard, current_perm_ctx};
use oxide_fw_jshost::{CallInput, set_route_permissions};

/// A `CallInput` for `route` — the shape the worker loop receives.
fn call_input(route: &str) -> CallInput {
    CallInput {
        payload: serde_json::json!({}),
        headers: std::collections::BTreeMap::new(),
        has_body: false,
        request_id: "ex-f19".to_string(),
        route: Some(route.to_string()),
    }
}

/// The route's declared grant — the `export const permissions` value
/// the CLI's declaration scan reads from `page.server.ts`.
fn checkout_route_grant() -> PermissionSet {
    PermissionSet {
        net: Some(vec!["api.stripe.com".into(), "*.internal".into()]),
        env: Some(vec!["STRIPE_KEY".into()]),
        fs: Some(vec!["./data".into()]),
    }
}

fn main() {
    println!("== V2-F19: per-route capability permissions ==");
    println!();

    // (1) The declaration: a route's `export const permissions` in
    // `page.server.ts`, scanned by `declares_permissions`. The typed
    // grant is the `PermissionSet` the dispatcher registers.
    let grant = checkout_route_grant();
    println!("the checkout route's declared grant:");
    println!("  net: {:?}", grant.net);
    println!("  env: {:?}", grant.env);
    println!("  fs: {:?}", grant.fs);
    println!();

    // (2) The dispatcher registers the grant before calling into the
    // isolate — the `PermGuard` reads it per call.
    set_route_permissions("checkout", grant.clone());
    println!(
        "the dispatcher registered the grant for route `checkout` via `set_route_permissions`"
    );
    println!();

    // (3) The PermGuard seeds the PERM_CTX thread-local per call — the
    // ops read it at the boundary.
    let _guard = PermGuard::for_call(&call_input("checkout"));
    let ctx = current_perm_ctx().expect("the guard seeded the grant");
    println!("the PermGuard seeded the PERM_CTX for the checkout call");
    println!();

    // (4) The net boundary: `op_oxide_fetch` extracts the host and calls
    // `is_net_allowed`. A denied host raises PRM-001.
    println!("the fetch op's net boundary checks:");
    for url in [
        "https://api.stripe.com/v1/charges",
        "https://api.internal/users",
        "https://evil.example/exfil",
    ] {
        let host = url_host(url);
        let allowed = host.as_deref().is_some_and(|h| ctx.is_net_allowed(h));
        println!(
            "  fetch({url:?}) -> host={host:?} allowed={allowed}{}",
            if allowed { "" } else { "  // PRM-001" }
        );
    }
    println!();

    // (5) The env boundary: the `process.env` Proxy's backing op calls
    // `is_env_allowed`.
    println!("the env op's boundary checks:");
    for name in ["STRIPE_KEY", "DATABASE_URL"] {
        let allowed = ctx.is_env_allowed(name);
        println!(
            "  process.env.{name} -> allowed={allowed}{}",
            if allowed { "" } else { "  // PRM-001" }
        );
    }
    println!();

    // (6) The fs boundary: the `node:fs.readFile` shim's backing op
    // calls `is_fs_allowed`.
    println!("the fs op's boundary checks:");
    for path in ["./data/users.json", "./secrets/key.pem"] {
        let allowed = ctx.is_fs_allowed(path);
        println!(
            "  readFile({path:?}) -> allowed={allowed}{}",
            if allowed { "" } else { "  // PRM-001" }
        );
    }
    println!();

    // (7) The wildcard rule: `*.internal` matches `api.internal` but
    // rejects `notinternal` (no label boundary).
    println!("the `*.` wildcard rule (label-boundary, not bare suffix):");
    let pats = &["*.internal".to_string()];
    for host in [
        "api.internal",
        "svc.internal",
        "notinternal",
        "api.internal.evil.com",
    ] {
        println!(
            "  host_allowed({pats:?}, {host:?}) = {}",
            host_allowed(pats, host)
        );
    }
    println!();

    // (8) The default posture: v0.2 ships allow-all (PermissionDefault::Allow).
    println!(
        "the v0.2 default posture: {:?} (allow-all; deny-by-default is the opt-in)",
        PermissionDefault::Allow
    );

    // The guard drops here — the PERM_CTX is cleared, so a later op on
    // this worker thread sees no grant (unrestricted), NOT the checkout
    // route's grant. Model 2 (the profile-keyed isolate pool) bakes the
    // grant in so a detached timer cannot bypass it — see the hostile
    // fixture in `tests/perm_tdd.rs` and the decision record.
    drop(_guard);
    assert!(current_perm_ctx().is_none(), "cleared on drop");
    println!();
    println!("the PermGuard dropped — PERM_CTX cleared (no cross-call leakage)");
}
