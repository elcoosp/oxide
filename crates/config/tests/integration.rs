//! Integration suite for oxide-config (spec §14, D14) — drives the crate only
//! through its public API: env-file parsing, size parsing, static extraction
//! (including the exact §14.1 example), layering, validation and public env.

use oxide_config::{OxideConfig, extract_static_config, load_with_env, parse_env_file, parse_size};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

const EMPTY_ENV: BTreeMap<String, String> = BTreeMap::new();

/// Create a temp project dir with the given files.
fn fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = tmp.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let dir = tmp.path().to_path_buf();
    (tmp, dir)
}

fn json_file(v: serde_json::Value) -> String {
    serde_json::to_string_pretty(&v).unwrap()
}

fn cli(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn envmap(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The exact §14.1 example from the spec (byte-for-byte).
const SPEC_EXAMPLE: &str = r#"// oxide.config.ts
import { defineConfig } from '@oxide-fw/core'
import react from '@oxide-fw/react'       // or @oxide-fw/vue | @oxide-fw/svelte | @oxide-fw/solid
import tailwind from '@oxide-fw/tailwind'

export default defineConfig({
  framework: react({ compiler: true }),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },
  cache: { handler: { type: 'disk' }, l0: { maxSize: '512mb' } },
  images: { formats: ['avif', 'webp'], remotePatterns: [{ host: 'cdn.example.com' }] },
  fonts: { providers: ['google'], subset: ['latin'] },
  i18n: { locales: ['en', 'fr'], default: 'en', strategy: 'prefix-except-default' },
  security: { csp: 'strict', csrf: 'origin' },
  rules: [{ source: '/old/:slug', redirect: '/new/:slug', status: 308 }],
  budgets: { clientJs: '150kb', routeJs: '60kb' },
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',                       // binary | node | static | docker | cloudflare | ...
  plugins: [tailwind()],
  modules: [],
})"#;

// ---------------------------------------------------------------------------
// parse_env_file
// ---------------------------------------------------------------------------

#[test]
fn env_file_parsing_table() {
    let content = "\
# full-line comment
PLAIN=hello

export EXPORTED=yes
QUOTED=\"line1\\nline2\\ttab\\\\end\"
SINGLE='stay \"as\" is'
SPACED = padded value
WITH_EQ=a=b=c
INLINE=tail   # trailing comment
EMPTY=
NUM=0042
DUPLICATED=1
DUPLICATED=2
";
    let m = parse_env_file(content);
    assert_eq!(m.get("PLAIN").map(String::as_str), Some("hello"));
    assert_eq!(
        m.get("EXPORTED").map(String::as_str),
        Some("yes"),
        "export prefix stripped"
    );
    assert_eq!(
        m.get("QUOTED").map(String::as_str),
        Some("line1\nline2\ttab\\end"),
        "double-quote escapes processed"
    );
    assert_eq!(
        m.get("SINGLE").map(String::as_str),
        Some("stay \"as\" is"),
        "single quotes literal"
    );
    assert_eq!(
        m.get("SPACED").map(String::as_str),
        Some("padded value"),
        "spaces around = trimmed"
    );
    assert_eq!(
        m.get("WITH_EQ").map(String::as_str),
        Some("a=b=c"),
        "only first = splits"
    );
    assert_eq!(
        m.get("INLINE").map(String::as_str),
        Some("tail"),
        "inline comment stripped"
    );
    assert_eq!(m.get("EMPTY").map(String::as_str), Some(""));
    assert_eq!(
        m.get("NUM").map(String::as_str),
        Some("0042"),
        "values stay strings here"
    );
    assert_eq!(
        m.get("DUPLICATED").map(String::as_str),
        Some("2"),
        "last wins"
    );
    assert_eq!(m.len(), 10);
}

#[test]
fn env_file_skips_non_assignments_and_bad_keys() {
    let m = parse_env_file(
        "NO_EQUALS_SIGN\n=novalue\nGOOD=1\n# commented=out\n   \nBAD KEY=2\nALSO_GOOD=3",
    );
    assert_eq!(m.len(), 2);
    assert_eq!(m.get("GOOD").map(String::as_str), Some("1"));
    assert_eq!(m.get("ALSO_GOOD").map(String::as_str), Some("3"));
}

// ---------------------------------------------------------------------------
// parse_size
// ---------------------------------------------------------------------------

#[test]
fn size_parsing_table() {
    assert_eq!(parse_size("512mb").unwrap(), 536_870_912);
    assert_eq!(parse_size("64kb").unwrap(), 65_536);
    assert_eq!(parse_size("1gb").unwrap(), 1_073_741_824);
    assert_eq!(parse_size("1024").unwrap(), 1024);
    assert_eq!(parse_size("1.5mb").unwrap(), 1_572_864, "fractional sizes");
    assert_eq!(
        parse_size("512MB").unwrap(),
        536_870_912,
        "case-insensitive"
    );
    assert_eq!(parse_size(" 8 KB ").unwrap(), 8_192, "trimmed");
    assert_eq!(parse_size("0").unwrap(), 0);
    assert_eq!(parse_size("2tb").unwrap(), 2_199_023_255_552);
}

#[test]
fn size_parsing_errors_carry_cfg_size() {
    for bad in ["abc", "-5", "12zb", "", "1.2.3mb", "mb"] {
        let e = parse_size(bad).unwrap_err();
        assert_eq!(e.0.code, "CFG-SIZE", "size `{bad}`");
    }
}

// ---------------------------------------------------------------------------
// extract_static_config
// ---------------------------------------------------------------------------

#[test]
fn extracts_the_spec_141_example() {
    let v = extract_static_config(SPEC_EXAMPLE).expect("§14.1 example must static-extract");
    assert_eq!(v["framework"]["name"], "react");
    assert_eq!(v["framework"]["adapter"], "react");
    assert_eq!(v["framework"]["compiler"], true);
    assert_eq!(v["routes"]["dir"], "src/routes");
    assert_eq!(v["server"]["port"], 3000);
    assert_eq!(
        v["server"]["compression"],
        serde_json::json!(["zstd", "br"])
    );
    assert_eq!(v["server"]["http3"], false);
    assert_eq!(v["render"]["default"], "auto");
    assert_eq!(v["cache"]["handler"]["type"], "disk");
    assert_eq!(v["images"]["formats"], serde_json::json!(["avif", "webp"]));
    assert_eq!(v["images"]["remotePatterns"][0]["host"], "cdn.example.com");
    assert_eq!(v["fonts"]["providers"], serde_json::json!(["google"]));
    assert_eq!(v["i18n"]["locales"], serde_json::json!(["en", "fr"]));
    assert_eq!(v["i18n"]["default"], "en");
    assert_eq!(v["i18n"]["strategy"], "prefix-except-default");
    assert_eq!(v["security"]["csp"], "strict");
    assert_eq!(v["rules"][0]["source"], "/old/:slug");
    assert_eq!(v["rules"][0]["redirect"], "/new/:slug");
    assert_eq!(v["rules"][0]["status"], 308);
    assert_eq!(v["budgets"]["clientJs"], "150kb");
    assert_eq!(v["build"]["target"], "baseline-widely-available");
    assert_eq!(v["adapter"], "binary");
    assert_eq!(v["plugins"][0]["name"], "tailwind");
    assert_eq!(v["modules"], serde_json::json!([]));
}

#[test]
fn extraction_forms() {
    // no `export`, double-quoted strings, trailing comma, comments, template
    // literal without interpolation, negative number, nested arrays.
    let src = r#"
        defineConfig({
            adapter: "node",
            // comment
            server: { host: `localhost`, ports: [-1, 2, 3], }, /* block */
            note: 'no ${interpolation} here',
        })
    "#;
    let v = extract_static_config(src).expect("literal forms extract");
    assert_eq!(v["adapter"], "node");
    assert_eq!(v["server"]["host"], "localhost");
    assert_eq!(v["server"]["ports"], serde_json::json!([-1, 2, 3]));
    assert_eq!(v["note"], "no ${interpolation} here");

    // every known framework factory resolves to the framework object
    for (f, name) in [("vue", "vue"), ("svelte", "svelte"), ("solid", "solid")] {
        let src =
            format!("export default defineConfig({{ framework: {f}({{ compiler: false }}) }})");
        let v = extract_static_config(&src).unwrap();
        assert_eq!(v["framework"]["name"], name);
        assert_eq!(v["framework"]["compiler"], false);
    }
}

#[test]
fn extraction_rejects_dynamic_shapes() {
    let cases: Vec<(&str, &str)> = vec![
        (
            "process.env read",
            "export default defineConfig({ port: process.env.PORT })",
        ),
        (
            "template interpolation",
            "export default defineConfig({ host: `x-${1}` })",
        ),
        (
            "function value",
            "export default defineConfig({ render: () => 'static' })",
        ),
        (
            "identifier reference",
            "const p = 3000\nexport default defineConfig({ port: p })",
        ),
        (
            "unknown factory",
            "export default defineConfig({ framework: myReact({ compiler: true }) })",
        ),
        (
            "variable spread",
            "export default defineConfig({ ...base })",
        ),
        (
            "arithmetic",
            "export default defineConfig({ port: 3000 + 1 })",
        ),
        (
            "top-level await",
            "export default defineConfig({ port: await getPort() })",
        ),
        (
            "factory with non-object arg",
            "export default defineConfig({ plugins: [tailwind('./tw.config.js')] })",
        ),
    ];
    for (label, src) in cases {
        assert!(extract_static_config(src).is_none(), "must reject: {label}");
    }
}

// ---------------------------------------------------------------------------
// load / layering
// ---------------------------------------------------------------------------

#[test]
fn load_layers_six_sources_in_order() {
    let (_t, dir) = fixture(&[
        (
            "preset.json",
            json_file(serde_json::json!({
                "adapter": "node",
                "server": { "port": 4100 },
                "budgets": { "clientJs": "100kb" }
            }))
            .as_str(),
        ),
        (
            "oxide.config.json",
            json_file(serde_json::json!({
                "extends": ["./preset.json"],
                "framework": { "adapter": "react" },
                "server": { "host": "base.example" },
                "render": { "default": "isr" }
            }))
            .as_str(),
        ),
        (
            "oxide.config.production.json",
            json_file(serde_json::json!({
                "server": { "port": 4200 },
                "build": { "target": "es2022" }
            }))
            .as_str(),
        ),
        (
            ".env",
            "OXIDE_RENDER_DEFAULT=ssr\nPUBLIC_API_URL=https://cdn.example\nSECRET_TOKEN=abc\n",
        ),
        (
            ".env.production",
            "OXIDE_RENDER_DEFAULT=static\nPUBLIC_MODE_FLAG=1\n",
        ),
    ]);

    let loaded = load_with_env(
        &dir,
        "production",
        cli(&[
            ("server.port", serde_json::json!(4300)),
            ("environments.client", serde_json::json!({"x": 1})),
        ]),
        &EMPTY_ENV,
    )
    .unwrap();

    // preset (via extends)
    assert_eq!(loaded.config.adapter, "node");
    assert_eq!(loaded.config.budgets.client_js.as_deref(), Some("100kb"));
    // base file
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("react"));
    assert_eq!(loaded.config.server.host, "base.example");
    // mode file
    assert_eq!(loaded.config.build.target, "es2022");
    // env files (mode file env beats plain .env) — the base's `isr` render
    // default is intentionally shadowed by the later OXIDE_RENDER_DEFAULT layer.
    assert_eq!(loaded.config.render.default, "static");
    // CLI overrides win over everything
    assert_eq!(loaded.config.server.port, 4300);
    assert_eq!(
        loaded.config.environments.get("client"),
        Some(&serde_json::json!({"x": 1}))
    );
    // env bookkeeping
    assert_eq!(
        loaded.env_sources,
        vec![dir.join(".env"), dir.join(".env.production")]
    );
    assert_eq!(
        loaded.env.get("OXIDE_RENDER_DEFAULT").map(String::as_str),
        Some("static")
    );
    assert_eq!(
        loaded.env.get("PUBLIC_API_URL").map(String::as_str),
        Some("https://cdn.example")
    );
    assert_eq!(
        loaded.env.get("SECRET_TOKEN").map(String::as_str),
        Some("abc")
    );
    // base config came from the JSON fallback → dynamic eval flagged (D14)
    assert!(loaded.used_dynamic_eval);
}

