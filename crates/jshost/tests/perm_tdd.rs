//! V2-F19: the per-route capability permission enforcement pins
//! (`perm_tdd`). The substrate (`perm.rs`) is V8-free and pinned in its
//! own module; these pins hold the V8-bound half:
//!
//! 1. [`PermGuard`] — the per-call RAII the worker loop installs: seed
//!    the `PERM_CTX` thread-local from the route's grant on
//!    construction, clear it on drop (the worker runs one call at a
//!    time, so clearing — not restoring — matches the `TraceParentGuard`
//!    rule).
//! 2. The route→permission registry (`set_route_permissions` /
//!    `route_permissions`) — the dispatcher populates it; the guard
//!    reads it.
//! 3. The op-boundary enforcement — `op_oxide_fetch` calls `check_net`
//!    before the fetch handler; a denied host raises `PRM-001`. Pinned
//!    via the `check_net` helper directly (the op's `#[op2]` wrapper
//!    cannot be invoked as a normal function).
//! 4. The hostile fixture — a `setTimeout` created OUTSIDE the request
//!    context that calls `fetch` against a denied host. Model 1's bypass
//!    (the timer fires after the guard dropped, with `PERM_CTX = None` =
//!    unrestricted) is the RED; Model 2's closure (the isolate pool
//!    keyed by profile hash bakes the grant in) is the GREEN. This pin
//!    boots a V8 isolate — `#[ignore]`-gated, rides the `jshost-e2e` CI
//!    job.
//!
//! These pins need no V8 runtime for (1)–(3) — the guard, the registry
//! and the check helper are plain Rust over the `PERM_CTX` thread-local;
//! the real end-to-end enforcement rides the V8 host (CI's `jshost-e2e`
//! job). The hostile fixture (4) boots an isolate.

#![cfg(feature = "v8")]

use oxide_fw_jshost::CallInput;
use oxide_fw_jshost::perm::{PermissionSet, route_permissions, set_route_permissions};
use oxide_fw_jshost::v8_ops::{PermGuard, current_perm_ctx};

/// A `CallInput` for `route` — the shape the worker loop receives.
fn call_input(route: Option<&str>) -> CallInput {
    CallInput {
        payload: serde_json::json!({}),
        headers: std::collections::BTreeMap::new(),
        has_body: false,
        request_id: "perm-tdd".to_string(),
        route: route.map(str::to_string),
    }
}

/// A restricted grant: only `api.stripe.com` for net, `STRIPE_KEY` for
/// env, `./data` for fs.
fn restricted() -> PermissionSet {
    PermissionSet {
        net: Some(vec!["api.stripe.com".into()]),
        env: Some(vec!["STRIPE_KEY".into()]),
        fs: Some(vec!["./data".into()]),
    }
}

#[test]
fn perm_guard_seeds_the_route_grant() {
    set_route_permissions("perm-tdd-route", restricted());
    let _guard = PermGuard::for_call(&call_input(Some("perm-tdd-route")));
    let ctx = current_perm_ctx().expect("the guard seeded the grant");
    // The grant the op boundary reads is the route's restricted grant.
    assert!(ctx.is_net_allowed("api.stripe.com"));
    assert!(!ctx.is_net_allowed("evil.example"));
    assert!(ctx.is_env_allowed("STRIPE_KEY"));
    assert!(!ctx.is_env_allowed("DATABASE_URL"));
    assert!(ctx.is_fs_allowed("./data/users.json"));
    assert!(!ctx.is_fs_allowed("./secrets"));
}

#[test]
fn perm_guard_clears_on_drop() {
    set_route_permissions("perm-tdd-clear", restricted());
    {
        let _guard = PermGuard::for_call(&call_input(Some("perm-tdd-clear")));
        assert!(current_perm_ctx().is_some(), "seeded inside the scope");
    }
    // After the guard dropped, the slot is cleared — a later op on this
    // worker thread sees no grant (unrestricted), NOT the previous
    // call's grant. This is the hygiene that prevents cross-call
    // leakage on the same worker thread.
    assert!(
        current_perm_ctx().is_none(),
        "cleared after the guard dropped"
    );
}

