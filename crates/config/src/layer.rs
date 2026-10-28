//! Config layering + validation (spec §14.2, D14).
//!
//! Order: built-in defaults → `extends` presets (relative paths, recursive with
//! a depth guard) → `oxide.config.{ts,json}` → `oxide.config.<mode>.{ts,json}`
//! → `OXIDE_*` environment variables (allow-list) → CLI overrides (dotted keys).
//! Env files load as `.env`, `.env.<mode>`, `.env.local`, `.env.<mode>.local`;
//! the process environment wins over file values (dotenv semantics).
//!
//! Merging is object-recursive: scalars, arrays and `null` replace, objects
//! merge key-by-key, so partial overrides (e.g. just `server.port`) keep the
//! remaining typed defaults. Validation rejects unknown top-level keys with a
//! Levenshtein ≤ 2 "did you mean" hint and requires `framework.adapter`.

use crate::{OxideConfig, Result, err, extract_static_config, parse_env_file};
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Top-level keys of [`OxideConfig`] (serde camelCase names).
const KNOWN_KEYS: &[&str] = &[
    "framework",
    "routes",
    "server",
    "render",
    "cache",
    "images",
    "fonts",
    "i18n",
    "security",
    "rules",
    "budgets",
    "build",
    "adapter",
    "content",
    "static",
    "plugins",
    "modules",
    "extends",
    "environments",
    "dev",
    "siteUrl",
    "manifest",
    "sw",
    "og",
    "imagesEndpoint",
];

/// Environment variables allowed to override config (§14.2), as dotted paths.
const ENV_ALLOW_LIST: &[&str] = &[
    "server.port",
    "server.host",
    "render.default",
    "adapter",
    "build.target",
];

pub(crate) fn load_with_env(
    dir: &Path,
    mode: &str,
    cli_overrides: BTreeMap<String, Value>,
    process_env: &BTreeMap<String, String>,
) -> Result<crate::LoadedConfig> {
    let mut value: Value = serde_json::to_value(OxideConfig::default())
        .map_err(|e| err("CFG-SCHEMA", format!("serialize defaults: {e}")))?;
    let mut used_dynamic_eval = false;
    let mut framework_source = "the built-in defaults".to_string();

    // 1+2. Base config file with its `extends` presets.
    if let Some(layer) = read_layer(dir, "oxide.config", &mut used_dynamic_eval)? {
        let extends: Vec<String> = layer
            .value
            .get("extends")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        for spec in &extends {
            let preset = load_preset(dir, spec, &mut used_dynamic_eval, 0)?;
            if preset.value.get("framework").is_some() {
                framework_source = format!("preset `{spec}`");
            }
            merge_into(&mut value, &preset.value);
        }
        if layer.value.get("framework").is_some() {
            framework_source = layer.path.display().to_string();
        }
        merge_into(&mut value, &layer.value);
    }

    // 3. Mode override file.
    if let Some(layer) = read_layer(dir, &format!("oxide.config.{mode}"), &mut used_dynamic_eval)? {
        if layer.value.get("framework").is_some() {
            framework_source = layer.path.display().to_string();
        }
        merge_into(&mut value, &layer.value);
    }

    // 4. Env files, then the process environment (which wins over files).
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut env_sources: Vec<PathBuf> = Vec::new();
    let env_names = [
        ".env".to_string(),
        format!(".env.{mode}"),
        ".env.local".to_string(),
        format!(".env.{mode}.local"),
    ];
    for name in &env_names {
        let p = dir.join(name);
        if p.is_file() {
            let content = std::fs::read_to_string(&p)
                .map_err(|e| err("CFG-IO", format!("read {}: {e}", p.display())))?;
            env.extend(parse_env_file(&content));
            env_sources.push(p);
        }
    }
    for (k, v) in process_env {
        env.insert(k.clone(), v.clone());
    }

    // 5. `OXIDE_*` allow-list → dotted config paths. `OXIDE_SERVER_PORT` →
    //    `server.port`. Values are parsed as JSON when possible ("4000" → 4000).
    for (key, raw) in &env {
        let Some(rest) = key.strip_prefix("OXIDE_") else {
            continue;
        };
        let dotted = rest.to_ascii_lowercase().replace('_', ".");
        if ENV_ALLOW_LIST.contains(&dotted.as_str()) {
            let parsed = serde_json::from_str::<Value>(raw).unwrap_or(Value::String(raw.clone()));
            set_dotted(&mut value, &dotted, parsed);
        }
    }

    // 6. CLI overrides (dotted keys), the final layer.
    for (k, v) in cli_overrides {
        set_dotted(&mut value, &k, v);
    }

    validate(&value, &framework_source)?;

    let config: OxideConfig = serde_json::from_value(value).map_err(|e| {
        err(
            "CFG-SCHEMA",
            format!("config failed schema validation: {e}"),
        )
    })?;

    Ok(crate::LoadedConfig {
        config,
        env_sources,
        env,
        used_dynamic_eval,
    })
}

