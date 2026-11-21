//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// Emit a search index per configured content collection (§16.5). Writes
/// `<dist>/content/<name>.index.json` — a deterministic JSON document the
/// front end fetches lazily.
///
/// A collection whose directory is missing is reported and skipped; the build
/// itself succeeds (content often lives outside the main pipeline's blast
/// radius).
/// Write via a sibling temp file + rename, so a dev-loop rescan cannot
/// expose a half-written entries file to a concurrent `getCollection`.
pub(crate) fn write_content_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// V2-F09 (feature `search`): the built-in search index emission — for
/// every configured `search.collections` name, index the collection's
/// entries into `dist/search/index` (the mmap-able tantivy index the
/// serve side opens read-only) and export `dist/search/static.json`
/// (the compact fallback the client matcher loads lazily). The §16.5
/// `content/<name>.index.json` projection stays untouched — it is the
/// loader-side contract (`getCollection`); the F09 index is the
/// serve-side query surface the spec adds on top.
///
/// The rebuild is CAS-shaped (the spec: "the build is CAS-cached by the
/// content hash of the collection"): the corpus' blake3
/// (`TantivyIndex::content_hash`) is kept next to the artifact in
/// `dist/search/.hash`; an unchanged hash skips the rebuild. An
/// unknown collection name fails the build with `SRC-106` — a typo in
/// `search.collections` must not ship an empty index.
#[cfg(feature = "search")]
pub(crate) fn emit_search_engine_index(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    use oxide_fw_assets::scan_collection;
    // The engine's doc_count is the SearchIndex trait method.
    use oxide_fw_search::SearchIndex as _;
    let search = &cfg.search;
    if search.collections.is_empty() {
        return Ok(());
    }
    let out_dir = dist.join("search");
    std::fs::create_dir_all(&out_dir)?;
    let mut docs: Vec<oxide_fw_search::SearchDocument> = Vec::new();
    for name in &search.collections {
        let coll = cfg
            .content
            .collections
            .iter()
            .find(|c| &c.name == name)
            .ok_or_else(|| {
                OxideError(Box::new(
                    oxide_fw_diagnostics::Diagnostic::error(
                        "SRC-106",
                        format!("search.collections names `{name}` but content.collections has no such collection"),
                    )
                    .with_fix(format!(
                        "add the collection to content.collections, or change search.collections to one of: {}",
                        cfg.content
                            .collections
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                ))
            })?;
        let dir = rootp.join(&coll.dir);
        let coll_cfg = oxide_fw_assets::CollectionConfig {
            name: coll.name.clone(),
            dir,
            schema: oxide_fw_assets::SchemaKind::Unchecked,
            indexed_fields: coll.indexed_fields.clone(),
        };
        let scanned = scan_collection(&coll_cfg)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        for entry in &scanned.entries {
            docs.push(search_emit::collection_document(name, entry, search));
        }
    }
    let hash = oxide_fw_search::TantivyIndex::content_hash(&docs);
    let marker = out_dir.join(".hash");
    if std::fs::read_to_string(&marker).is_ok_and(|h| h.trim() == hash) {
        outln!(
            w,
            "search index unchanged ({} docs, hash {})",
            docs.len(),
            &hash[..12]
        );
        return Ok(());
    }
    // The engine owns a fresh directory (a stale index must never be
    // committed onto), then the static fallback + the CAS marker. The
    // wipe is the engine's own: a bare `remove_dir_all` loses the race
    // against the serving reader's OnCommit reloader (its 500 ms poll
    // tick re-creates `.tantivy-meta.lock` mid-wipe, and the refresh
    // would fail before the rebuild ever runs).
    let index_dir = out_dir.join(oxide_fw_search::SEARCH_INDEX_DIR);
    oxide_fw_search::wipe_dir_for_rebuild(&index_dir)?;
    let opts = oxide_fw_search::SearchEngineOptions::from_config(search);
    let idx = oxide_fw_search::TantivyIndex::build_in_dir(&docs, &opts, &index_dir)?;
    let static_json = oxide_fw_search::StaticIndex::build(&docs).to_json()?;
    write_content_atomic(
        &out_dir.join(oxide_fw_search::SEARCH_STATIC_FILE),
        static_json.as_bytes(),
    )?;
    write_content_atomic(&marker, hash.as_bytes())?;
    outln!(
        w,
        "search index: {} docs (hash {}) + {}",
        idx.doc_count(),
        &hash[..12],
        oxide_fw_search::SEARCH_STATIC_FILE
    );
    Ok(())
}

/// V2-F09 (feature `search`): the dev loop's post-change refresh —
/// re-emit the F09 index (the CAS marker makes an unchanged corpus a
/// no-op) and swap the serve side onto the rebuild. The reload is
/// best-effort exactly like the wire-up's open: a failure (a mid-rebuild
/// wipe, a corrupt dist) is a loud warning and the previous snapshot
/// keeps serving `/_oxide/search` — never a dev-loop failure. Called
/// after the §16.5 projection's re-emission in the watcher's
/// content-change branch (and once in the `--once` hook), with the
/// handle `start_server_inner_full` attached to [`StartedServer`].
#[cfg(feature = "search")]
pub(crate) fn refresh_search_index(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    hot: Option<&std::sync::Arc<oxide_fw_search::SwappableSearchIndex>>,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    emit_search_engine_index(rootp, cfg, &rootp.join("dist"), w)?;
    let Some(hot) = hot else {
        return Ok(());
    };
    match hot.try_reload() {
        Ok(oxide_fw_search::ReloadOutcome::Swapped { docs }) => {
            outln!(w, "search index reloaded ({docs} docs)");
        }
        Ok(oxide_fw_search::ReloadOutcome::Unchanged) => {}
        Err(e) => {
            tracing::warn!(
                code = e.0.code,
                message = %e.0.message,
                "search index reload skipped; the previous index keeps serving"
            );
        }
    }
    Ok(())
}

pub(crate) fn emit_content_search_indexes(
    root: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    if cfg.content.collections.is_empty() {
        return Ok(());
    }
    let out_dir = dist.join("content");
    std::fs::create_dir_all(&out_dir)?;
    // §16-C: collect each scanned collection so a typed `content.d.ts` can be
    // emitted once the loop finishes. Config carries no schema field yet, so
    // the schema is inferred from the frontmatter actually on disk.
    let mut scanned_collections: Vec<oxide_fw_assets::Collection> = Vec::new();
    let mut inferred_schemas: Vec<(String, oxide_fw_assets::NativeSchema)> = Vec::new();
    for coll in &cfg.content.collections {
        if coll.name.is_empty() || coll.dir.is_empty() {
            outln!(w, "content collection skipped: name and dir are required");
            continue;
        }
        let dir = root.join(&coll.dir);
        let cfg_in = oxide_fw_assets::CollectionConfig {
            name: coll.name.clone(),
            dir: dir.clone(),
            schema: oxide_fw_assets::SchemaKind::Unchecked,
            indexed_fields: coll.indexed_fields.clone(),
        };
        let scanned = match oxide_fw_assets::scan_collection(&cfg_in) {
            Ok(c) => c,
            Err(e) => {
                outln!(
                    w,
                    "content collection `{}` skipped: {}",
                    coll.name,
                    e.0.message
                );
                continue;
            }
        };
        let idx = oxide_fw_assets::build_search_index(&scanned, &coll.indexed_fields);
        let json = oxide_fw_assets::search_index_to_json(&idx)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        write_content_atomic(
            &out_dir.join(format!("{}.index.json", coll.name)),
            json.as_bytes(),
        )?;
        // §16.5: emit the full entries too. The search index is a
        // *projection* (terms + slugs + title/fields), too lossy for a
        // loader that needs the body. A page.server.ts's
        // `getCollection(name)` reads this file at request time.
        let entries_json = serde_json::json!({
            "name": scanned.name,
            "entries": scanned.entries.iter().map(|e| serde_json::json!({
                "slug": e.slug,
                "frontmatter": e.frontmatter,
                "body": e.body_markdown,
                "is_mdx": e.is_mdx,
            })).collect::<Vec<_>>(),
        });
        if let Ok(pretty) = serde_json::to_string_pretty(&entries_json) {
            // WP-r9-1: do not swallow the write error. A silent failure
            // here surfaces later as a misleading "no entries file" from
            // `getCollection` at request time; the build should fail loud.
            write_content_atomic(
                &out_dir.join(format!("{}.entries.json", coll.name)),
                pretty.as_bytes(),
            )
            .map_err(|e| anyhow::anyhow!("write content/{}.entries.json: {e}", coll.name))?;
        }
        outln!(
            w,
            "content index: {} ({} entries) -> dist/content/{}.index.json",
            coll.name,
            scanned.entries.len(),
            coll.name
        );
        inferred_schemas.push((
            coll.name.clone(),
            oxide_fw_assets::content_typegen::infer_schema(&scanned),
        ));
        scanned_collections.push(scanned);
    }
    // Pair each scanned collection with its inferred schema.
    let typed_collections: Vec<oxide_fw_assets::content_typegen::TypedCollection<'_>> =
        scanned_collections
            .iter()
            .enumerate()
            .map(|(i, c)| oxide_fw_assets::content_typegen::TypedCollection {
                collection: c,
                schema: Some(&inferred_schemas[i].1),
            })
            .collect();
    // §16-C: emit `.oxide/types/content.d.ts`. A typegen failure is a
    // warning, not a build failure — the app still ships.
    let dts_path = root.join(".oxide/types/content.d.ts");
    match oxide_fw_assets::content_typegen::write_content_dts(&typed_collections, &dts_path) {
        Ok(()) => outln!(
            w,
            "content typegen: {} collection(s) -> .oxide/types/content.d.ts",
            typed_collections.len()
        ),
        Err(e) => outln!(
            w,
            "warn: content.d.ts typegen failed ({} {}); stale",
            e.0.code,
            e.0.message
        ),
    }
    Ok(())
}
