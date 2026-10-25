//! SPEC-15.1 / §15.2: native plugin registry.
//!
//! `config.plugins` entries with `kind: "native"` name a BUILT-IN feature
//! (images, fonts, i18n, mdx, tailwind, manifest) instead of an external
//! JS/WASM module. This module is the registry: `NativePlugin::from_name`
//! maps the name to a variant, and `parse_native_plugins` walks the config
//! and either returns the recognised set or fails with `OX-PLUGIN` on an
//! unknown name (SPEC-15.2 "fail at load").
//!
//! Per-variant feature wiring is a follow-up: each variant's underlying
//! feature already works when its `config.<feature>` block is set. The
//! native plugin is a discoverable alias. `enables(cfg, plugin)` is the
//! one-liner callers use once that flip is in place.

use oxide_fw_config::OxideConfig;
use oxide_fw_diagnostics::OxideError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePlugin {
    Images,
    Fonts,
    I18n,
    Mdx,
    Tailwind,
    Manifest,
}

impl NativePlugin {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "images" | "image" => Self::Images,
            "fonts" | "font" => Self::Fonts,
            "i18n" => Self::I18n,
            "mdx" => Self::Mdx,
            "tailwind" | "tailwindcss" => Self::Tailwind,
            "manifest" | "pwa" => Self::Manifest,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Images => "images",
            Self::Fonts => "fonts",
            Self::I18n => "i18n",
            Self::Mdx => "mdx",
            Self::Tailwind => "tailwind",
            Self::Manifest => "manifest",
        }
    }
    /// §15.2 / WP-J2: the feature gates this plugin activates. Each
    /// native plugin owns a feature domain of the same name; a plugin
    /// that declares no gate is validation-only (a name for aliasing,
    /// nothing to toggle). `cmd_check` reports these so a user sees
    /// the actual activated set, not just the declared names.
    pub fn enables(&self) -> &'static [&'static str] {
        match self {
            NativePlugin::Images => &["images"],
            NativePlugin::Fonts => &["fonts"],
            NativePlugin::I18n => &["i18n"],
            NativePlugin::Mdx => &["mdx"],
            NativePlugin::Tailwind => &["tailwind"],
            NativePlugin::Manifest => &["manifest"],
        }
    }

    pub fn all_names() -> &'static str {
        "images, fonts, i18n, mdx, tailwind, manifest"
    }
}

/// SPEC-15.2 "fail at load": walk `config.plugins`, return every recognised
/// `kind:"native"` entry OR the first unknown-name error. JS and WASM
/// entries are skipped (their own loaders handle them).
pub fn parse_native_plugins(cfg: &OxideConfig) -> Result<Vec<NativePlugin>, OxideError> {
    let mut out = Vec::new();
    for (i, p) in cfg.plugins.iter().enumerate() {
        if p.get("kind").and_then(|v| v.as_str()) != Some("native") {
            continue;
        }
        let Some(name) = p.get("name").and_then(|v| v.as_str()) else {
            return Err(OxideError::new(
                "OX-PLUGIN",
                format!("plugins[{i}] (kind:\"native\") is missing a `name` field"),
            ));
        };
        match NativePlugin::from_name(name) {
            Some(n) => out.push(n),
            None => {
                return Err(OxideError::new(
                    "OX-PLUGIN",
                    format!(
                        "plugins[{i}]: unknown native plugin `{name}`. Known: {}",
                        NativePlugin::all_names(),
                    ),
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_name_and_from_name() {
        for p in [
            NativePlugin::Images,
            NativePlugin::Fonts,
            NativePlugin::I18n,
            NativePlugin::Mdx,
            NativePlugin::Tailwind,
            NativePlugin::Manifest,
        ] {
            assert_eq!(NativePlugin::from_name(p.name()), Some(p));
        }
    }

    #[test]
    fn aliases_map_to_canonical_variant() {
        assert_eq!(NativePlugin::from_name("image"), Some(NativePlugin::Images));
        assert_eq!(NativePlugin::from_name("font"), Some(NativePlugin::Fonts));
        assert_eq!(
            NativePlugin::from_name("tailwindcss"),
            Some(NativePlugin::Tailwind)
        );
        assert_eq!(NativePlugin::from_name("pwa"), Some(NativePlugin::Manifest));
    }

    #[test]
    fn unknown_name_is_none() {
        assert_eq!(NativePlugin::from_name("bogus"), None);
        assert_eq!(NativePlugin::from_name(""), None);
    }

    #[test]
    fn parse_skips_non_native_kinds() {
        let mut cfg = OxideConfig::default();
        cfg.plugins = vec![
            serde_json::json!({"kind":"js","path":"./a.mjs"}),
            serde_json::json!({"kind":"native","name":"images"}),
            serde_json::json!({"kind":"wasm","path":"./b.wasm"}),
        ];
        assert_eq!(
            parse_native_plugins(&cfg).expect("parse ok"),
            vec![NativePlugin::Images]
        );
    }

    #[test]
    fn parse_rejects_unknown_native() {
        let mut cfg = OxideConfig::default();
        cfg.plugins = vec![serde_json::json!({"kind":"native","name":"bogus"})];
        let e = parse_native_plugins(&cfg).unwrap_err();
        assert_eq!(e.0.code, "OX-PLUGIN");
        assert!(e.0.message.contains("unknown native plugin"));
        assert!(e.0.message.contains("images"));
    }

    #[test]
    fn parse_rejects_native_without_name() {
        let mut cfg = OxideConfig::default();
        cfg.plugins = vec![serde_json::json!({"kind":"native"})];
        let e = parse_native_plugins(&cfg).unwrap_err();
        assert_eq!(e.0.code, "OX-PLUGIN");
        assert!(e.0.message.contains("missing a `name`"));
    }

    #[test]
    fn enables_returns_the_variant_gate() {
        assert_eq!(NativePlugin::Images.enables(), &["images"]);
        assert_eq!(NativePlugin::I18n.enables(), &["i18n"]);
        assert_eq!(NativePlugin::Mdx.enables(), &["mdx"]);
        // Every variant today has at least one gate; a future
        // validation-only variant would need a note here.
        let all = [
            NativePlugin::Images,
            NativePlugin::Fonts,
            NativePlugin::I18n,
            NativePlugin::Mdx,
            NativePlugin::Tailwind,
            NativePlugin::Manifest,
        ];
        for p in all {
            assert!(
                !p.enables().is_empty(),
                "variant {} declares no gate — is that intentional?",
                p.name()
            );
        }
    }
}
