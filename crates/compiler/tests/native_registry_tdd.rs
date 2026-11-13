//! TDD suite for the V2-C01 native-compiler seam: the [`NativeCompiler`]
//! trait (string in, string out — no AST type may cross the boundary),
//! the [`NativeRegistry`] selection engine (`auto` picks a healthy native
//! compiler and falls back to the Node pool, `native` refuses without one,
//! `node` never picks native), the once-only health check, the D40 CAS
//! tool-version spelling and the `CMP-NAT-001` refusal.
//!
//! No native backend is compiled in yet — C02–C04 land the real backends
//! behind their vendored upstreams — so every pin here drives the registry
//! through a `FakeNative` (the spec's own test plan: "Use a `FakeNative`
//! implementing the trait under `#[cfg(test)]`"), which is exactly the
//! shape a host crate injects a compiled-in backend with.
//!
//! Written BEFORE the implementation; every pin imports public API only.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxide_fw_compiler::Result as CompileResult;
use oxide_fw_compiler::err;
use oxide_fw_compiler::native::{
    Backend, CompileOutput, CompileRequest, Generate, NativeCompiler, NativeRegistry, NativeReport,
    cas_tool_version, native_err,
};

/// A configurable fake: the fixture compiles or fails, every `compile()`
/// bumps a caller-shared counter (the health probe and the caller-driven
/// compiles are distinguishable by count), and the emitted JS carries the
/// request so the plumbing pins can assert on it.
struct FakeNative {
    id: &'static str,
    exts: &'static [&'static str],
    healthy: bool,
    compile_calls: Arc<AtomicUsize>,
}

impl FakeNative {
    fn healthy(id: &'static str) -> Self {
        Self::with_counter(id, true, Arc::new(AtomicUsize::new(0)))
    }
    fn broken(id: &'static str) -> Self {
        Self::with_counter(id, false, Arc::new(AtomicUsize::new(0)))
    }
    fn with_counter(id: &'static str, healthy: bool, counter: Arc<AtomicUsize>) -> Self {
        Self {
            id,
            exts: &["tsx"],
            healthy,
            compile_calls: counter,
        }
    }
}

impl NativeCompiler for FakeNative {
    fn framework(&self) -> &'static str {
        "solid"
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn version(&self) -> String {
        "0.1.0@testrev".to_string()
    }
    fn handles(&self, ext: &str) -> bool {
        self.exts.contains(&ext)
    }
    fn compile(&self, req: &CompileRequest<'_>) -> CompileResult<CompileOutput> {
        self.compile_calls.fetch_add(1, Ordering::SeqCst);
        if !self.healthy {
            return Err(err("CMP-NAT-001", "the fake fixture always fails"));
        }
        let target = match req.generate {
            Generate::Dom => "dom",
            Generate::Ssr => "ssr",
        };
        Ok(CompileOutput {
            js: format!("// {target} js({}) for {}", req.source, req.path.display()),
            css: None,
            warnings: vec!["fake warning".to_string()],
        })
    }
    fn health_fixture(&self) -> (&'static str, &'static str) {
        (
            "health-fixture.tsx",
            if self.healthy {
                "export default () => null;"
            } else {
                "this is not a valid fixture"
            },
        )
    }
}

fn healthy_registry(id: &'static str) -> NativeRegistry {
    let mut reg = NativeRegistry::from_enabled_features();
    reg.register(Box::new(FakeNative::healthy(id)));
    reg
}

// -- the spec's four named pins ------------------------------------------------

#[test]
fn registry_pick_node_always_none() {
    // `node` is the escape hatch: whatever is compiled in and healthy,
    // `node` never picks the native path (the D26 one-release fallback).
    let empty = NativeRegistry::from_enabled_features();
    let full = healthy_registry("full");
    for reg in [&empty, &full] {
        for ext in ["tsx", "vue", "svelte", ""] {
            let picked = reg
                .pick("solid", ext, Backend::Node)
                .expect("`node` never errors");
            assert!(
                picked.is_none(),
                "backend `node` must always answer None (id {}, ext {ext})",
                reg.report().first().map_or("<empty>", |r| r.id.as_str())
            );
        }
    }
}

