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

use crate::{OxideConfig, Result, err, extract_static_config, parse_env_file, parse_size};
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
    "experiments",
    "live",
    "stream",
    "brownout",
    "search",
    "interactionReplay",
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
        if let Some(v) = extract_static_config(&src) {
            return Ok(Some(Layer { value: v, path: ts }));
        }
        *used_dynamic_eval = true;
        if json.is_file() {
            return Ok(Some(Layer {
                value: read_json(&json)?,
                path: json,
            }));
        }
        return Ok(None); // dynamic TS, nothing static to merge
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
        let mut value = if path.extension().is_some_and(|e| e == "ts") {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| err("CFG-IO", format!("read {}: {e}", path.display())))?;
            if let Some(v) = extract_static_config(&src) {
                v
            } else {
                *used_dynamic_eval = true;
                return Err(err(
                    "CFG-EXTENDS",
                    format!(
                        "preset `{}` could not be statically extracted",
                        path.display()
                    ),
                ));
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
                .map_or_else(|| dir.to_path_buf(), Path::to_path_buf);
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
        .is_none_or(Value::is_null);
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
    // V2-F04 (§15.4): validate every experiment entry before the typed
    // parse — a bad `traffic` percentage or a too-few-variants entry is a
    // config authoring bug and gets the DSP-110 diagnostic at load time.
    if let Some(Value::Array(entries)) = map.get("experiments") {
        for entry in entries {
            validate_experiment(entry)?;
        }
    }
    // V2-F05: the live knobs carry their own load-time bounds (DSP-111).
    if let Some(v) = map.get("live") {
        validate_live(v)?;
    }
    // V2-F07: the stream-resume knobs carry their own load-time bounds
    // (DSP-112) and the size literals are the §14 grammar (CFG-SIZE).
    if let Some(v) = map.get("stream") {
        validate_stream(v)?;
    }
    // V2-F12: the brownout knobs carry their own load-time bounds
    // (DSP-113) — ascending ladders, a stale-ceiling multiplier ≥ 1 and a
    // hedger factor > 1. A rule's `priority` rides the same diagnostic.
    if let Some(v) = map.get("brownout") {
        validate_brownout(v)?;
    }
    // V2-F03: the interaction-replay opt-out is a boolean (DSP-114). The
    // raw-`Value` pass runs before the typed parse, so a quoted `"false"`
    // (the TS-import shape) or a `0`/`1` gets the DSP diagnostic — naming
    // the key and carrying the fix hint — instead of a bare serde type
    // error at the typed boundary.
    if let Some(v) = map.get("interactionReplay") {
        validate_interaction_replay(v)?;
    }
    // V2-E04: the security surface's new enum spellings get the same
    // load-time treatment — a bad `csp`/`trustedTypes` value is an authoring
    // bug and gets the CFG-SEC diagnostic naming the valid modes, not a
    // serde type error.
    if let Some(v) = map.get("security") {
        validate_security(v)?;
    }
    if let Some(Value::Array(rules)) = map.get("rules") {
        for rule in rules {
            match rule.get("priority") {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) if matches!(s.as_str(), "low" | "normal" | "critical") => {}
                _ => {
                    return Err(dsp_113(
                        "`rules[].priority` must be one of `\"low\"`, `\"normal\"`, \
                         `\"critical\"` — the brownout enforcement sheds `low` routes at \
                         level Shed and everything but `critical` at Emergency",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// V2-F04 (§15.4): one `experiments` entry must declare a `traffic`
/// percentage in 0–100 and at least two variants — the `DSP-110`
/// diagnostic.
fn validate_experiment(value: &Value) -> Result<()> {
    let Value::Object(entry) = value else {
        return Err(dsp_110("each `experiments` entry must be an object"));
    };
    let traffic_ok = match entry.get("traffic") {
        Some(Value::Number(n)) => n.as_u64().is_some_and(|t| t <= 100),
        _ => false,
    };
    if !traffic_ok {
        return Err(dsp_110(
            "`experiments[].traffic` must be an integer percentage between 0 and 100",
        ));
    }
    let variants_ok = entry
        .get("variants")
        .and_then(Value::as_array)
        .is_some_and(|v| v.len() >= 2);
    if !variants_ok {
        return Err(dsp_110(
            "`experiments[].variants` must list at least two variant names",
        ));
    }
    Ok(())
}

/// The `DSP-110` diagnostic for an invalid `experiments` entry (§V2-F04).
fn dsp_110(msg: &str) -> OxideError {
    let mut d = Diagnostic::error("DSP-110", msg);
    d.fix = Some(
        "fix the `experiments` entry — `traffic` is a 0–100 percentage and an \
         experiment needs at least two variants"
            .into(),
    );
    OxideError(Box::new(d))
}

/// V2-F05 (§V2-F05 step 8): the `live` knobs must be physically usable —
/// a 0 s keepalive would emit a `ping` per tick (busy-looping every open
/// connection), a 0-subscriber hub rejects everyone, and a per-IP cap
/// above the global cap is dead config. Violations fail the load with the
/// `DSP-111` diagnostic (the DSP-110 pattern, next free code).
fn validate_live(value: &Value) -> Result<()> {
    let Value::Object(map) = value else {
        return Err(dsp_111("`live` must be an object"));
    };
    let knob = |key: &str| -> Result<Option<u64>> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => n
                .as_u64()
                .map(Some)
                .ok_or_else(|| dsp_111(&format!("`live.{key}` must be a non-negative integer"))),
            Some(_) => Err(dsp_111(&format!(
                "`live.{key}` must be a non-negative integer"
            ))),
        }
    };
    if knob("maxSubscribers")? == Some(0) {
        return Err(dsp_111(
            "`live.maxSubscribers` must be at least 1 — the hub must be able to hold a subscriber",
        ));
    }
    if knob("keepaliveSeconds")? == Some(0) {
        return Err(dsp_111(
            "`live.keepaliveSeconds` must be at least 1 — a 0s keepalive busy-loops every connection",
        ));
    }
    if knob("maxRerunsPerSecond")? == Some(0) {
        return Err(dsp_111(
            "`live.maxRerunsPerSecond` must be at least 1 — a 0 rate cap drops every rerun",
        ));
    }
    let max = knob("maxSubscribers")?.unwrap_or(5_000);
    if knob("perIpMax")?.is_some_and(|per_ip| per_ip > max) {
        return Err(dsp_111(
            "`live.perIpMax` cannot exceed `live.maxSubscribers` — the per-peer cap is a \
             fraction of the global capacity",
        ));
    }
    if let Some(transport) = map.get("transport") {
        match transport {
            Value::String(s) if s == "sse" || s == "poll" => {}
            _ => {
                return Err(dsp_111("`live.transport` must be `\"sse\"` or `\"poll\"`"));
            }
        }
    }
    Ok(())
}

/// The `DSP-111` diagnostic for an invalid `live` section (§V2-F05).
fn dsp_111(msg: &str) -> OxideError {
    let mut d = Diagnostic::error("DSP-111", msg);
    d.fix = Some(
        "fix the `live` section — `keepaliveSeconds` >= 1, `maxSubscribers` >= 1, \
         `maxRerunsPerSecond` >= 1 and `perIpMax` <= `maxSubscribers`"
            .into(),
    );
    OxideError(Box::new(d))
}

/// V2-F07 (§V2-F07 wiring step 3): the `stream.resume` knobs must be
/// physically usable — a 0 s ttl expires every buffer before the reattach
/// arrives, a 0-byte per-stream bound disables replay entirely, and a
/// total bound below the per-stream bound would evict the first buffer
/// before it ever replays. The size literals are the §14 grammar
/// (`CFG-SIZE`). Violations fail the load with the `DSP-112` diagnostic
/// (the DSP-110/111 pattern, next free code).
fn validate_stream(value: &Value) -> Result<()> {
    let Value::Object(map) = value else {
        return Err(dsp_112("`stream` must be an object"));
    };
    // An absent (or null) `resume` section is the typed defaults — valid.
    let Some(Value::Object(resume)) = map.get("resume") else {
        return Ok(());
    };
    match resume.get("ttlSecs") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_u64().is_some_and(|t| t >= 1) => {}
        Some(_) => {
            return Err(dsp_112(
                "`stream.resume.ttlSecs` must be an integer >= 1 — a 0s ttl expires every \
                 buffer before the reattach arrives",
            ));
        }
    }
    let size = |key: &str| -> Result<u64> {
        match resume.get(key) {
            None | Some(Value::Null) => Ok(0), // absent: the typed default applies
            Some(Value::String(s)) => parse_size(s),
            Some(_) => Err(dsp_112(&format!(
                "`stream.resume.{key}` must be a size literal (e.g. `\"4mb\"`)"
            ))),
        }
    };
    let each = size("maxBytesEach")?;
    if resume.get("maxBytesEach").is_some() && each == 0 {
        return Err(dsp_112(
            "`stream.resume.maxBytesEach` must be at least 1 byte — a 0-byte bound disables \
             replay entirely",
        ));
    }
    let total = size("maxBytesTotal")?;
    if each > 0 && total > 0 && total < each {
        return Err(dsp_112(
            "`stream.resume.maxBytesTotal` cannot be below `stream.resume.maxBytesEach` — \
             the total bound is the sum of the per-stream bounds",
        ));
    }
    Ok(())
}

/// The `DSP-112` diagnostic for an invalid `stream.resume` section (§V2-F07).
fn dsp_112(msg: &str) -> OxideError {
    let mut d = Diagnostic::error("DSP-112", msg);
    d.fix = Some(
        "fix the `stream.resume` section — `ttlSecs` >= 1, `maxBytesEach` >= 1 byte and \
         `maxBytesTotal` >= `maxBytesEach`"
            .into(),
    );
    OxideError(Box::new(d))
}

/// V2-F12: the `brownout` knobs must be physically usable — each threshold
/// ladder ascending (a higher rung below a lower one would de-escalate into
/// a breach), the `staleCeiling` multiplier parseable and ≥ 1 (a 0× ceiling
/// would serve entries infinitely past their TTL), and the hedger's
/// `afterFactor` > 1 (a factor of 1 would hedge every render). Violations
/// fail the load with the `DSP-113` diagnostic (the DSP-110/111/112
/// pattern, next free code).
fn validate_brownout(value: &Value) -> Result<()> {
    let Value::Object(map) = value else {
        return Err(dsp_113("`brownout` must be an object"));
    };
    match map.get("enabled") {
        None | Some(Value::Null | Value::Bool(_)) => {}
        Some(_) => return Err(dsp_113("`brownout.enabled` must be a boolean")),
    }
    if let Some(c) = map.get("staleCeiling") {
        match c {
            Value::Null => {}
            Value::String(s) if crate::parse_stale_ceiling(s).is_some() => {}
            _ => {
                return Err(dsp_113(
                    "`brownout.staleCeiling` must be a multiplier of at least 1, e.g. `\"10x\"` \
                     or `\"10\"` — a 0× ceiling would serve entries infinitely past their TTL",
                ));
            }
        }
    }
    if let Some(Value::Object(thresholds)) = map.get("thresholds") {
        for (key, value) in thresholds {
            let Value::Array(rungs) = value else {
                return Err(dsp_113(&format!(
                    "`brownout.thresholds.{key}` must be an array of three numbers"
                )));
            };
            if rungs.len() != 3 {
                return Err(dsp_113(&format!(
                    "`brownout.thresholds.{key}` must hold exactly three rungs \
                     (levels Stale/Shed/Emergency), got {}",
                    rungs.len()
                )));
            }
            let mut prev = None;
            for (i, r) in rungs.iter().enumerate() {
                let Some(n) = r.as_u64() else {
                    return Err(dsp_113(&format!(
                        "`brownout.thresholds.{key}[{i}]` must be a non-negative number"
                    )));
                };
                if prev.is_some_and(|p: u64| n < p) {
                    return Err(dsp_113(&format!(
                        "`brownout.thresholds.{key}` must be ascending — a rung below the \
                         previous one would de-escalate into a breach"
                    )));
                }
                prev = Some(n);
            }
        }
    }
    if let Some(Value::Object(hedge)) = map.get("hedge") {
        match hedge.get("enabled") {
            None | Some(Value::Null | Value::Bool(_)) => {}
            Some(_) => return Err(dsp_113("`brownout.hedge.enabled` must be a boolean")),
        }
        match hedge.get("afterFactor") {
            None | Some(Value::Null) => {}
            Some(Value::Number(n)) if n.as_f64().is_some_and(|f| f > 1.0) => {}
            _ => {
                return Err(dsp_113(
                    "`brownout.hedge.afterFactor` must be a number greater than 1 — a factor \
                     of 1 (or less) would hedge every render",
                ));
            }
        }
    }
    Ok(())
}

/// The `DSP-113` diagnostic for an invalid `brownout` section (§V2-F12).
fn dsp_113(msg: &str) -> OxideError {
    let mut d = Diagnostic::error("DSP-113", msg);
    d.fix = Some(
        "fix the `brownout` section — ascending threshold ladders, `staleCeiling` >= 1 \
         (e.g. `\"10x\"`) and `hedge.afterFactor` > 1"
            .into(),
    );
    OxideError(Box::new(d))
}

/// V2-F03: `interactionReplay` must be a boolean — the `DSP-114`
/// diagnostic (the DSP-110/111/112/113 pattern, next free code).
fn validate_interaction_replay(value: &Value) -> Result<()> {
    if !value.is_boolean() {
        return Err(dsp_114(&format!(
            "`interactionReplay` must be a boolean (true or false) — got {value}. The flag \
             opts the page out of the pre-hydration interaction replay; quoting it \
             (\"false\") or passing a number (0/1) does not opt out"
        )));
    }
    Ok(())
}

/// The `DSP-114` diagnostic for a non-boolean `interactionReplay` (§V2-F03).
fn dsp_114(msg: &str) -> OxideError {
    let mut d = Diagnostic::error("DSP-114", msg);
    d.fix = Some(
        "set `interactionReplay` to the unquoted boolean `true` or `false` — \
         `false` skips the pre-hydration interaction capture"
            .into(),
    );
    OxideError(Box::new(d))
}

/// V2-E04: the `security.csp` / `security.trustedTypes` spellings are
/// validated at load time (the knob-validator pattern): a bad value is a
/// config authoring bug and gets the `CFG-SEC` diagnostic — the message
/// names the key and the valid spellings, the fix restates them — instead
/// of a bare serde type error at the typed boundary.
fn validate_security(value: &Value) -> Result<()> {
    let Value::Object(map) = value else {
        return Ok(()); // not an object — the typed parse reports the shape
    };
    if let Some(csp) = map.get("csp").and_then(Value::as_str)
        && !matches!(csp, "strict" | "hash" | "off" | "report-only")
    {
        return Err(cfg_sec(
            "csp",
            csp,
            "`strict`, `hash`, `report-only`, `off`",
        ));
    }
    if let Some(tt) = map.get("trustedTypes").and_then(Value::as_str)
        && !matches!(tt, "off" | "report" | "enforce")
    {
        return Err(cfg_sec("trustedTypes", tt, "`off`, `report`, `enforce`"));
    }
    Ok(())
}

/// The `CFG-SEC` diagnostic for a bad `security.*` enum spelling (V2-E04).
fn cfg_sec(key: &str, got: &str, valid: &str) -> OxideError {
    let mut d = Diagnostic::error(
        "CFG-SEC",
        format!("`security.{key}` must be one of {valid} — got `{got}`"),
    );
    d.fix = Some(format!("set `security.{key}` to one of {valid}"));
    OxideError(Box::new(d))
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