#[test]
fn load_resolves_nested_extends_and_errors_on_missing_preset() {
    let (_t, dir) = fixture(&[
        ("presets/common.json", json_file(serde_json::json!({ "security": { "csp": "hash" } })).as_str()),
        (
            "presets/base.json",
            json_file(serde_json::json!({ "extends": ["./common.json"], "adapter": "node" })).as_str(),
        ),
        (
            "oxide.config.json",
            json_file(serde_json::json!({ "extends": ["./presets/base.json"], "framework": { "adapter": "vue" } }))
                .as_str(),
        ),
    ]);
    let loaded = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert_eq!(loaded.config.security.csp, "hash", "nested preset merged");
    assert_eq!(loaded.config.adapter, "node");
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("vue"));

    let (_t2, dir2) = fixture(&[(
        "oxide.config.json",
        json_file(
            serde_json::json!({ "extends": ["./nope.json"], "framework": { "adapter": "vue" } }),
        )
        .as_str(),
    )]);
    let e = load_with_env(&dir2, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    assert_eq!(e.0.code, "CFG-EXTENDS");
}

#[test]
fn load_static_ts_end_to_end_with_spec_example() {
    let (_t, dir) = fixture(&[
        ("oxide.config.ts", SPEC_EXAMPLE),
        (
            "oxide.config.production.ts",
            "export default defineConfig({ server: { port: 8080 } })",
        ),
    ]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert!(!loaded.used_dynamic_eval, "both TS files static-extract");
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("react"));
    assert_eq!(loaded.config.framework.compiler, Some(true));
    assert_eq!(loaded.config.server.port, 8080, "mode file wins over base");
    assert_eq!(
        loaded.config.server.host, "localhost",
        "untouched defaults survive"
    );
    assert_eq!(loaded.config.routes.dir, "src/routes");
    assert_eq!(loaded.config.adapter, "binary");
    assert_eq!(loaded.config.cache.l0.max_size, "512mb");
    assert_eq!(
        loaded.config.cache.handler,
        oxide_config::CacheHandlerConfig::Disk
    );
    assert_eq!(loaded.config.rules.len(), 1);
    assert_eq!(loaded.config.rules[0].source, "/old/:slug");
    assert_eq!(loaded.config.rules[0].status, Some(308));
    assert_eq!(loaded.config.i18n.default_locale, "en");
    assert_eq!(loaded.config.i18n.strategy, "prefix-except-default");
    assert_eq!(
        loaded.config.images.remote_patterns[0].host,
        "cdn.example.com"
    );
    assert_eq!(loaded.config.plugins.len(), 1);
    assert_eq!(loaded.config.plugins[0]["name"], "tailwind");
}

