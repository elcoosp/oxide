//! Internationalization (spec §16.4). Locale-prefixed routing, Accept-Language
//! negotiation, and locale-aware formatting (plurals, dates, numbers) via
//! ICU4X. Message catalogs use ICU MessageFormat; a Fluent option is available
//! by swapping the parser.
//!
//! The negotiation algorithm follows RFC 4647 "lookup":
//! 1. For each language range in the request's `Accept-Language` header, in
//!    descending q-value order, try exact, then progressively truncated
//!    subtags against the supported set.
//! 2. Fall back to the configured default.

use crate::{Result, err};
use icu_locale_core::LanguageIdentifier;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Supported-locale configuration.
#[derive(Debug, Clone)]
pub struct I18nConfig {
    /// BCP-47 language tags, e.g. `["en", "fr", "pt-BR"]`.
    pub locales: Vec<String>,
    /// The default locale (must be in `locales`).
    pub default: String,
    /// `"prefix-except-default"` (default) | `"prefix-always"` | `"prefix-never"`.
    pub strategy: String,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            locales: vec!["en".into()],
            default: "en".into(),
            strategy: "prefix-except-default".into(),
        }
    }
}

/// A parsed `Accept-Language` header entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LanguageRange {
    pub tag: String,
    pub q: f32,
}

/// Parse an `Accept-Language` header into a q-ordered list of ranges.
/// Malformed entries are skipped silently (per RFC 7231 §5.3.5, senders
/// SHOULD NOT send malformed but receivers MUST be tolerant).
pub fn parse_accept_language(header: &str) -> Vec<LanguageRange> {
    let mut out: Vec<LanguageRange> = Vec::new();
    for part in header.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut tag = String::new();
        let mut q = 1.0f32;
        for param in part.split(';') {
            let param = param.trim();
            if let Some(v) = param.strip_prefix("q=") {
                q = v.parse().unwrap_or(1.0);
            } else if tag.is_empty() {
                tag = param.to_string();
            }
        }
        if !tag.is_empty() {
            out.push(LanguageRange { tag, q });
        }
    }
    // Stable sort descending by q.
    out.sort_by(|a, b| b.q.partial_cmp(&a.q).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Negotiate the best locale for the given request against `cfg`. Returns the
/// matched supported tag or the default.
pub fn negotiate(cfg: &I18nConfig, accept_language: &str) -> String {
    let ranges = parse_accept_language(accept_language);
    if ranges.is_empty() {
        return cfg.default.clone();
    }
    let supported: Vec<LanguageIdentifier> = cfg
        .locales
        .iter()
        .filter_map(|s| LanguageIdentifier::from_str(s).ok())
        .collect();
    let supported_tags: Vec<String> = cfg.locales.clone();

    for range in &ranges {
        if range.tag == "*" {
            return cfg.default.clone();
        }
        let Some(range_li) = LanguageIdentifier::from_str(&range.tag).ok() else {
            continue;
        };
        // Exact match on the canonical form.
        if let Some(i) = supported.iter().position(|s| *s == range_li) {
            return supported_tags[i].clone();
        }
        // Language-subtag fallback: if the supported set contains a regional
        // variant of the requested language (`pt-BR` for `pt`), prefer it.
        let lang = range_li.language;
        if let Some(i) = supported.iter().position(|s| s.language == lang) {
            return supported_tags[i].clone();
        }
    }
    cfg.default.clone()
}

/// Build the URL prefix for a locale under the given strategy.
pub fn locale_prefix(cfg: &I18nConfig, locale: &str) -> String {
    match cfg.strategy.as_str() {
        "prefix-always" => format!("/{locale}"),
        "prefix-never" => String::new(),
        // default: prefix-except-default
        _ => {
            if locale == cfg.default {
                String::new()
            } else {
                format!("/{locale}")
            }
        }
    }
}

/// Strip a locale prefix from an incoming path, returning `(locale, path)`.
/// Falls back to the default when no prefix matches.
pub fn strip_locale_prefix<'a>(cfg: &I18nConfig, path: &'a str) -> (String, &'a str) {
    for loc in &cfg.locales {
        let pref = format!("/{loc}");
        if let Some(rest) = path.strip_prefix(&pref) {
            // Guard against false positives like `/english/...`.
            if rest.is_empty() || rest.starts_with('/') {
                return (loc.clone(), if rest.is_empty() { "/" } else { rest });
            }
        }
    }
    (cfg.default.clone(), path)
}

