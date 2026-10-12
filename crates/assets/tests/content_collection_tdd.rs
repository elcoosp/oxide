//! Integration tests for §16.5 content collections + MDX bridging, exercising
//! the public API as a caller would.

use oxide_assets::{
    build_search_index, check_references, parse_mdx_source, scan_collection,
    search_index_to_json, validate_collection, CollectionConfig, FieldType, NativeSchema,
    SchemaKind,
};
use std::fs;
use std::path::Path;

fn write(dir: &Path, name: &str, content: &str) {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn cfg(dir: &Path, schema: SchemaKind) -> CollectionConfig {
    CollectionConfig {
        name: "posts".into(),
        dir: dir.to_path_buf(),
        schema,
        indexed_fields: vec!["title".into(), "excerpt".into()],
    }
}

#[test]
fn full_collection_pipeline_scan_validate_index() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "hello.md",
        "---\ntitle: Hello\nexcerpt: A first post\n---\n\nWelcome to the Rust world.\n",
    );
    write(
        dir.path(),
        "world.mdx",
        "---\ntitle: World\nexcerpt: A second post\n---\n\n# Hello, MDX\n",
    );

    let mut schema = NativeSchema::empty();
    schema.required.insert("title".into(), FieldType::String);
    schema.required.insert("excerpt".into(), FieldType::String);

    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Native(schema.clone()))).unwrap();
    assert_eq!(coll.entries.len(), 2);
    assert!(validate_collection(&coll, &SchemaKind::Native(schema)).is_empty());
    assert!(check_references(&coll).is_empty());

    let idx = build_search_index(&coll, &["title".into(), "excerpt".into()]);
    assert_eq!(idx.entries.len(), 2);
    // "rust" appears only in hello.md (index 0).
    assert_eq!(idx.terms.get("rust").map(Vec::as_slice), Some(&[0][..]));
    // "world" appears in hello's body AND world's title.
    let world_bucket = idx.terms.get("world").expect("world term");
    assert!(world_bucket.contains(&0));
    assert!(world_bucket.contains(&1));

    let json = search_index_to_json(&idx).unwrap();
    assert!(json.contains("\"collection\": \"posts\""));
    assert!(json.contains("\"hello\""));
    assert!(json.contains("\"world\""));
}

#[test]
fn collection_with_broken_reference_reports_ast_ref() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.md",
        "---\ntitle: A\n---\n\nSee [the other one](missing.md).\n",
    );
    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Unchecked)).unwrap();
    let diags = check_references(&coll);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "AST-REF");
    assert!(diags[0].message.contains("missing"));
}

#[test]
fn collection_schema_validation_reports_all_errors() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "---\ntitle: 42\n---\nbody\n");
    write(dir.path(), "b.md", "---\n---\nbody\n");

    let mut schema = NativeSchema::empty();
    schema.required.insert("title".into(), FieldType::String);

    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Native(schema.clone()))).unwrap();
    let diags = validate_collection(&coll, &SchemaKind::Native(schema));
    assert_eq!(diags.len(), 2);
    assert!(diags.iter().all(|d| d.code == "AST-SCH"));
}

#[test]
fn parse_mdx_source_yields_compile_request_targeting_mdx_js() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "post.mdx",
        "---\ntitle: Post\n---\n\n# Hello <Widget />\n",
    );
    let path = dir.path().join("post.mdx");
    let source = fs::read_to_string(&path).unwrap();
    let (entry, req) = parse_mdx_source(&path, &source).unwrap();
    assert_eq!(
        entry.frontmatter.get("title").unwrap(),
        &serde_json::json!("Post")
    );
    assert_eq!(req.module, "@mdx-js/mdx");
    assert_eq!(req.export, "compile");
    assert!(req.source.contains("<Widget />"));
    assert!(req.options.get("outputFormat").is_some());
    assert_eq!(req.options["outputFormat"], "function-body");
    assert_eq!(req.options["providerImportSource"], "@mdx-js/react");
}

#[test]
fn search_index_is_stable_and_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "---\ntitle: Alpha\n---\nRust is fast.\n");
    write(dir.path(), "b.md", "---\ntitle: Beta\n---\nRust is safe.\n");
    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Unchecked)).unwrap();
    let i1 = build_search_index(&coll, &["title".into()]);
    let i2 = build_search_index(&coll, &["title".into()]);
    assert_eq!(i1, i2);
    let j1 = search_index_to_json(&i1).unwrap();
    let j2 = search_index_to_json(&i2).unwrap();
    assert_eq!(j1, j2);
    // The JSON parses back into the same SearchIndex.
    let back: oxide_assets::SearchIndex = serde_json::from_str(&j1).unwrap();
    assert_eq!(back, i1);
}

#[test]
fn nested_collection_directories_are_scanned_recursively() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "2026/01/a.md", "---\ntitle: A\n---\nbody\n");
    write(dir.path(), "2026/02/b.md", "---\ntitle: B\n---\nbody\n");
    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Unchecked)).unwrap();
    assert_eq!(coll.entries.len(), 2);
    let slugs: Vec<&str> = coll.entries.iter().map(|e| e.slug.as_str()).collect();
    assert_eq!(slugs, vec!["a", "b"]);
}

#[test]
fn empty_collection_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let mut schema = NativeSchema::empty();
    schema.required.insert("title".into(), FieldType::String);
    let coll = scan_collection(&cfg(dir.path(), SchemaKind::Native(schema.clone()))).unwrap();
    assert!(coll.entries.is_empty());
    assert!(validate_collection(&coll, &SchemaKind::Native(schema)).is_empty());
    assert!(check_references(&coll).is_empty());
    let idx = build_search_index(&coll, &[]);
    assert!(idx.entries.is_empty());
    assert!(idx.terms.is_empty());
}

#[test]
fn external_schema_requires_nonempty_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "body without frontmatter\n");
    let schema = SchemaKind::External {
        validator_module: "zod".into(),
        validator_export: "z".into(),
        schema_json: serde_json::json!({}),
    };
    let coll = scan_collection(&cfg(dir.path(), schema.clone())).unwrap();
    let diags = validate_collection(&coll, &schema);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("frontmatter"));
}

#[test]
fn native_schema_serialization_round_trip() {
    let mut s = NativeSchema::empty();
    s.required.insert("title".into(), FieldType::String);
    s.optional.insert(
        "tags".into(),
        FieldType::Array(Box::new(FieldType::String)),
    );
    let json = serde_json::to_value(&s).unwrap();
    let back: NativeSchema = serde_json::from_value(json).unwrap();
    assert_eq!(back, s);
}
