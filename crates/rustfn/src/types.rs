//! The TypeScript declaration side of V2-F08: the `#[derive(TsType)]`
//! contract ("writes declarations to `.oxide/types/rust-fns.d.ts`, so
//! `thumbnail(input)` is typed in TSX, Vue, Svelte and Solid") — the
//! derive generates [`TsType`] impls, the `#[server_fn]` macro
//! collects them into a [`FnTypeEntry`] per function, the
//! `export_table!` macro serializes the entries into the optional
//! `oxide_fn_types_json` library export and the host renders the
//! `.d.ts` from it ([`render_types_dts`] / [`write_types`]).

use std::collections::BTreeMap;
use std::path::Path;

use oxide_fw_diagnostics::OxideError;
use serde::{Deserialize, Serialize};

use crate::err;

/// The inline annotation a type contributes where it appears (`string`,
/// `number`, `ThumbIn`, `Array<T>`, `T | null`, …).
pub trait TsType {
    /// The inline TypeScript annotation for this type.
    #[must_use]
    fn ts_type_name() -> String
    where
        Self: Sized;
    /// The declaration blocks this type contributes (itself plus its
    /// nested types), as `(type name, block)` pairs — the collector
    /// deduplicates by name.
    #[must_use]
    fn ts_decls() -> Vec<(String, String)>
    where
        Self: Sized;
}

// The primitives (the derive's syntactic mapping matches these, so a
// field type never needs a hand impl to pass through).

macro_rules! ts_number {
    ($($t:ty),+ $(,)?) => {
        $(impl TsType for $t {
            fn ts_type_name() -> String { "number".to_string() }
            fn ts_decls() -> Vec<(String, String)> { Vec::new() }
        })+
    };
}

ts_number!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
);

impl TsType for bool {
    fn ts_type_name() -> String {
        "boolean".to_string()
    }
    fn ts_decls() -> Vec<(String, String)> {
        Vec::new()
    }
}

impl TsType for char {
    fn ts_type_name() -> String {
        "string".to_string()
    }
    fn ts_decls() -> Vec<(String, String)> {
        Vec::new()
    }
}

impl TsType for String {
    fn ts_type_name() -> String {
        "string".to_string()
    }
    fn ts_decls() -> Vec<(String, String)> {
        Vec::new()
    }
}

impl TsType for std::path::PathBuf {
    fn ts_type_name() -> String {
        "string".to_string()
    }
    fn ts_decls() -> Vec<(String, String)> {
        Vec::new()
    }
}

impl TsType for serde_json::Value {
    fn ts_type_name() -> String {
        "unknown".to_string()
    }
    fn ts_decls() -> Vec<(String, String)> {
        Vec::new()
    }
}