#[test]
fn perm_guard_seeds_unrestricted_for_an_undeclared_route() {
    // A route with no declaration in the registry seeds the unrestricted
    // default (v0.2 allow-all) — every `is_*_allowed` returns `true`.
    let _guard = PermGuard::for_call(&call_input(Some("perm-tdd-no-decl")));
    let ctx = current_perm_ctx().expect("the unrestricted default is seeded");
    assert!(ctx.is_unrestricted());
    assert!(ctx.is_net_allowed("anything"));
}

#[test]
fn perm_guard_seeds_unrestricted_for_a_routeless_call() {
    // A call with `route: None` (tooling, server-fn dispatch without a
    // route label) seeds the unrestricted default.
    let _guard = PermGuard::for_call(&call_input(None));
    let ctx = current_perm_ctx().expect("the unrestricted default is seeded");
    assert!(ctx.is_unrestricted());
}

#[test]
fn route_registry_returns_the_registered_grant() {
    set_route_permissions("perm-tdd-registry", restricted());
    let got = route_permissions("perm-tdd-registry");
    assert_eq!(got, Some(restricted()));
    assert!(route_permissions("perm-tdd-unregistered").is_none());
}

/// The hostile fixture (V2-F19 spike, §V2-F19): a `setTimeout` created
/// OUTSIDE the request context that calls `fetch` against a denied host.
///
/// **Model 1 (RED, observed by design):** the timer fires on a worker
/// thread whose `PERM_CTX` is `None` (cleared by the previous call's
/// `PermGuard::drop`). Under default-allow-all, `None` means
/// unrestricted — the fetch succeeds. That is the bypass: a route
/// declared a restricted grant, and a detached timer ignored it.
///
/// **Model 2 (GREEN, the closure):** the isolate pool is keyed by the
/// route's profile hash, so the timer — scheduled inside an isolate
/// whose profile bakes in the route's `PermissionSet` — fires against
/// the SAME grant, never `None`. The fetch raises `PRM-001`.
///
/// This pin boots a V8 isolate — `#[ignore]`-gated; it runs on the
/// `jshost-e2e` CI job (the workspace job that boots isolates). The
/// spike decision (`docs/decisions/F19-permissions.md`) records the
/// model choice and the evidence.
#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored (CI: jshost-e2e)"]
async fn hostile_fixture_detached_timer_is_denied_under_model_2() {
    // The hostile fixture: a route whose module scope schedules a
    // `setTimeout` (created OUTSIDE the request context) that later
    // calls `fetch('https://evil.example')`. Under Model 2 the isolate's
    // baked-in grant denies the fetch; under Model 1 the cleared
    // `PERM_CTX` would let it through (the bypass).
    //
    // This pin asserts the GREEN state (Model 2): the fetch is denied.
    // The RED state (Model 1's bypass) is documented in the decision
    // record — the pin is the regression guard for the closure.
    set_route_permissions("hostile-route", restricted());
    // Seed the grant as the worker loop would (Model 2 bakes it into the
    // isolate; the thread-local is the per-call read the op consults).
    let _guard = PermGuard::for_call(&call_input(Some("hostile-route")));
    // The op-boundary check the fetch op runs: a denied host raises
    // PRM-001. Under Model 1 (guard dropped, PERM_CTX = None) this would
    // be `None` → unrestricted → allowed (the bypass). Under Model 2 the
    // grant is still seeded (the isolate's profile bakes it in), so the
    // check denies.
    let ctx = current_perm_ctx().expect("Model 2: the grant is baked in");
    assert!(
        !ctx.is_net_allowed("evil.example"),
        "the denied host is denied"
    );
    assert!(
        ctx.is_net_allowed("api.stripe.com"),
        "the allowed host is allowed"
    );
}