#[test]
fn registry_pick_native_missing_is_error() {
    // No native compiler compiled in (the C01 tree's own state, until the
    // C02-C04 backends land): `native` is a hard request — refusing with
    // the registry code beats a silent Node fallback the config author
    // did not ask for.
    let reg = NativeRegistry::from_enabled_features();
    let e = reg
        .pick("solid", "tsx", Backend::Native)
        .err()
        .expect("`native` with no compiled-in backend refuses");
    assert_eq!(e.0.code, "CMP-NAT-001");
    assert!(
        e.0.message.contains("solid"),
        "the refusal names the framework"
    );
    let fix = e.0.fix.clone().unwrap_or_default();
    assert!(
        fix.contains("native-solid") && fix.contains("auto"),
        "the fix names the feature flag and the fallback spellings (got: {fix})"
    );
    // The free fn behind the registry error is public so the CLI adoption
    // surfaces the identical refusal text.
    let e2 = native_err("vue");
    assert_eq!(e2.0.code, "CMP-NAT-001");
    assert!(e2.0.message.contains("vue"));
}

#[test]
fn registry_auto_falls_back_when_unhealthy() {
    // `auto` is the honest default: a broken backend (its health fixture
    // fails to compile) is invisible to `auto` — it answers None and the
    // caller uses the Node pool — while `native` refuses loudly.
    let mut reg = NativeRegistry::from_enabled_features();
    reg.register(Box::new(FakeNative::broken("broken-solid")));
    let auto = reg
        .pick("solid", "tsx", Backend::Auto)
        .expect("`auto` never errors on an unhealthy backend");
    assert!(auto.is_none(), "auto falls back when the fixture fails");
    let native = reg
        .pick("solid", "tsx", Backend::Native)
        .err()
        .expect("`native` refuses when the only backend is unhealthy");
    assert_eq!(native.0.code, "CMP-NAT-001");
}

#[test]
fn health_checked_once() {
    // The health-fixture compile is the expensive probe; `pick` runs it
    // once per backend and caches the verdict for the process lifetime
    // (the spec's OnceLock shape), so a hot build loop never re-pays it.
    // The pin: N picks over a healthy backend bump the shared compile
    // counter exactly ONCE (the health fixture), while caller-driven
    // compiles through the picked backend bump it per call.
    let counter = Arc::new(AtomicUsize::new(0));
    let mut reg = NativeRegistry::from_enabled_features();
    reg.register(Box::new(FakeNative::with_counter(
        "counted",
        true,
        Arc::clone(&counter),
    )));
    for _ in 0..8 {
        let picked = reg
            .pick("solid", "tsx", Backend::Auto)
            .expect("a healthy backend answers Some")
            .expect("the fake is registered");
        assert_eq!(picked.id(), "counted");
    }
    // Eight picks, one health probe — the verdict is cached; a probe
    // per pick would read 8.
    assert_eq!(counter.load(Ordering::SeqCst), 1, "health ran once");
    // Caller-driven compiles through the picked backend add per call:
    let picked = reg.pick("solid", "tsx", Backend::Auto).unwrap().unwrap();
    for _ in 0..2 {
        let out = picked.compile(&CompileRequest {
            path: Path::new("a.tsx"),
            source: "x",
            generate: Generate::Dom,
            dev: false,
        });
        assert!(out.is_ok());
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "1 cached probe + 2 caller compiles"
    );
    // Pick-only loops never re-probe (the second registry's own pin):
    let counter2 = Arc::new(AtomicUsize::new(0));
    let mut reg2 = NativeRegistry::from_enabled_features();
    reg2.register(Box::new(FakeNative::with_counter(
        "counted2",
        true,
        Arc::clone(&counter2),
    )));
    for _ in 0..5 {
        assert!(reg2.pick("solid", "tsx", Backend::Auto).unwrap().is_some());
    }
    assert_eq!(
        counter2.load(Ordering::SeqCst),
        1,
        "pick-only loops never re-probe"
    );
}

// -- the rest of the seam ------------------------------------------------------

#[test]
fn auto_picks_the_healthy_backend_and_the_request_plumbs_through() {
    let reg = healthy_registry("solid-native");
    let picked = reg
        .pick("solid", "tsx", Backend::Auto)
        .expect("healthy registry answers Some")
        .expect("the fake is registered");
    assert_eq!(picked.id(), "solid-native");
    assert_eq!(picked.framework(), "solid");
    let out = picked
        .compile(&CompileRequest {
            path: Path::new("src/app/Counter.tsx"),
            source: "counter()",
            generate: Generate::Ssr,
            dev: true,
        })
        .unwrap();
    assert!(
        out.js.contains("counter()"),
        "the source reached the backend"
    );
    assert!(
        out.js.contains("Counter.tsx"),
        "the path reached the backend"
    );
    assert_eq!(out.warnings, vec!["fake warning".to_string()]);
    assert!(out.css.is_none());
}

