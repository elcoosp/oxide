//! Prebundle CJS dependencies to ESM for the native ESM dev server.
//!
//! The browser can only execute ESM. React, react-dom and similar packages
//! publish CJS entry points. This module bundles each entry to a single-file
//! ESM module with `process.env.NODE_ENV` defined (React's CJS branches on
//! it) and no externals — the client has no `require()`.
//!
//! The caller (oxide-fw-cli's `esm_dev`) passes the package names from
//! `DevConfig::prebundle` and the output directory; each becomes one file
//! `<sanitized-name>.js`. A per-package failure is recorded and the rest
//! continue, so a missing optional dep doesn't break the dev server.

use std::path::{Path, PathBuf};

/// Outcome of [`prebundle_cjs_to_esm`]: which packages bundled, which failed.
#[derive(Debug, Default)]
pub struct PrebundleResult {
    /// Package names that bundled successfully.
    pub bundled: Vec<String>,
    /// Package names that failed, with a human-readable error.
    pub failures: Vec<(String, String)>,
}

/// Bundle each `package` from CJS to ESM in `out_dir`, one file per entry.
///
/// Resolution walks `node_modules` from `root` via `oxc_resolver`, so a
/// workspace hoist and a per-package install both work. Output uses
/// `platform = Browser` (React's `react-dom` picks the browser client) and
/// `format = Esm`. `process.env.NODE_ENV` is defined to `"development"`.
pub fn prebundle_cjs_to_esm(
    root: &Path,
    packages: &[String],
    out_dir: &Path,
) -> anyhow::Result<PrebundleResult> {
    let resolver = oxc_resolver::Resolver::new(oxc_resolver::ResolveOptions::default());
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    for name in packages {
        match resolver.resolve(root, name) {
            Ok(res) => entries.push((name.clone(), res.path().to_path_buf())),
            Err(e) => failures.push((name.clone(), format!("{e:?}"))),
        }
    }
    if entries.is_empty() {
        return Ok(PrebundleResult {
            bundled: Vec::new(),
            failures,
        });
    }

    std::fs::create_dir_all(out_dir)?;

    let root_owned = root.to_path_buf();
    let out_dir_owned = out_dir.to_path_buf();
    let (bundled, more_failures) = std::thread::scope(|scope| {
        scope
            .spawn(move || -> (Vec<String>, Vec<(String, String)>) {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        return (
                            Vec::new(),
                            vec![("<runtime>".into(), format!("tokio: {e}"))],
                        );
                    }
                };
                rt.block_on(async move {
                    let mut ok = Vec::new();
                    let mut fails = Vec::new();
                    for (name, entry_path) in entries {
                        let file_name = sanitized_filename(&name);
                        let stem = file_name
                            .strip_suffix(".js")
                            .unwrap_or(&file_name)
                            .to_string();
                        // React's CJS branches on `process.env.NODE_ENV`;
                        // the browser has no `process`, so bake it in.
                        let opts = rolldown::BundlerOptions {
                            input: Some(vec![rolldown::InputItem {
                                name: Some(stem),
                                import: entry_path.to_string_lossy().into_owned(),
                            }]),
                            cwd: Some(root_owned.clone()),
                            platform: Some(rolldown::Platform::Browser),
                            format: Some(rolldown::OutputFormat::Esm),
                            dir: Some(out_dir_owned.to_string_lossy().into_owned()),
                            define: Some(
                                [(
                                    "process.env.NODE_ENV".to_string(),
                                    "\"development\"".to_string(),
                                )]
                                .into_iter()
                                .collect(),
                            ),
                            ..Default::default()
                        };
                        let mut b = match rolldown::BundlerBuilder::default()
                            .with_options(opts)
                            .build()
                        {
                            Ok(b) => b,
                            Err(e) => {
                                fails.push((name, format!("init: {e}")));
                                continue;
                            }
                        };
                        match b.write().await {
                            Ok(_) => ok.push(name),
                            Err(e) => fails.push((name, format!("write: {e}"))),
                        }
                    }
                    (ok, fails)
                })
            })
            .join()
            .unwrap_or_else(|_| {
                (
                    Vec::new(),
                    vec![("<thread>".into(), "prebundle worker panicked".into())],
                )
            })
    });

    failures.extend(more_failures);
    Ok(PrebundleResult { bundled, failures })
}

fn sanitized_filename(name: &str) -> String {
    let mut s: String = name.replace('/', "-").replace('@', "");
    s.push_str(".js");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_package_names() {
        assert_eq!(sanitized_filename("react"), "react.js");
        assert_eq!(sanitized_filename("react-dom/client"), "react-dom-client.js");
        assert_eq!(sanitized_filename("react/jsx-runtime"), "react-jsx-runtime.js");
        assert_eq!(sanitized_filename("rangi/themes"), "rangi-themes.js");
    }

    #[test]
    fn empty_packages_yields_empty_result() {
        let tmp = tempfile::tempdir().unwrap();
        let r = prebundle_cjs_to_esm(tmp.path(), &[], &tmp.path().join("out")).unwrap();
        assert!(r.bundled.is_empty());
        assert!(r.failures.is_empty());
    }
}