impl<T: TsType> TsType for Vec<T> {
    fn ts_type_name() -> String {
        format!("Array<{}>", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

impl<T: TsType> TsType for Option<T> {
    fn ts_type_name() -> String {
        format!("{} | null", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

impl<T: TsType, S: std::hash::BuildHasher> TsType for std::collections::HashSet<T, S> {
    fn ts_type_name() -> String {
        format!("Array<{}>", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

impl<T: TsType> TsType for std::collections::BTreeSet<T> {
    fn ts_type_name() -> String {
        format!("Array<{}>", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

impl<T: TsType, S: std::hash::BuildHasher> TsType for std::collections::HashMap<String, T, S> {
    fn ts_type_name() -> String {
        format!("Record<string, {}>", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

impl<T: TsType> TsType for std::collections::BTreeMap<String, T> {
    fn ts_type_name() -> String {
        format!("Record<string, {}>", T::ts_type_name())
    }
    fn ts_decls() -> Vec<(String, String)> {
        T::ts_decls()
    }
}

/// One function's type entry (what `__oxide_types_<name>` produces and
/// what the wire JSON carries per fn).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnTypeEntry {
    /// The export name (the entry name the host indexes).
    pub name: String,
    /// The source file the `#[server_fn]` macro recorded (`file!()`
    /// at the user crate's compile time) — the §9.4 id triple's file
    /// half.
    pub file: String,
    /// The inline annotations of the input types (empty for a
    /// zero-argument fn).
    pub args: Vec<String>,
    /// The inline annotation of the output type.
    pub ret: String,
    /// The declaration blocks the input and output types contribute.
    #[serde(default)]
    pub decls: Vec<(String, String)>,
}

/// The wire shape `oxide_fn_types_json` carries (the fns plus the
/// deduplicated declaration registry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypesPayload {
    /// One record per `#[server_fn]` export.
    pub fns: Vec<FnTypeEntry>,
    /// The type-name → declaration-block registry (sorted, deduped).
    pub decls: BTreeMap<String, String>,
}

/// Serialize the entries into the JSON the library exports. Duplicate
/// declaration names collapse to the first contribution.
///
/// The `String` return is the export contract (`export_table!` emits the
/// call inside the generated side export), so a serialize failure cannot
/// become a coded refusal — but it is never silent: the empty payload a
/// failure would write registers ZERO native fns at boot (the consult's
/// id reconstruction walks the payload), which is exactly the
/// silently-empty-write shape the silent-fallback cleanups all rule out.
/// The failure is a loud `RFN-004` error naming the serde cause; the
/// fallback stays (the export must still answer).
#[must_use]
pub fn types_json(entries: &[FnTypeEntry]) -> String {
    let mut decls: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        for (name, block) in &entry.decls {
            decls.entry(name.clone()).or_insert_with(|| block.clone());
        }
    }
    let payload = TypesPayload {
        fns: entries.to_vec(),
        decls,
    };
    serde_json::to_string(&payload).inspect_err(|e| {
        tracing::error!(
            code = crate::bridge::RFN_WIRE,
            fns = entries.len(),
            error = %e,
            "the types payload failed to serialize; the library's side export carries an EMPTY payload and no native fn will register at boot"
        );
    }).unwrap_or_default()
}

/// Render the `.d.ts` source from a `types_json` payload: the
/// declaration blocks sorted by type name, then the `export declare
/// function` lines sorted by fn name (deterministic output — the
/// house rule no timestamps, no map-order noise).
///
/// # Errors
///
/// [`crate::types::TYPES_JSON`] when the payload does not parse or a
/// fn record is malformed.
pub fn render_types_dts(json: &str) -> Result<String, OxideError> {
    let payload: TypesPayload = serde_json::from_str(json).map_err(|e| {
        err(
            TYPES_JSON,
            format!("the native function types payload did not parse: {e}"),
        )
    })?;
    let mut out = String::from("// Generated by oxide (V2-F08 native functions). Do not edit.\n\n");
    for block in payload.decls.values() {
        out.push_str(block);
        out.push('\n');
    }
    if !payload.decls.is_empty() {
        out.push('\n');
    }
    let mut fns = payload.fns;
    fns.sort_by(|a, b| a.name.cmp(&b.name));
    for f in &fns {
        let line = match f.args.as_slice() {
            [] => format!("export declare function {}(): {};\n", f.name, f.ret),
            [one] => format!(
                "export declare function {}(input: {}): {};\n",
                f.name, one, f.ret
            ),
            many => {
                let params: Vec<String> = many
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("arg{i}: {t}"))
                    .collect();
                format!(
                    "export declare function {}({}): {};\n",
                    f.name,
                    params.join(", "),
                    f.ret
                )
            }
        };
        out.push_str(&line);
    }
    Ok(out)
}

/// Render and persist the `.d.ts` atomically (temp file + rename, the
/// record-store convention) so a reader never sees a half-written
/// file.
///
/// # Errors
///
/// [`crate::types::TYPES_JSON`] when the payload does not parse;
/// filesystem errors ride their own codes.
pub fn write_types(json: &str, path: &Path) -> Result<(), OxideError> {
    let rendered = render_types_dts(json)?;
    let tmp = path.with_extension("d.ts.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            err(
                TYPES_JSON,
                format!("cannot create {}: {e}", parent.display()),
            )
        })?;
    }
    std::fs::write(&tmp, rendered)
        .map_err(|e| err(TYPES_JSON, format!("cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        err(
            TYPES_JSON,
            format!("cannot finalize {}: {e}", path.display()),
        )
    })?;
    Ok(())
}

/// The code a malformed types payload raises (`RFN-TYPES`).
pub const TYPES_JSON: &str = "RFN-TYPES";