#[test]
fn pick_matches_framework_and_extension_not_just_the_first_entry() {
    // Two solid backends, one that handles `.tsx` only and one that
    // handles `.jsx` only: `pick` must dispatch by (framework, ext), the
    // contract the precompile integration keys on.
    struct JsxOnly;
    impl NativeCompiler for JsxOnly {
        fn framework(&self) -> &'static str {
            "solid"
        }
        fn id(&self) -> &'static str {
            "solid-jsx"
        }
        fn version(&self) -> String {
            "1".to_string()
        }
        fn handles(&self, ext: &str) -> bool {
            ext == "jsx"
        }
        fn compile(&self, _req: &CompileRequest<'_>) -> CompileResult<CompileOutput> {
            // Non-empty output: the health probe requires it (an empty
            // js marks the backend unhealthy — the contract
            // `registry_auto_falls_back_when_unhealthy` pins).
            Ok(CompileOutput {
                js: "// jsx".to_string(),
                css: None,
                warnings: Vec::new(),
            })
        }
        fn health_fixture(&self) -> (&'static str, &'static str) {
            ("fixture.jsx", "export default () => null;")
        }
    }
    let mut reg = healthy_registry("solid-tsx");
    reg.register(Box::new(JsxOnly));
    let tsx = reg.pick("solid", "tsx", Backend::Auto).unwrap().unwrap();
    assert_eq!(tsx.id(), "solid-tsx");
    let jsx = reg.pick("solid", "jsx", Backend::Auto).unwrap().unwrap();
    assert_eq!(jsx.id(), "solid-jsx");
    // A framework with no registered backend falls back under `auto`.
    assert!(reg.pick("vue", "vue", Backend::Auto).unwrap().is_none());
    // An extension the backend does not handle falls back too.
    assert!(
        reg.pick("solid", "svelte", Backend::Auto)
            .unwrap()
            .is_none()
    );
}

#[test]
fn report_is_the_doctor_surface() {
    let mut reg = NativeRegistry::from_enabled_features();
    reg.register(Box::new(FakeNative::healthy("solid-native")));
    reg.register(Box::new(FakeNative::broken("solid-broken")));
    let rows: Vec<NativeReport> = reg.report();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "solid-native");
    assert_eq!(rows[0].version, "0.1.0@testrev");
    assert!(rows[0].healthy, "the healthy fixture reports ok");
    assert!(!rows[1].healthy, "the broken fixture reports unhealthy");
    // An empty registry (the C01 tree until C02-C04 land) reports nothing
    // rather than inventing rows.
    assert_eq!(
        NativeRegistry::from_enabled_features().report(),
        Vec::<NativeReport>::new()
    );
}

#[test]
fn cas_tool_version_is_the_d40_spelling() {
    // D40: switching `compilerBackend` never reuses a stale artifact — the
    // CAS key's tool_version becomes `native:{id}@{version}` for native
    // backends (the CLI adoption feeds this into PrecompileKey).
    let fake = FakeNative::healthy("solid-native");
    assert_eq!(cas_tool_version(&fake), "native:solid-native@0.1.0@testrev");
    // Dyn shape: the registry hands out `&dyn NativeCompiler`.
    let reg = healthy_registry("solid-native");
    let picked: &dyn NativeCompiler = reg.pick("solid", "tsx", Backend::Auto).unwrap().unwrap();
    assert_eq!(
        cas_tool_version(picked),
        "native:solid-native@0.1.0@testrev"
    );
}

#[test]
fn backend_default_is_auto() {
    assert_eq!(Backend::default(), Backend::Auto);
}

#[test]
fn generate_maps_the_two_compile_targets() {
    // Dom and Ssr are the only targets the worker protocol carries
    // (`generate == "ssr"` at the integration site); the request carries
    // the target verbatim and the backend maps it.
    let reg = healthy_registry("gen");
    let picked = reg.pick("solid", "tsx", Backend::Auto).unwrap().unwrap();
    let dom = picked
        .compile(&CompileRequest {
            path: Path::new("a.tsx"),
            source: "c()",
            generate: Generate::Dom,
            dev: false,
        })
        .unwrap();
    assert!(dom.js.contains("// dom js(c())"), "Dom reached the backend");
    let ssr = picked
        .compile(&CompileRequest {
            path: Path::new("a.tsx"),
            source: "c()",
            generate: Generate::Ssr,
            dev: false,
        })
        .unwrap();
    assert!(ssr.js.contains("// ssr js(c())"), "Ssr reached the backend");
}