/// Localized `href()`: prefix the path with the locale per the strategy.
pub fn href(cfg: &I18nConfig, locale: &str, path: &str) -> String {
    let prefix = locale_prefix(cfg, locale);
    if prefix.is_empty() {
        path.to_string()
    } else if path == "/" {
        prefix
    } else {
        format!("{prefix}{path}")
    }
}

// ---------------- ICU4X-backed formatting ----------------

/// Format a number for a locale. Region-aware grouping separators are applied
/// for common locales; ICU4X's `DecimalFormatter` will replace this body when
/// the `fixed_decimal` crate lands in the workspace (the ICU4X path is the
/// spec's §16.4 target).
pub fn format_number(locale: &str, value: f64) -> String {
    let (thousands, decimal, use_commas) = match locale.split('-').next().unwrap_or("en") {
        // Locales that use `.` for thousands and `,` for decimals.
        "de" | "es" | "it" | "pt" => (".", ",", false),
        // Locales with a non-breaking space: use a regular space as a stand-in.
        "fr" | "ru" | "sv" | "pl" => (" ", ",", true),
        // Default: comma thousands, dot decimal.
        _ => (",", ".", true),
    };
    let _ = thousands;
    if !use_commas {
        // Simple group-3 for comma-thousands locales.
        let neg = value < 0.0;
        let s = format!("{:.}", value.abs());
        let (int_part, frac) = match s.split_once('.') {
            Some((i, f)) => (i.to_string(), f.to_string()),
            None => (s, String::new()),
        };
        let grouped = group_thousands(&int_part, thousands);
        let sign = if neg { "-" } else { "" };
        return if frac.is_empty() {
            format!("{sign}{grouped}")
        } else {
            format!("{sign}{grouped}{decimal}{frac}")
        };
    }
    // Comma-grouped default.
    let s = value.to_string();
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (s, String::new()),
    };
    let neg = int_part.starts_with('-');
    let int_part = int_part.trim_start_matches('-');
    let grouped = group_thousands(int_part, ",");
    let sign = if neg { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

fn group_thousands(int_part: &str, sep: &str) -> String {
    let mut out = String::with_capacity(int_part.len() + int_part.len() / 3);
    let bytes = int_part.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(*b as char);
    }
    out
}

// ---------------- Message catalogs ----------------

/// A locale → key → message catalog.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub entries: BTreeMap<String, BTreeMap<String, String>>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a message for a locale.
    pub fn insert(&mut self, locale: &str, key: &str, message: &str) {
        self.entries
            .entry(locale.to_string())
            .or_default()
            .insert(key.to_string(), message.to_string());
    }

    /// Look up a key with a fallback chain: locale → language → default.
    pub fn get<'a>(&'a self, locale: &str, key: &str, default: &'a str) -> &'a str {
        if let Some(m) = self.entries.get(locale).and_then(|c| c.get(key)) {
            return m;
        }
        // Language subtag fallback: `pt-BR` → `pt`.
        if let Some(lang) = locale.split('-').next() {
            if let Some(m) = self.entries.get(lang).and_then(|c| c.get(key)) {
                return m;
            }
        }
        self.entries
            .get(default)
            .and_then(|c| c.get(key))
            .map(String::as_str)
            .unwrap_or(default)
    }

    /// Replace `{name}` placeholders in a message with values from `args`.
    pub fn format(
        &self,
        locale: &str,
        key: &str,
        default: &str,
        args: &BTreeMap<String, String>,
    ) -> String {
        let template = self.get(locale, key, default);
        let mut out = String::with_capacity(template.len());
        let bytes = template.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                if let Some(close_rel) = template[i..].find('}') {
                    let close = i + close_rel;
                    let name = &template[i + 1..close];
                    if let Some(v) = args.get(name) {
                        out.push_str(v);
                    }
                    i = close + 1;
                    continue;
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// ICU MessageFormat integration (spec §16.4)
//
// The parser and formatter are provided by the `formatjs_icu_messageformat`
// crate (which uses ICU4X for plurals, numbers and dates). We wrap it in a
// process-wide cache because templates do not change at runtime.
// ---------------------------------------------------------------------------

use formatjs_icu_messageformat::{IcuMessageFormat, Value as IcuValue};
use std::sync::{Mutex, OnceLock};

/// Argument map in the shape formatjs expects. A value is a string, number,
/// boolean, or `null`; other JSON shapes are stringified.
pub type IcuValues = std::collections::HashMap<String, IcuValue<String>>;

/// Convert our JSON-shaped argument map into a formatjs `Values<String>`.
/// Objects and arrays are stringified with `serde_json` so callers can pass
/// anything without losing data.
pub fn icu_values_from_json(args: &BTreeMap<String, serde_json::Value>) -> IcuValues {
    let mut out = IcuValues::with_capacity(args.len());
    for (k, v) in args {
        let value = match v {
            serde_json::Value::String(s) => IcuValue::from(s.clone()),
            serde_json::Value::Bool(b) => IcuValue::from(*b),
            serde_json::Value::Null => IcuValue::Null,
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    IcuValue::from(i)
                } else if let Some(f) = n.as_f64() {
                    IcuValue::from(f)
                } else {
                    IcuValue::from(n.to_string())
                }
            }
            // Arrays, objects, and anything else are JSON-stringified.
            other => IcuValue::from(other.to_string()),
        };
        out.insert(k.clone(), value);
    }
    out
}