#[test]
fn load_dynamic_ts_falls_back_to_json_and_flags_dynamic_eval() {
    let (_t, dir) = fixture(&[
        (
            "oxide.config.ts",
            "export default defineConfig({ port: process.env.PORT })",
        ),
        (
            "oxide.config.json",
            json_file(serde_json::json!({
                "framework": { "adapter": "svelte" },
                "server": { "port": 5100 }
            }))
            .as_str(),
        ),
    ]);
    let loaded = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert!(loaded.used_dynamic_eval, "TS needed V8 evaluation (D14)");
    assert_eq!(
        loaded.config.framework.adapter.as_deref(),
        Some("svelte"),
        "JSON fallback merged"
    );
    assert_eq!(loaded.config.server.port, 5100);
}

#[test]
fn load_json_only_base_sets_dynamic_flag_but_loads() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        json_file(serde_json::json!({ "framework": { "adapter": "solid" } })).as_str(),
    )]);
    let loaded = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert!(
        loaded.used_dynamic_eval,
        "JSON fallback for the base config flags dynamic eval"
    );
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("solid"));
    // everything else comes from the typed defaults
    assert_eq!(loaded.config.routes.dir, "src/routes");
    assert_eq!(loaded.config.server.port, 3000);
    assert_eq!(loaded.config.adapter, "binary");
    assert_eq!(loaded.config.i18n.strategy, "none");
    assert_eq!(loaded.config.build.sourcemap, "hidden");
}

