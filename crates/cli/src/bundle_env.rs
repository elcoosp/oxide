//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// Framework runtime packages the ESM client graph needs prebundled beyond
/// the React-shaped default list. The pool-compiled client module for each
/// framework imports its runtime by bare specifier (`vue`,
/// `svelte/internal/client`, `@solidjs/web`), and the `@oxide-fw/*`
/// adapter dist files do the same — without these entries the browser
/// fetches a bare specifier and the whole module graph halts.
///
/// `solid-js` is listed for the Solid client adapter's dynamic
/// `import('solid-js')`; `svelte/internal/disclose-version` is emitted by
/// every Svelte 5 compiled module, and `svelte/internal/flags/legacy` by
/// the Svelte 5.3x compiler (it gates the legacy-syntax shims).
pub(crate) fn framework_prebundle_extras(framework: &str) -> Vec<String> {
    match framework {
        "vue" => vec!["vue".to_string()],
        "svelte" => vec![
            "svelte".to_string(),
            "svelte/internal/client".to_string(),
            "svelte/internal/disclose-version".to_string(),
            "svelte/internal/flags/legacy".to_string(),
        ],
        "solid" => vec!["solid-js".to_string(), "@solidjs/web".to_string()],
        _ => Vec::new(),
    }
}

/// §6.4 / §21: the framework runtimes must never be inlined into the SSR
/// bundle. Each runtime carries module-level state (React's
/// `ReactCurrentDispatcher`, Vue's current instance, Svelte's component
/// context, Solid's reactive root); two copies silently break hook dispatch
/// (the classic "invalid hook call / reading 'useState' of null" failure).
///
/// The user's `build.ssrExternals` still wins when set; this default only
/// applies when that list is empty. Extra entries are harmless — an external
/// that is never imported is never resolved.
pub(crate) fn default_ssr_externals() -> Vec<String> {
    // §21: SSR externals for every supported framework. React is
    // externalized (not bundled) because react/jsx-runtime and
    // react-dom/server are CJS. Inlining either produces a second React
    // copy alongside node_modules' copy — the plain-Node render of the
    // bundled page then throws "Invalid hook call / two Reacts". The V8
    // loader handles CJS→ESM interop separately; that problem must not be
    // worked around by changing bundler output.
    [
        // React
        "react",
        "react-dom",
        "react/jsx-runtime",
        "react/jsx-dev-runtime",
        "react-dom/server",
        "react-dom/client",
        // Vue
        "vue",
        "vue/server-renderer",
        // Svelte
        "svelte",
        "svelte/internal",
        // Solid: v1 namespace + v2 package (renamed `./web` -> `@solidjs/web`)
        "solid-js",
        "solid-js/web",
        "solid-js/server",
        "@solidjs/web",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub(crate) struct BundleEnvArgs {
    pub env: oxide_fw_bundler::BundleEnv,
    pub entries: BTreeMap<String, PathBuf>,
    pub out_dir: PathBuf,
    pub precompiled: BTreeMap<PathBuf, String>,
    pub jsx_import_source: Option<String>,
    pub build_secret: String,
    pub transform_hook: Option<oxide_fw_bundler::TransformHook>,
    pub css_transform: oxide_fw_bundler::CssTransformHook,
    pub ssr_externals: Vec<String>,
    pub sourcemap_on: bool,
    pub minify: bool,
}

pub(crate) fn bundle_env_options(args: BundleEnvArgs) -> oxide_fw_bundler::BundleOptions {
    let BundleEnvArgs {
        env,
        entries,
        out_dir,
        precompiled,
        jsx_import_source,
        build_secret,
        transform_hook,
        css_transform,
        ssr_externals,
        sourcemap_on,
        minify,
    } = args;

    // §6.1: `process.env.NODE_ENV` drives React's dev-vs-production
    // bundle selection. Setting it at the CLI level (rather than in the
    // bundler) keeps the bundler agnostic of a project's framework — the
    // CLI is what decides a build is a production build. Without this,
    // React ships the dev build (~1 MB) into every island chunk.
    // P1 (readiness): dev-mode probes answer from the atomic (set once at
    // startup), not a racy per-call env read.
    let is_dev = oxide_fw_server::dev_mode();
    let mut define: BTreeMap<String, String> = BTreeMap::new();
    match env {
        oxide_fw_bundler::BundleEnv::Client => {
            // [wr102] NODE_ENV must match dev/prod, not be hardcoded to
            // production. Otherwise `oxide dev` ships React's minified
            // production build and #418 becomes unreadable.
            let env_val = if minify && !is_dev {
                "production"
            } else {
                "development"
            };
            define.insert("process.env.NODE_ENV".to_string(), format!("\"{env_val}\""));
        }
        oxide_fw_bundler::BundleEnv::Ssr => {
            // Server rendering must NOT bake in "production": loaders may
            // read `process.env.NODE_ENV` at request time to decide dev
            // diagnostics vs. silent failures, and a worker evaluates them
            // with the real environment. Left unset so the define is a
            // no-op on the server bundle.
        }
        oxide_fw_bundler::BundleEnv::Edge => {
            // Edge runtimes do not have a `process` global; a define
            // baking in "production" would be accurate but the runtime is
            // a browser-like sandbox, so leave it untouched for now.
        }
    }
    oxide_fw_bundler::BundleOptions {
        env,
        entries,
        out_dir,
        minify,
        // BUG-11: honour `build.sourcemap`. `RolldownBundler` supports
        // `SourceMapType::File`; `off` emits no map.
        sourcemap: sourcemap_on,
        // §21: externals only apply to the server bundle; a browser bundle
        // must inline everything.
        externals: match env {
            oxide_fw_bundler::BundleEnv::Ssr => {
                let mut e = ssr_externals;
                if e.is_empty() {
                    e = default_ssr_externals();
                }
                e
            }
            _ => Vec::new(),
        },
        define,
        tsconfig_paths: BTreeMap::new(),
        build_secret,
        precompiled,
        jsx_import_source,
        transform_hook,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: Some(css_transform),
        vendor_groups: Vec::new(),
        inline_dynamic_imports: false,
        resolve_conditions: None,
        virtual_modules: Default::default(),
    }
}
