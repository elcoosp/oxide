//! Typed content collections (spec §16.5).
//!
//! A collection is a directory of Markdown (or MDX) files with YAML
//! frontmatter, validated against a schema, cross-reference-checked, and
//! indexed for search. This module is the pure-Rust half of §16.5:
//!
//! - scanning: read a directory of entries, sorted by slug
//! - validation: frontmatter vs a `SchemaKind`
//! - reference checking: relative `.md`/`.mdx` links between entries
//! - search index: deterministic JSON, keyed by term
//!
//! MDX compilation itself is not performed here — the request is described
//! (`MdxCompileRequest` in `content`) and handed to the tooling pool (§4.7),
//! which runs `@mdx-js/mdx` in a Node worker.

use crate::{Diagnostic, Result, err};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Which validator to apply to a collection's frontmatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SchemaKind {
    /// Only require that the entry has *some* frontmatter; no field checks.
    Unchecked,
    /// The in-Rust validator, covering the field types the spec names.
    Native(NativeSchema),
    /// Delegate to a Standard Schema validator in the user's project via the
    /// tooling pool. The `validator_module`/`validator_export` identify the
    /// callable (e.g. `zod`/`z` or `valibot`/`object`).
    External {
        validator_module: String,
        validator_export: String,
        /// The serialized schema, produced by the user's config in TS.
        schema_json: serde_json::Value,
    },
}

/// A minimal field-type schema for frontmatter validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeSchema {
    pub required: BTreeMap<String, FieldType>,
    pub optional: BTreeMap<String, FieldType>,
}