#[test]
fn load_applies_oxide_env_allow_list_from_process_env() {
    let (_t, dir) = fixture(&[(
        "oxide.config.ts",
        "export default defineConfig({ framework: solid({}), server: { host: 'localhost' } })",
    )]);
    let loaded = load_with_env(
        &dir,
        "dev",
        BTreeMap::new(),
        &envmap(&[
            ("OXIDE_SERVER_PORT", "4000"),
            ("OXIDE_SERVER_HOST", "0.0.0.0"),
            ("OXIDE_RENDER_DEFAULT", "dynamic"),
            ("OXIDE_ADAPTER", "docker"),
            ("OXIDE_BUILD_TARGET", "es2023"),
            ("OXIDE_FRAMEWORK_NAME", "vue"), // NOT in the allow-list → ignored
        ]),
    )
    .unwrap();
    assert_eq!(
        loaded.config.server.port, 4000,
        "env value parsed as JSON number"
    );
    assert_eq!(loaded.config.server.host, "0.0.0.0");
    assert_eq!(loaded.config.render.default, "dynamic");
    assert_eq!(loaded.config.adapter, "docker");
    assert_eq!(loaded.config.build.target, "es2023");
    assert_eq!(
        loaded.config.framework.name.as_deref(),
        Some("solid"),
        "non-allow-listed OXIDE_FRAMEWORK_NAME must not override the factory result"
    );
    assert_ne!(loaded.config.framework.name.as_deref(), Some("vue"));
    assert!(!loaded.used_dynamic_eval);
}