// ---------------------------------------------------------------------------
// file layers
// ---------------------------------------------------------------------------

struct Layer {
    value: Value,
    path: PathBuf,
}

/// Read `<dir>/<stem>.{ts,json}`: the TS variant is statically extracted; when
/// that fails (or the file is absent) the JSON variant is the fallback. Using
/// the JSON fallback for the base config means V8 evaluation was needed →
/// `used_dynamic_eval = true` (D14).
fn read_layer(dir: &Path, stem: &str, used_dynamic_eval: &mut bool) -> Result<Option<Layer>> {
    let ts = dir.join(format!("{stem}.ts"));
    let json = dir.join(format!("{stem}.json"));
    if ts.is_file() {
        let src = std::fs::read_to_string(&ts)
            .map_err(|e| err("CFG-IO", format!("read {}: {e}", ts.display())))?;
        match extract_static_config(&src) {
            Some(v) => return Ok(Some(Layer { value: v, path: ts })),
            None => {
                *used_dynamic_eval = true;
                if json.is_file() {
                    return Ok(Some(Layer {
                        value: read_json(&json)?,
                        path: json,
                    }));
                }
                return Ok(None); // dynamic TS, nothing static to merge
            }
        }
    }
    if json.is_file() {
        if stem == "oxide.config" {
            *used_dynamic_eval = true; // JSON fallback for the base config (D14)
        }
        return Ok(Some(Layer {
            value: read_json(&json)?,
            path: json,
        }));
    }
    Ok(None)
}

fn read_json(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| err("CFG-IO", format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&content)
        .map_err(|e| err("CFG-IO", format!("parse {}: {e}", path.display())))
}

/// Resolve one `extends` preset (relative to the project `dir`), recursively.
fn load_preset(
    dir: &Path,
    spec: &str,
    used_dynamic_eval: &mut bool,
    depth: usize,
) -> Result<Layer> {
    if depth > 8 {
        return Err(err(
            "CFG-EXTENDS",
            format!("extends chain too deep at `{spec}` (cycle?)"),
        ));
    }
    let direct = dir.join(spec);
    let candidates: Vec<PathBuf> = if direct.extension().is_some() {
        vec![direct]
    } else {
        let mut v = vec![
            dir.join(format!("{spec}.ts")),
            dir.join(format!("{spec}.json")),
        ];
        // SPEC-16: a bare package spec resolves from the workspace root's
        // node_modules when `dir` is inside one.
        if let Some(ws) = crate::resolve_workspace_preset(dir, spec) {
            v.push(ws);
        }
        v
    };
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let mut value = if path.extension().map(|e| e == "ts").unwrap_or(false) {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| err("CFG-IO", format!("read {}: {e}", path.display())))?;
            match extract_static_config(&src) {
                Some(v) => v,
                None => {
                    *used_dynamic_eval = true;
                    return Err(err(
                        "CFG-EXTENDS",
                        format!(
                            "preset `{}` could not be statically extracted",
                            path.display()
                        ),
                    ));
                }
            }
        } else {
            read_json(&path)?
        };
        // Nested extends: merge the preset's own presets under it. Their paths
        // resolve relative to *this preset's* directory.
        let nested: Vec<String> = value
            .get("extends")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if !nested.is_empty() {
            let preset_dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dir.to_path_buf());
            let mut acc: Value = Value::Object(Map::new());
            for spec in &nested {
                let p = load_preset(&preset_dir, spec, used_dynamic_eval, depth + 1)?;
                merge_into(&mut acc, &p.value);
            }
            merge_into(&mut acc, &value);
            value = acc;
        }
        return Ok(Layer { value, path });
    }
    Err(err(
        "CFG-EXTENDS",
        format!(
            "extends preset `{spec}` not found (relative to {})",
            dir.display()
        ),
    ))
}