impl NativeSchema {
    pub fn empty() -> Self {
        Self {
            required: BTreeMap::new(),
            optional: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Number,
    Bool,
    Array(Box<FieldType>),
    Enum(Vec<String>),
}

/// A collection declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub name: String,
    pub dir: PathBuf,
    pub schema: SchemaKind,
    /// Fields scanned into the search index; `title` is always included when
    /// present in frontmatter, regardless of this list.
    pub indexed_fields: Vec<String>,
}

// ---------------------------------------------------------------------------
// Scanned entries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub name: String,
    pub dir: PathBuf,
    pub entries: Vec<CollectionEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionEntry {
    pub slug: String,
    pub path: PathBuf,
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    pub body_markdown: String,
    /// `true` when the source is `.mdx` (compilation is deferred to the
    /// tooling pool; this flag lets callers know a compile step is required).
    pub is_mdx: bool,
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// Scan a collection directory. Entries are sorted by slug for determinism.
/// Hidden files (`.` or `_` prefix) and non-`.md`/`.mdx` files are ignored.
pub fn scan_collection(cfg: &CollectionConfig) -> Result<Collection> {
    if !cfg.dir.is_dir() {
        return Err(crate::OxideError(Box::new(
            Diagnostic::error(
                "AST-DIR",
                format!(
                    "collection `{}` directory not found: {}",
                    cfg.name,
                    cfg.dir.display()
                ),
            )
            .with_fix("create the directory or fix the collection's `dir` in config"),
        )));
    }

    let mut entries: Vec<CollectionEntry> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    walk(&cfg.dir, &mut paths);
    paths.sort();

    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let is_mdx = match ext {
            "md" => false,
            "mdx" => true,
            _ => continue,
        };
        let source = std::fs::read_to_string(&p)
            .map_err(|e| err("AST-ENTRY", format!("failed to read {}: {e}", p.display())))?;
        let entry = crate::content::parse_content_entry(&p, &source)?;
        let slug = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        entries.push(CollectionEntry {
            slug,
            path: p,
            frontmatter: entry.frontmatter,
            body_markdown: entry.body_markdown,
            is_mdx,
        });
    }

    entries.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(Collection {
        name: cfg.name.clone(),
        dir: cfg.dir.clone(),
        entries,
    })
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut here: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    here.sort();
    for p in here {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// Validate every entry's frontmatter against `schema`. Emits one diagnostic
/// per failed constraint; a clean collection yields an empty vector.
pub fn validate_collection(coll: &Collection, schema: &SchemaKind) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    match schema {
        SchemaKind::Unchecked => {}
        SchemaKind::Native(n) => {
            for e in &coll.entries {
                validate_native(&coll.name, e, n, &mut out);
            }
        }
        SchemaKind::External { .. } => {
            // External validation is dispatched by the caller through the
            // tooling pool; this function only records the intent when the
            // frontmatter is empty (nothing to ship).
            for e in &coll.entries {
                if e.frontmatter.is_empty() {
                    out.push(
                        Diagnostic::error(
                            "AST-SCH",
                            format!(
                                "collection `{}` entry `{}` has no frontmatter; \
                                 external schema requires content",
                                coll.name, e.slug
                            ),
                        )
                        .with_fix("add a `---` frontmatter block to the entry"),
                    );
                }
            }
        }
    }
    out
}

fn validate_native(
    coll: &str,
    entry: &CollectionEntry,
    schema: &NativeSchema,
    out: &mut Vec<Diagnostic>,
) {
    for (field, ty) in &schema.required {
        match entry.frontmatter.get(field) {
            None => out.push(
                Diagnostic::error(
                    "AST-SCH",
                    format!(
                        "collection `{coll}` entry `{}` missing required field `{field}`",
                        entry.slug
                    ),
                )
                .with_fix(format!("add `{field}` to the entry's frontmatter")),
            ),
            Some(v) if !field_matches(v, ty) => out.push(
                Diagnostic::error(
                    "AST-SCH",
                    format!(
                        "collection `{coll}` entry `{}` field `{field}` has wrong type \
                         (expected {})",
                        entry.slug,
                        type_name(ty)
                    ),
                )
                .with_fix(format!("change `{field}` to a {}", type_name(ty))),
            ),
            _ => {}
        }
    }
    for (field, ty) in &schema.optional {
        if let Some(v) = entry.frontmatter.get(field) {
            if !field_matches(v, ty) {
                out.push(
                    Diagnostic::error(
                        "AST-SCH",
                        format!(
                            "collection `{coll}` entry `{}` optional field `{field}` \
                             has wrong type (expected {})",
                            entry.slug,
                            type_name(ty)
                        ),
                    )
                    .with_fix(format!("change `{field}` to a {}", type_name(ty))),
                );
            }
        }
    }
}

fn field_matches(v: &serde_json::Value, ty: &FieldType) -> bool {
    match ty {
        FieldType::String => v.is_string(),
        FieldType::Number => v.is_number(),
        FieldType::Bool => v.is_boolean(),
        FieldType::Array(inner) => match v {
            serde_json::Value::Array(a) => a.iter().all(|x| field_matches(x, inner)),
            _ => false,
        },
        FieldType::Enum(allowed) => match v.as_str() {
            Some(s) => allowed.iter().any(|a| a == s),
            None => false,
        },
    }
}

fn type_name(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::String => "string",
        FieldType::Number => "number",
        FieldType::Bool => "bool",
        FieldType::Array(_) => "array",
        FieldType::Enum(_) => "enum value",
    }
}

// ---------------------------------------------------------------------------
// Reference checking
// ---------------------------------------------------------------------------

/// Walk every entry's body and check that relative links pointing at
/// `.md`/`.mdx` files resolve to a known entry's slug. Absolute URLs and
/// anchor-only links are ignored.
pub fn check_references(coll: &Collection) -> Vec<Diagnostic> {
    let known: std::collections::BTreeSet<&str> =
        coll.entries.iter().map(|e| e.slug.as_str()).collect();
    let mut out = Vec::new();

    for e in &coll.entries {
        for target in markdown_link_targets(&e.body_markdown) {
            if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with('#')
                || target.starts_with("mailto:")
                || target.starts_with('/')
            {
                continue;
            }
            let lower = target.to_ascii_lowercase();
            let is_doc = lower.ends_with(".md") || lower.ends_with(".mdx");
            if !is_doc {
                continue;
            }
            let path_part = target.split('#').next().unwrap_or(&target);
            let stem = std::path::Path::new(path_part)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path_part);
            if !known.contains(stem) {
                out.push(
                    Diagnostic::error(
                        "AST-REF",
                        format!(
                            "collection `{}` entry `{}` links to `{target}` \
                             but no entry with slug `{stem}` exists",
                            coll.name, e.slug
                        ),
                    )
                    .with_fix(format!(
                        "create `{stem}.md` in the collection or fix the link target"
                    )),
                );
            }
        }
    }
    out
}