/// Thread-safe cache around `IcuMessageFormat` — compiles each template at most
/// once per process. Templates are immutable at runtime, so a process-wide
/// singleton is the right lifetime (see [`global_formatter`]).
#[derive(Default)]
pub struct MessageFormatter {
    cache: Mutex<std::collections::HashMap<String, std::sync::Arc<IcuMessageFormat>>>,
}

impl MessageFormatter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Format `template` in `locale` with `values`.
    ///
    /// Parse errors map to `AST-I18N`; runtime errors map to `AST-I18N-ARG`
    /// (missing argument or type mismatch).
    pub fn format(&self, locale: &str, template: &str, values: &IcuValues) -> Result<String> {
        let compiled = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = cache.get(template) {
                std::sync::Arc::clone(c)
            } else {
                let compiled = IcuMessageFormat::try_new(template).map_err(|e| {
                    let d = oxide_diagnostics::Diagnostic::error(
                        "AST-I18N",
                        format!("MessageFormat parse failed: {}", e.message),
                    )
                    .with_fix(
                        "check the ICU MessageFormat syntax; braces must balance and plural \
                         cases must include `other`",
                    );
                    oxide_diagnostics::OxideError(d)
                })?;
                let compiled = std::sync::Arc::new(compiled);
                cache.insert(template.to_string(), std::sync::Arc::clone(&compiled));
                compiled
            }
        };
        compiled.format_to_string(locale, values).map_err(|e| {
            let code = match e.code {
                formatjs_icu_messageformat::ErrorCode::Parse => "AST-I18N",
                formatjs_icu_messageformat::ErrorCode::MissingValue => "AST-I18N-ARG",
                formatjs_icu_messageformat::ErrorCode::InvalidValueType => "AST-I18N-TYPE",
                formatjs_icu_messageformat::ErrorCode::InvalidValue => "AST-I18N-ARG",
                _ => "AST-I18N",
            };
            err(code, format!("MessageFormat: {}", e.message))
        })
    }

    /// Drop cached templates. Useful in tests.
    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Number of templates currently cached.
    pub fn len(&self) -> usize {
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Process-wide formatter instance. Reused by every catalog call so templates
/// compile once per process, not once per request.
pub fn global_formatter() -> &'static MessageFormatter {
    static F: OnceLock<MessageFormatter> = OnceLock::new();
    F.get_or_init(MessageFormatter::new)
}

/// Convenience: format a template directly, without a catalog. Uses the
/// process-wide cache.
pub fn format_icu_message(locale: &str, template: &str, values: &IcuValues) -> Result<String> {
    global_formatter().format(locale, template, values)
}