// ---------------------------------------------------------------------------
// merge + dotted paths
// ---------------------------------------------------------------------------

/// Object-recursive merge: objects merge per key, everything else replaces.
fn merge_into(acc: &mut Value, layer: &Value) {
    match (acc, layer) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    Some(slot) => merge_into(slot, v),
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (acc, v) => *acc = v.clone(),
    }
}

/// Set `value` at a dotted path (`server.port`), creating intermediate objects.
fn set_dotted(target: &mut Value, path: &str, value: Value) {
    let mut parts = path.split('.').peekable();
    let mut cur = target;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if let Value::Object(map) = cur {
                map.insert(part.to_string(), value);
            }
            return;
        }
        if !matches!(cur, Value::Object(_)) {
            *cur = Value::Object(Map::new());
        }
        let Value::Object(map) = cur else { return };
        let slot = map
            .entry(part.to_string())
            .or_insert(Value::Object(Map::new()));
        cur = slot;
    }
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

fn validate(value: &Value, framework_source: &str) -> Result<()> {
    let Value::Object(map) = value else {
        return Err(err("CFG-UNKNOWN-KEY", "config root must be an object"));
    };
    for key in map.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            let suggestion = closest_key(key, KNOWN_KEYS);
            let e = match suggestion {
                Some(s) => err(
                    "CFG-UNKNOWN-KEY",
                    format!("unknown config key `{key}` — did you mean `{s}`?"),
                ),
                None => err("CFG-UNKNOWN-KEY", format!("unknown config key `{key}`")),
            };
            return Err(OxideError(Box::new(e.0.with_fix(format!(
                "rename `{key}` to a valid top-level config key (one of {KNOWN_KEYS:?})"
            )))));
        }
    }
    let adapter_missing = value
        .get("framework")
        .and_then(|f| f.get("adapter"))
        .map(|v| v.is_null())
        .unwrap_or(true);
    if adapter_missing {
        let mut d = Diagnostic::error(
            "CFG-FRAMEWORK",
            format!(
                "`framework.adapter` is missing — set it (react|vue|svelte|solid) in {framework_source}"
            ),
        );
        d.fix = Some(
            "add `framework.adapter` to the config, e.g. `framework: react({ compiler: true })`"
                .into(),
        );
        return Err(OxideError(Box::new(d)));
    }
    Ok(())
}

/// Levenshtein edit distance (small strings only — config keys).
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The known key within edit distance 2 of `key`, closest first (§14.2).
fn closest_key<'k>(key: &str, known: &[&'k str]) -> Option<&'k str> {
    let lower = key.to_ascii_lowercase();
    known
        .iter()
        .filter(|&&k| levenshtein(&lower, k) <= 2)
        .min_by_key(|&&k| levenshtein(&lower, k))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("servr", "server"), 1);
        assert_eq!(levenshtein("secuity", "security"), 1);
        assert!(levenshtein("xyzzy", "framework") > 2);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn dotted_set_creates_nested_objects() {
        let mut v = serde_json::json!({"a": {"b": 1}});
        set_dotted(&mut v, "a.c.d", serde_json::json!(2));
        set_dotted(&mut v, "x", serde_json::json!(9));
        assert_eq!(v["a"]["c"]["d"], serde_json::json!(2));
        assert_eq!(v["x"], serde_json::json!(9));
    }

    #[test]
    fn merge_replaces_scalars_and_merges_objects() {
        let mut acc = serde_json::json!({"server": {"port": 3000, "host": "x"}, "n": 1});
        merge_into(
            &mut acc,
            &serde_json::json!({"server": {"port": 4000}, "n": 2}),
        );
        assert_eq!(acc["server"]["port"], serde_json::json!(4000));
        assert_eq!(acc["server"]["host"], serde_json::json!("x"));
        assert_eq!(acc["n"], serde_json::json!(2));
    }
}