#[test]
fn load_process_env_wins_over_env_file_values() {
    let (_t, dir) = fixture(&[
        (
            "oxide.config.ts",
            "export default defineConfig({ framework: vue({}) })",
        ),
        (".env", "OXIDE_SERVER_PORT=9999\nFILE_ONLY=1\n"),
    ]);
    let loaded = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert_eq!(
        loaded.config.server.port, 9999,
        "env-file OXIDE_ var feeds the allow-list"
    );
    assert_eq!(loaded.env.get("FILE_ONLY").map(String::as_str), Some("1"));

    let loaded2 = load_with_env(
        &dir,
        "dev",
        BTreeMap::new(),
        &envmap(&[("OXIDE_SERVER_PORT", "4000")]),
    )
    .unwrap();
    assert_eq!(
        loaded2.config.server.port, 4000,
        "process env beats the env file"
    );
    assert_eq!(
        loaded2.env.get("OXIDE_SERVER_PORT").map(String::as_str),
        Some("4000")
    );
    assert_eq!(loaded2.env.get("FILE_ONLY").map(String::as_str), Some("1"));
}

#[test]
fn load_unknown_key_is_rejected_with_typo_suggestion() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        json_file(
            serde_json::json!({ "servr": { "port": 1 }, "framework": { "adapter": "react" } }),
        )
        .as_str(),
    )]);
    let e = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    assert_eq!(e.0.code, "CFG-UNKNOWN-KEY");
    assert!(
        e.0.message.contains("servr"),
        "names the offending key: {}",
        e.0.message
    );
    assert!(
        e.0.message.contains("server"),
        "suggests the close key: {}",
        e.0.message
    );
    assert!(e.0.fix.is_some(), "§19.4 fix hint attached");
}