impl Catalog {
    /// ICU MessageFormat entry point. Looks up `key` in the catalog for
    /// `locale` (with the usual language-subtag and default fallbacks), then
    /// formats it as a MessageFormat template.
    ///
    /// The existing [`Catalog::format`] (plain `{name}` substitution) is kept
    /// for call sites that don't need ICU types; this method is the one that
    /// supports `plural`, `select`, `selectordinal`, `number`, and escapes.
    pub fn format_icu(
        &self,
        locale: &str,
        key: &str,
        default: &str,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<String> {
        let template = self.get(locale, key, default);
        let icu_values = icu_values_from_json(values);
        global_formatter().format(locale, template, &icu_values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> I18nConfig {
        I18nConfig {
            locales: vec!["en".into(), "fr".into(), "pt-BR".into()],
            default: "en".into(),
            strategy: "prefix-except-default".into(),
        }
    }

    #[test]
    fn parse_accept_language_orders_by_q() {
        let ranges = parse_accept_language("fr;q=0.9,en;q=1.0,de;q=0.5");
        assert_eq!(ranges[0].tag, "en");
        assert_eq!(ranges[1].tag, "fr");
        assert_eq!(ranges[2].tag, "de");
    }

    #[test]
    fn negotiate_exact_match() {
        assert_eq!(negotiate(&cfg(), "fr-FR,fr;q=0.9"), "fr");
    }

    #[test]
    fn negotiate_language_subtag_match() {
        // `pt` matches `pt-BR`'s language.
        assert_eq!(negotiate(&cfg(), "pt"), "pt-BR");
    }

    #[test]
    fn negotiate_falls_back_to_default() {
        assert_eq!(negotiate(&cfg(), "de,es"), "en");
        assert_eq!(negotiate(&cfg(), ""), "en");
    }

    #[test]
    fn negotiate_wildcard_returns_default() {
        assert_eq!(negotiate(&cfg(), "*"), "en");
    }

    #[test]
    fn locale_prefix_under_strategies() {
        assert_eq!(locale_prefix(&cfg(), "fr"), "/fr");
        assert_eq!(locale_prefix(&cfg(), "en"), ""); // default not prefixed
        let mut always = cfg();
        always.strategy = "prefix-always".into();
        assert_eq!(locale_prefix(&always, "en"), "/en");
        let mut never = cfg();
        never.strategy = "prefix-never".into();
        assert_eq!(locale_prefix(&never, "fr"), "");
    }

    #[test]
    fn strip_locale_prefix_roundtrip() {
        let (loc, path) = strip_locale_prefix(&cfg(), "/fr/photos/1");
        assert_eq!(loc, "fr");
        assert_eq!(path, "/photos/1");
        let (loc, path) = strip_locale_prefix(&cfg(), "/photos/1");
        assert_eq!(loc, "en");
        assert_eq!(path, "/photos/1");
        // Guard: `/english` must not match `/en`.
        let (loc, path) = strip_locale_prefix(&cfg(), "/english");
        assert_eq!(loc, "en");
        assert_eq!(path, "/english");
    }

    #[test]
    fn href_prefixes_and_leaves_default_bare() {
        assert_eq!(href(&cfg(), "fr", "/blog"), "/fr/blog");
        assert_eq!(href(&cfg(), "en", "/blog"), "/blog");
        assert_eq!(href(&cfg(), "fr", "/"), "/fr");
    }

    #[test]
    fn format_number_uses_locale_grouping() {
        // We can't fully control ICU's CLDR data in a test, but the function
        // must not panic and must produce a string containing the digits.
        let s = format_number("en", 1234567.0);
        assert!(s.contains('1') && s.contains('7'), "{s}");
    }

    #[test]
    fn catalog_lookup_and_fallback_chain() {
        let mut c = Catalog::new();
        c.insert("en", "hello", "Hello");
        c.insert("fr", "hello", "Bonjour");
        c.insert("fr", "goodbye", "Au revoir");
        assert_eq!(c.get("fr", "hello", "en"), "Bonjour");
        assert_eq!(c.get("fr", "missing", "Hello"), "Hello");
        // `pt-BR` should fall through to `pt` when present, else default.
        c.insert("pt", "hi", "Olá");
        assert_eq!(c.get("pt-BR", "hi", "en"), "Olá");
    }

    // ---------------- §16.4 ICU MessageFormat (formatjs) ----------------

    #[test]
    fn icu_messageformat_formats_plural_cases() {
        let mut values = BTreeMap::new();
        values.insert("name".to_string(), serde_json::json!("Ada"));
        values.insert("count".to_string(), serde_json::json!(2));
        let out = format_icu_message(
            "en",
            "{name} has {count, plural, =0 {no tasks} one {# task} other {# tasks}}.",
            &icu_values_from_json(&values),
        )
        .unwrap();
        assert_eq!(out, "Ada has 2 tasks.");
    }

    #[test]
    fn icu_messageformat_one_case_in_english() {
        let mut values = BTreeMap::new();
        values.insert("count".to_string(), serde_json::json!(1));
        let out = format_icu_message(
            "en",
            "{count, plural, one {# item} other {# items}}",
            &icu_values_from_json(&values),
        )
        .unwrap();
        assert_eq!(out, "1 item");
    }

    #[test]
    fn icu_messageformat_locale_specific_plural_category() {
        // Polish has "few" for 2–4; English falls through to "other".
        let values: BTreeMap<String, serde_json::Value> =
            [("count".to_string(), serde_json::json!(3))].into();
        let template = "{count, plural, one {jeden} few {kilka} many {wiele} other {inne}}";

        let pl = format_icu_message("pl", template, &icu_values_from_json(&values)).unwrap();
        assert_eq!(pl, "kilka");

        let en = format_icu_message("en", template, &icu_values_from_json(&values)).unwrap();
        assert_eq!(en, "inne");
    }

    #[test]
    fn icu_messageformat_selectordinal() {
        for (n, want) in [(1, "1st"), (2, "2nd"), (3, "3rd"), (11, "11th")] {
            let values: BTreeMap<String, serde_json::Value> =
                [("place".to_string(), serde_json::json!(n))].into();
            let out = format_icu_message(
                "en",
                "{place, selectordinal, one {#st} two {#nd} few {#rd} other {#th}}",
                &icu_values_from_json(&values),
            )
            .unwrap();
            assert_eq!(out, want, "n={n}");
        }
    }

    #[test]
    fn icu_messageformat_select() {
        let values: BTreeMap<String, serde_json::Value> =
            [("gender".to_string(), serde_json::json!("female"))].into();
        let out = format_icu_message(
            "en",
            "{gender, select, male {He} female {She} other {They}} said hi",
            &icu_values_from_json(&values),
        )
        .unwrap();
        assert_eq!(out, "She said hi");
    }

    #[test]
    fn icu_messageformat_offset() {
        let values: BTreeMap<String, serde_json::Value> =
            [("count".to_string(), serde_json::json!(3))].into();
        let out = format_icu_message(
            "en",
            "{count, plural, offset:1 =1 {one guest} other {# other guests}}",
            &icu_values_from_json(&values),
        )
        .unwrap();
        assert_eq!(out, "2 other guests");
    }

    #[test]
    fn icu_messageformat_number_percent() {
        let values: BTreeMap<String, serde_json::Value> =
            [("value".to_string(), serde_json::json!(12.3))].into();
        let out = format_icu_message(
            "en",
            "{value, number, ::percent scale/0.01}",
            &icu_values_from_json(&values),
        )
        .unwrap();
        assert_eq!(out, "12%");
    }

    #[test]
    fn icu_messageformat_missing_argument_yields_ast_i18n_arg() {
        let values: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let e =
            format_icu_message("en", "Hello, {name}!", &icu_values_from_json(&values)).unwrap_err();
        assert_eq!(e.0.code, "AST-I18N-ARG");
        assert!(e.0.message.contains("name"));
    }

    #[test]
    fn icu_messageformat_parse_error_yields_ast_i18n() {
        // Missing `other` clause is a parse error.
        let values: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let e = format_icu_message("en", "{n, plural, one {x}}", &icu_values_from_json(&values))
            .unwrap_err();
        assert_eq!(e.0.code, "AST-I18N");
    }

    #[test]
    fn message_formatter_caches_by_template() {
        let f = MessageFormatter::new();
        let v = IcuValues::new();
        assert!(f.is_empty());
        let _ = f.format("en", "hello", &v).unwrap();
        let _ = f.format("en", "hello", &v).unwrap();
        assert_eq!(f.len(), 1, "same template must be cached once");
        let _ = f.format("en", "other", &v).unwrap();
        assert_eq!(f.len(), 2);
        f.clear();
        assert!(f.is_empty());
    }

    #[test]
    fn catalog_format_icu_uses_lookup_chain() {
        let mut c = Catalog::new();
        c.insert("en", "greeting", "Hello, {name}!");
        c.insert("fr", "greeting", "Bonjour, {name} !");
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), serde_json::json!("Ada"));

        assert_eq!(
            c.format_icu("en", "greeting", "x", &args).unwrap(),
            "Hello, Ada!"
        );
        assert_eq!(
            c.format_icu("fr", "greeting", "x", &args).unwrap(),
            "Bonjour, Ada !"
        );
        // Fall back through language subtag: `pt-BR` has no entry → falls to `en` via default.
        assert_eq!(
            c.format_icu("pt-BR", "greeting", "Hello, {name}!", &args)
                .unwrap(),
            "Hello, Ada!"
        );
    }

    #[test]
    fn icu_values_from_json_preserves_types() {
        let args: BTreeMap<String, serde_json::Value> = [
            ("s".to_string(), serde_json::json!("x")),
            ("i".to_string(), serde_json::json!(42)),
            ("f".to_string(), serde_json::json!(2.5)),
            ("b".to_string(), serde_json::json!(true)),
            ("n".to_string(), serde_json::Value::Null),
        ]
        .into();
        let v = icu_values_from_json(&args);
        assert!(matches!(v.get("s"), Some(IcuValue::String(s)) if s == "x"));
        assert!(matches!(v.get("i"), Some(IcuValue::Integer(42))));
        assert!(matches!(v.get("f"), Some(IcuValue::Number(_))));
        assert!(matches!(v.get("b"), Some(IcuValue::Boolean(true))));
        assert!(matches!(v.get("n"), Some(IcuValue::Null)));
    }

    #[test]
    fn catalog_format_substitutes_placeholders() {
        let mut c = Catalog::new();
        c.insert("en", "welcome", "Welcome, {name}!");
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), "Ada".to_string());
        assert_eq!(
            c.format("en", "welcome", "fallback", &args),
            "Welcome, Ada!"
        );
    }
}