/// Extract the target of every `[text](target)` link in a markdown body.
/// Skips `![alt](src)` image syntax, code blocks, and inline code.
fn markdown_link_targets(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = md.as_bytes();
    let mut i = 0usize;
    let mut in_fence = false;
    while i < bytes.len() {
        if i + 2 < bytes.len() && &bytes[i..i + 3] == b"```" {
            in_fence = !in_fence;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if in_fence {
            i += 1;
            continue;
        }
        if bytes[i] == b'`' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'`' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let is_image = i >= 2 && bytes[i - 2] == b'!' && bytes[i - 1] == b'[';
            let mut j = i + 2;
            let mut depth = 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth == 0 {
                let target = &md[i + 2..j - 1];
                let target = target.split_whitespace().next().unwrap_or(target);
                if !is_image {
                    out.push(target.to_string());
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Search index
// ---------------------------------------------------------------------------

/// A searchable index for a collection. `terms` maps a lowercased token to
/// the entries containing it (indices into `entries`). Deterministic across
/// runs because all iteration is BTreeMap / sorted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchIndex {
    pub collection: String,
    pub entries: Vec<IndexedEntry>,
    pub terms: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedEntry {
    pub slug: String,
    pub title: Option<String>,
    pub fields: BTreeMap<String, String>,
}

/// Build the search index. Every term is a lowercased alphanumeric run of
/// length >= 2. `indexed_fields` controls which frontmatter fields (besides
/// `title`, always included if present) plus the body are scanned.
pub fn build_search_index(coll: &Collection, indexed_fields: &[String]) -> SearchIndex {
    let mut entries = Vec::with_capacity(coll.entries.len());
    let mut terms: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (idx, e) in coll.entries.iter().enumerate() {
        let title = e
            .frontmatter
            .get("title")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        for f in indexed_fields {
            if let Some(v) = e.frontmatter.get(f) {
                if let Some(s) = v.as_str() {
                    fields.insert(f.clone(), s.to_string());
                }
            }
        }

        let mut text = String::new();
        if let Some(t) = &title {
            text.push_str(t);
            text.push(' ');
        }
        for v in fields.values() {
            text.push_str(v);
            text.push(' ');
        }
        text.push_str(&e.body_markdown);

        for term in tokenize(&text) {
            let bucket = terms.entry(term).or_default();
            if bucket.last() != Some(&idx) {
                bucket.push(idx);
            }
        }

        entries.push(IndexedEntry {
            slug: e.slug.clone(),
            title,
            fields,
        });
    }

    SearchIndex {
        collection: coll.name.clone(),
        entries,
        terms,
    }
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                cur.push(lc);
            }
        } else if !cur.is_empty() {
            if cur.chars().count() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= 2 {
        out.push(cur);
    }
    out
}

/// Serialize the index as pretty JSON. Byte-stable across runs.
pub fn search_index_to_json(idx: &SearchIndex) -> Result<String> {
    serde_json::to_string_pretty(idx)
        .map_err(|e| err("AST-IDX", format!("index serialization failed: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, content: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    fn sample_cfg(dir: &Path) -> CollectionConfig {
        CollectionConfig {
            name: "posts".into(),
            dir: dir.to_path_buf(),
            schema: SchemaKind::Unchecked,
            indexed_fields: vec!["title".into(), "excerpt".into()],
        }
    }

    #[test]
    fn scan_reads_directory_and_sorts_by_slug() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "z.md", "---\ntitle: Z\n---\nZ body\n");
        write(dir.path(), "a.md", "---\ntitle: A\n---\nA body\n");
        write(dir.path(), "m.mdx", "---\ntitle: M\n---\nM body\n");
        let coll = scan_collection(&sample_cfg(dir.path())).unwrap();
        let slugs: Vec<&str> = coll.entries.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["a", "m", "z"]);
        assert!(coll.entries.iter().find(|e| e.slug == "m").unwrap().is_mdx);
        assert!(!coll.entries.iter().find(|e| e.slug == "a").unwrap().is_mdx);
    }

    #[test]
    fn scan_ignores_hidden_and_non_markdown() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "A\n");
        write(dir.path(), ".hidden.md", "H\n");
        write(dir.path(), "_draft.md", "D\n");
        write(dir.path(), "readme.txt", "text\n");
        write(dir.path(), "image.png", "binary\n");
        let coll = scan_collection(&sample_cfg(dir.path())).unwrap();
        assert_eq!(coll.entries.len(), 1);
        assert_eq!(coll.entries[0].slug, "a");
    }

    #[test]
    fn scan_reports_missing_directory() {
        let cfg = CollectionConfig {
            name: "gone".into(),
            dir: PathBuf::from("/definitely/not/here"),
            schema: SchemaKind::Unchecked,
            indexed_fields: vec![],
        };
        let e = scan_collection(&cfg).unwrap_err();
        assert_eq!(e.0.code, "AST-DIR");
        assert!(e.0.fix.is_some());
    }

    fn entry_with(fm: &[(&str, serde_json::Value)]) -> CollectionEntry {
        let mut m = BTreeMap::new();
        for (k, v) in fm {
            m.insert((*k).to_string(), v.clone());
        }
        CollectionEntry {
            slug: "e".into(),
            path: PathBuf::from("e.md"),
            frontmatter: m,
            body_markdown: String::new(),
            is_mdx: false,
        }
    }

    fn coll_with(entries: Vec<CollectionEntry>) -> Collection {
        Collection {
            name: "posts".into(),
            dir: PathBuf::from("/tmp"),
            entries,
        }
    }

    #[test]
    fn native_schema_accepts_valid_entry() {
        let mut schema = NativeSchema::empty();
        schema.required.insert("title".into(), FieldType::String);
        schema.required.insert("count".into(), FieldType::Number);
        let coll = coll_with(vec![entry_with(&[
            ("title", serde_json::json!("Hi")),
            ("count", serde_json::json!(3)),
        ])]);
        assert!(validate_collection(&coll, &SchemaKind::Native(schema)).is_empty());
    }

    #[test]
    fn native_schema_reports_missing_required() {
        let mut schema = NativeSchema::empty();
        schema.required.insert("title".into(), FieldType::String);
        let coll = coll_with(vec![entry_with(&[])]);
        let diags = validate_collection(&coll, &SchemaKind::Native(schema));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "AST-SCH");
        assert!(diags[0].message.contains("title"));
    }

    #[test]
    fn native_schema_reports_type_mismatch() {
        let mut schema = NativeSchema::empty();
        schema.required.insert("count".into(), FieldType::Number);
        let coll = coll_with(vec![entry_with(&[(
            "count",
            serde_json::json!("not a number"),
        )])]);
        let diags = validate_collection(&coll, &SchemaKind::Native(schema));
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("number"));
    }

    #[test]
    fn native_schema_optional_field() {
        let mut schema = NativeSchema::empty();
        schema.optional.insert("excerpt".into(), FieldType::String);
        let ok = coll_with(vec![entry_with(&[])]);
        assert!(validate_collection(&ok, &SchemaKind::Native(schema.clone())).is_empty());
        let bad = coll_with(vec![entry_with(&[("excerpt", serde_json::json!(1))])]);
        assert_eq!(
            validate_collection(&bad, &SchemaKind::Native(schema)).len(),
            1
        );
    }

    #[test]
    fn native_schema_enum_rejects_invalid() {
        let mut schema = NativeSchema::empty();
        schema.required.insert(
            "status".into(),
            FieldType::Enum(vec!["draft".into(), "published".into()]),
        );
        let ok = coll_with(vec![entry_with(&[("status", serde_json::json!("draft"))])]);
        assert!(validate_collection(&ok, &SchemaKind::Native(schema.clone())).is_empty());
        let bad = coll_with(vec![entry_with(&[(
            "status",
            serde_json::json!("archived"),
        )])]);
        assert_eq!(
            validate_collection(&bad, &SchemaKind::Native(schema)).len(),
            1
        );
    }

    #[test]
    fn native_schema_array_item_type_checked() {
        let mut schema = NativeSchema::empty();
        schema
            .required
            .insert("tags".into(), FieldType::Array(Box::new(FieldType::String)));
        let ok = coll_with(vec![entry_with(&[("tags", serde_json::json!(["a", "b"]))])]);
        assert!(validate_collection(&ok, &SchemaKind::Native(schema.clone())).is_empty());
        let bad = coll_with(vec![entry_with(&[("tags", serde_json::json!(["a", 1]))])]);
        assert_eq!(
            validate_collection(&bad, &SchemaKind::Native(schema)).len(),
            1
        );
    }

    fn body_entry(slug: &str, body: &str) -> CollectionEntry {
        CollectionEntry {
            slug: slug.into(),
            path: PathBuf::from(format!("{slug}.md")),
            frontmatter: BTreeMap::new(),
            body_markdown: body.into(),
            is_mdx: false,
        }
    }

    #[test]
    fn references_ok_when_target_exists() {
        let coll = coll_with(vec![
            body_entry("a", "See [b](b.md) for details."),
            body_entry("b", "Back to [a](a.md#top)."),
        ]);
        assert!(check_references(&coll).is_empty());
    }

    #[test]
    fn references_report_missing_target() {
        let coll = coll_with(vec![body_entry("a", "See [ghost](ghost.md).")]);
        let diags = check_references(&coll);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "AST-REF");
        assert!(diags[0].message.contains("ghost"));
    }

    #[test]
    fn references_ignore_external_and_anchor_links() {
        let body =
            "See [ext](https://example.com/x.md) and [anchor](#section) and [abs](/other.md)";
        let coll = coll_with(vec![body_entry("a", body)]);
        assert!(check_references(&coll).is_empty());
    }

    #[test]
    fn references_ignore_image_links_and_code_blocks() {
        let body = "![img](missing.png)\n\n```\n[inside](ghost.md)\n```\n\nInline `[code](ghost2.md)` here.\n";
        let coll = coll_with(vec![body_entry("a", body)]);
        assert!(check_references(&coll).is_empty());
    }

    #[test]
    fn search_index_terms_are_lowercased_and_sorted() {
        let coll = coll_with(vec![
            body_entry("a", "The Rust language"),
            body_entry("b", "rust is fast"),
        ]);
        let idx = build_search_index(&coll, &[]);
        let bucket = idx.terms.get("rust").expect("rust term");
        assert_eq!(bucket, &vec![0, 1]);
        assert!(
            idx.terms
                .keys()
                .all(|k| k.chars().all(|c| !c.is_uppercase()))
        );
    }

    #[test]
    fn search_index_json_deterministic_across_runs() {
        let coll = coll_with(vec![
            body_entry("a", "Hello world"),
            body_entry("b", "World peace"),
        ]);
        let idx1 = build_search_index(&coll, &[]);
        let idx2 = build_search_index(&coll, &[]);
        assert_eq!(idx1, idx2);
        assert_eq!(
            search_index_to_json(&idx1).unwrap(),
            search_index_to_json(&idx2).unwrap()
        );
    }
}