#[test]
fn load_unknown_key_without_close_match_has_no_suggestion() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        json_file(serde_json::json!({ "zzzzzzz": 1, "framework": { "adapter": "react" } }))
            .as_str(),
    )]);
    let e = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    assert_eq!(e.0.code, "CFG-UNKNOWN-KEY");
    assert!(
        !e.0.message.contains("did you mean"),
        "no bogus suggestion: {}",
        e.0.message
    );
}

#[test]
fn load_missing_framework_adapter_errors_naming_file_and_key() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        json_file(serde_json::json!({ "framework": { "name": "react" } })).as_str(),
    )]);
    let e = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    assert_eq!(e.0.code, "CFG-FRAMEWORK");
    assert!(
        e.0.message.contains("framework.adapter"),
        "names the key: {}",
        e.0.message
    );
    assert!(
        e.0.message.contains("oxide.config.json"),
        "names the contributing file: {}",
        e.0.message
    );
}

#[test]
fn load_cli_overrides_apply_dotted_keys_last() {
    let (_t, dir) = fixture(&[(
        "oxide.config.ts",
        "export default defineConfig({ framework: react({ compiler: true }), server: { port: 3000 } })",
    )]);
    let loaded = load_with_env(
        &dir,
        "dev",
        cli(&[
            ("server.port", serde_json::json!(3005)),
            ("adapter", serde_json::json!("static")),
            ("environments.client", serde_json::json!({"foo": "bar"})),
        ]),
        &EMPTY_ENV,
    )
    .unwrap();
    assert_eq!(loaded.config.server.port, 3005);
    assert_eq!(loaded.config.adapter, "static");
    assert_eq!(
        loaded.config.environments.get("client"),
        Some(&serde_json::json!({"foo": "bar"}))
    );
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("react"));
}

#[test]
fn load_reports_env_sources_for_existing_files_only() {
    let (_t, dir) = fixture(&[
        (
            "oxide.config.ts",
            "export default defineConfig({ framework: svelte({}) })",
        ),
        (".env", "A=1\n"),
        (".env.local", "A=2\nB=3\n"),
    ]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    // order: .env, .env.<mode>, .env.local, .env.<mode>.local — existing only
    assert_eq!(
        loaded.env_sources,
        vec![dir.join(".env"), dir.join(".env.local")]
    );
    assert_eq!(
        loaded.env.get("A").map(String::as_str),
        Some("2"),
        ".env.local overrides .env"
    );
    assert_eq!(loaded.env.get("B").map(String::as_str), Some("3"));
}

#[test]
fn public_env_exposes_only_public_prefixed_vars() {
    let (_t, dir) = fixture(&[
        (
            "oxide.config.ts",
            "export default defineConfig({ framework: react({}) })",
        ),
        (
            ".env",
            "PUBLIC_API_URL=https://api\nPUBLIC_TOKEN=tok\nPRIVATE_KEY=hidden\n",
        ),
    ]);
    let loaded = load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV).unwrap();
    let public = loaded.public_env();
    assert_eq!(public.len(), 2, "only PUBLIC_-prefixed vars are exposed");
    assert_eq!(
        public.get("PUBLIC_API_URL").map(String::as_str),
        Some("https://api")
    );
    assert_eq!(public.get("PUBLIC_TOKEN").map(String::as_str), Some("tok"));
    assert!(!public.contains_key("PRIVATE_KEY"));
    assert!(
        loaded.env.contains_key("PRIVATE_KEY"),
        "private vars stay in the full map"
    );
}

#[test]
fn load_uses_the_real_process_environment() {
    let (_t, dir) = fixture(&[(
        "oxide.config.ts",
        "export default defineConfig({ framework: react({ compiler: false }) })",
    )]);
    let loaded = oxide_config::load(&dir, "dev", BTreeMap::new()).unwrap();
    assert_eq!(loaded.config.framework.adapter.as_deref(), Some("react"));
    assert_eq!(loaded.config.server.port, 3000);
}