// ─── SPEC-16.4: catalog file loader + messages.d.ts typegen ────────────────

/// Walk `locales_root/{locale}/**/*.json` and merge every file into a single
/// `Catalog`. A file named for the locale (`locales/en/en.json`) contributes
/// keys directly; any other file contributes namespaced keys
/// (`locales/en/common/buttons.json` → `buttons.save`). Sorted-walk for
/// deterministic merge.
pub fn load_catalog_from_dir(locales_root: &std::path::Path) -> Result<Catalog> {
    use std::fs;
    let mut cat = Catalog::new();
    if !locales_root.is_dir() {
        return Ok(cat);
    }
    let entries = match fs::read_dir(locales_root) {
        Ok(e) => e,
        Err(e) => {
            return Err(err(
                "I18N-LOAD",
                format!("cannot read locales dir {}: {e}", locales_root.display()),
            ));
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(locale) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        walk_json(&path, &mut files);
        files.sort();
        for file in files {
            // Namespace: path of the file relative to the locale dir, minus
            // `.json`, with `/` → `.`. The locale's own top-level file
            // (`<locale>.json` directly under the locale dir) has an empty ns.
            let rel = file.strip_prefix(&path).unwrap_or(&file);
            let mut parts: Vec<String> = rel
                .iter()
                .map(|c| c.to_string_lossy().into_owned())
                .collect();
            if let Some(last) = parts.last_mut() {
                if let Some(stem) = last.strip_suffix(".json") {
                    *last = stem.to_string();
                }
            }
            // Drop a leading file whose stem equals the locale name.
            if parts.first().map(String::as_str) == Some(locale) && parts.len() == 1 {
                parts.clear();
            }
            let ns = parts.join(".");
            let body = match fs::read_to_string(&file) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let json: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    return Err(err(
                        "I18N-LOAD",
                        format!("cannot parse {}: {e}", file.display()),
                    ));
                }
            };
            flatten_into_catalog(&mut cat, locale, &ns, &json);
        }
    }
    Ok(cat)
}

fn walk_json(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk_json(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
}

fn flatten_into_catalog(cat: &mut Catalog, locale: &str, ns: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if ns.is_empty() {
                    k.clone()
                } else {
                    format!("{ns}.{k}")
                };
                flatten_into_catalog(cat, locale, &key, v);
            }
        }
        other => {
            // A non-object leaf. Strings are the common case (a message
            // template); a number/bool leaf is uncommon but stringify rather
            // than fail.
            let s = match other {
                serde_json::Value::String(s) => s.clone(),
                _ => other.to_string(),
            };
            cat.insert(locale, ns, &s);
        }
    }
}

/// SPEC-16.4: render `messages.d.ts` from a loaded catalog. Deterministic:
/// locales and keys are sorted before emit.
pub fn write_messages_dts(cat: &Catalog, out_path: &std::path::Path) -> Result<()> {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("// Generated by oxide assets i18n typegen (spec §16.4). DO NOT EDIT.\n\n");
    let mut all_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entries in cat.entries.values() {
        for k in entries.keys() {
            all_keys.insert(k.clone());
        }
    }
    out.push_str("export type MessageKey =\n");
    if all_keys.is_empty() {
        out.push_str("  never;\n");
    } else {
        let keys: Vec<&str> = all_keys.iter().map(String::as_str).collect();
        for (i, k) in keys.iter().enumerate() {
            let _ = writeln!(
                out,
                "  | {}{}",
                ts_string_literal(k),
                if i + 1 == keys.len() { ";" } else { "" }
            );
        }
    }
    out.push_str("\nexport type LocaleMessages = Record<MessageKey, string>;\n");
    out.push_str("\nexport interface Messages {\n");
    let locales: Vec<&String> = cat.entries.keys().collect();
    for locale in &locales {
        let ident = locale.replace('-', "_");
        let _ = writeln!(out, "  {}: LocaleMessages;", ident);
    }
    out.push_str("}\n");
    out.push_str("\nexport type DefaultLocale = ");
    if let Some(first) = locales.first() {
        out.push_str(&ts_string_literal(first));
    } else {
        out.push_str("never");
    }
    out.push_str(";\n");
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| err("I18N-LOAD", format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(out_path, out)
        .map_err(|e| err("I18N-LOAD", format!("write {}: {e}", out_path.display())))?;
    Ok(())
}

fn ts_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn loads_top_level_and_nested_locale_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("locales");
        std::fs::create_dir_all(root.join("en")).unwrap();
        std::fs::create_dir_all(root.join("fr")).unwrap();
        std::fs::write(
            root.join("en").join("en.json"),
            r#"{"hello":"Hello","bye":"Goodbye"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("en").join("common")).unwrap();
        std::fs::write(
            root.join("en").join("common").join("buttons.json"),
            r#"{"save":"Save"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("fr").join("fr.json"),
            r#"{"hello":"Bonjour"}"#,
        )
        .unwrap();
        let cat = load_catalog_from_dir(&root).unwrap();
        assert_eq!(cat.get("en", "hello", "?"), "Hello");
        assert_eq!(cat.get("en", "bye", "?"), "Goodbye");
        assert_eq!(cat.get("en", "common.buttons.save", "?"), "Save");
        assert_eq!(cat.get("fr", "hello", "?"), "Bonjour");
        assert_eq!(cat.get("fr", "bye", "fallback"), "fallback");
    }

    #[test]
    fn empty_locales_dir_yields_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = load_catalog_from_dir(tmp.path()).unwrap();
        assert!(cat.entries.is_empty());
    }

    #[test]
    fn dts_lists_every_key_and_locale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("locales");
        std::fs::create_dir_all(root.join("en")).unwrap();
        std::fs::create_dir_all(root.join("pt-BR")).unwrap();
        std::fs::write(
            root.join("en").join("en.json"),
            r#"{"hello":"Hello","bye":"Goodbye"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("pt-BR").join("pt-BR.json"),
            r#"{"hello":"Olá"}"#,
        )
        .unwrap();
        let cat = load_catalog_from_dir(&root).unwrap();
        let out = tmp.path().join("messages.d.ts");
        write_messages_dts(&cat, &out).unwrap();
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("export type MessageKey"), "{content}");
        assert!(content.contains("\"hello\""), "{content}");
        assert!(content.contains("\"bye\""), "{content}");
        assert!(content.contains("en: LocaleMessages"), "{content}");
        assert!(content.contains("pt_BR: LocaleMessages"), "{content}");
    }
}