#[test]
fn typed_config_serializes_camel_case_and_roundtrips() {
    let src = r#"{
        "framework": { "name": "react", "adapter": "react", "compiler": true },
        "server": { "port": 3001, "http3": true },
        "rules": [{ "source": "/a", "status": 301 }]
    }"#;
    let mut cfg: OxideConfig = serde_json::from_str(src).unwrap();
    cfg.framework.name = Some("react".into());
    let json = serde_json::to_value(&cfg).unwrap();
    assert!(json.get("framework").is_some(), "camelCase top-level keys");
    assert_eq!(json["server"]["port"], 3001);
    assert_eq!(
        json["server"]["http3"], true,
        "camelCase field names (http3)"
    );
    let back: OxideConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back, cfg);
}

// ---------------------------------------------------------------------------
// extract_islands_map — §10.3 regression tests.
//
// The earlier implementation used `source.find("islands")` and walked to the
// next `{`. That matched the smoke-app config's own comment ("see
// src/islands/counter.tsx") and then grabbed the `components = {...}` map.
// These tests pin the fixed behavior.
// ---------------------------------------------------------------------------

#[test]
fn islands_extractor_reads_a_simple_map() {
    let src = r#"
import react from '@oxide-fw/react'
export default {
  framework: react({
    islands: {
      Counter: 'src/islands/counter.tsx',
      Widget: 'src/islands/widget.tsx',
    },
  }),
}
"#;
    let v = oxide_config::extract_for_test(src).expect("should extract something");
    let islands = v
        .get("framework")
        .and_then(|f| f.get("islands"))
        .and_then(|i| i.as_object())
        .expect("islands map");
    assert_eq!(islands.len(), 2);
    assert_eq!(islands["Counter"], "src/islands/counter.tsx");
    assert_eq!(islands["Widget"], "src/islands/widget.tsx");
}

#[test]
fn islands_extractor_ignores_the_word_in_comments() {
    // The failing case at handoff: the smoke-app config mentions
    // `src/islands/counter.tsx` inside a comment, and has a separate
    // `components = { ... }` map. A naive `find("islands")` picked up
    // that comment, walked to the next `{`, and grabbed `components`.
    let src = r#"
// The home page renders a `<ox-island>` for the Counter component
// (see src/islands/counter.tsx). The Playwright suite in tests/playwright/
// asserts the SSR'd island markup end-to-end.

const components = {
  'routes/index': Home,
  'routes/blog/[slug]': Post,
}

export default {
  framework: react({
    islands: { Counter: 'src/islands/counter.tsx' },
  }),
}
"#;
    let v = oxide_config::extract_for_test(src).expect("should extract something");
    let islands = v
        .get("framework")
        .and_then(|f| f.get("islands"))
        .and_then(|i| i.as_object())
        .expect("islands map");
    assert_eq!(
        islands.len(),
        1,
        "only the real islands block counts: {islands:?}"
    );
    assert_eq!(islands["Counter"], "src/islands/counter.tsx");
    assert!(
        !islands.contains_key("routes/index"),
        "must not grab the components map"
    );
}

#[test]
fn islands_extractor_no_block_yields_none() {
    let src = "export default { framework: react({}) }
";
    let v = oxide_config::extract_for_test(src);
    assert!(
        v.and_then(|x| x.get("framework").and_then(|f| f.get("islands")).cloned())
            .is_none(),
        "no islands block must not surface a map"
    );
}

#[test]
fn islands_extractor_handles_path_with_brace() {
    // A string containing `{` must not throw off brace counting.
    let src = r#"
export default {
  framework: react({
    islands: { Weird: 'src/a{b/c.tsx' },
  }),
}
"#;
    let v = oxide_config::extract_for_test(src).expect("should extract something");
    let islands = v
        .get("framework")
        .and_then(|f| f.get("islands"))
        .and_then(|i| i.as_object())
        .expect("islands map");
    assert_eq!(islands["Weird"], "src/a{b/c.tsx");
}
