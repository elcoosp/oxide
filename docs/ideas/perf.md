Recon complete. Here's what I found, ordered by measured-or-measurable impact.

## Tier 1 — per-request hot path (every dynamic SSR request)

### 1. Four `O(routes)` linear scans per request, all redundant

`build_dispatch`'s precompute runs **per request**, and does four full `tree.entries` walks to find middleware / not-found / error / layout ancestors:

```rust
for candidate in &tree.entries {   // middleware   (line 3605)
for candidate in &tree.entries {   // not-found    (line 3668)
for candidate in &tree.entries {   // error        (line 3704)
```

These are **pure functions of the matched route id**, which is known at build time. With 200 routes that's ~800 comparisons per request; at 10k req/s it's 8M wasted comparisons/s, and each carries a `format!("{base}/")` allocation. **Fix:** precompute a `route_id → (middleware, error, not_found, layouts)` table once into the `OnceLock` you already have for `self_dispatch`, and probe it. Straightforward, large win on route-heavy apps.

### 2. Endpoint inputs built unconditionally for every page GET

```rust
let endpoint_params_json = params.iter().map(...).collect();  // allocs a Map
let endpoint_headers = req.headers.clone();                    // clones every header
let endpoint_body = req.body.clone();                          // refcount bump
let endpoint_method_for_dispatch = req.method.clone();         // String alloc
let endpoint_url_for_dispatch = req.url.clone();               // String alloc
```

All five are used **only** inside the `if let (Some(dispatcher), Some(mp)) = (endpoint_for_dispatch, endpoint_module_path)` branch — which is empty for every non-`route.ts` request. **Fix:** move them inside the branch. Saves 2 String allocs + 1 Map alloc + a full header-vec clone on every page render.

### 3. `pattern_of(&m.route.segments)` allocates a String per request for a metrics label

`status`/`label` are computed before the async block; `label` is the URL pattern. It's used as a metrics key and in `tracing!`. **Fix:** the router could store a precomputed pattern on `RouteEntry` (it already stores `segments`); then `label` is `&str`. `Metrics::observe` already takes `&str` and does a `read()` lookup — that read-lock + hash on every request is also worth a `RwLock` → `papaya`/`dashmap` swap if metrics matter.

### 4. `nonce_for` called up to 5× per request

Lines 4036, 4075, 4353, 4362, 4450 all call `oxide_server::nonce_for(&req_owned.request_id)` — it's a hash of the request id. Compute once per request, pass it down.

---

## Tier 2 — router: the spec's "allocation-free" claim isn't met

§8.2 says *"Match is allocation-free on the hot path"*. It isn't:

```rust
pub(crate) fn match_in<E>(...) -> Option<(usize, BTreeMap<String,String>, i64)> {
    for (i, e) in entries.iter().enumerate() {      // O(n) over ALL routes
        let mut acc: Vec<(String,String)> = Vec::new();   // alloc per candidate
        if let Some(score) = match_entry(...) {
            let prios: Vec<u8> = (0..e.seg_len()).map(...).collect();  // alloc per candidate
```

Per request: **linear scan over every route**, a `Vec` allocation per candidate that matches, and a `BTreeMap` at the end. Two fixes, in order of value:

- **Build the radix tree the spec describes.** `RouteTree` currently holds `Vec<RouteEntry>` and `match_in` iterates all of them. A real radix trie makes match O(segments), not O(routes) — this is the single biggest algorithmic gap between the tree and §8.2.
- **`SmallVec`/`ArrayVec` for `prios` and `acc`** removes the per-candidate heap allocation even without the trie.

Note `arch_match_impl` calls `layout_chain(entries, ...)` on every match — another full scan, recomputing what `match_in` could return alongside.

---

## Tier 3 — cache and compression

### 5. `compress_body` runs **synchronously on the async runtime**

`finish_response_full` calls `compress_body(&enc, &resp.body, 3)` inline. gzip/zstd/brotli at level 3 on a 50 KB HTML page is ~0.5–2 ms of pure CPU. On a Tokio worker thread that blocks the reactor. **Fix:** `tokio::task::spawn_blocking`, or better, a rayon pool. This is a classic throughput killer under load and is invisible in single-request benchmarks.

Also: `resp.headers.retain(...)` plus two `push`es per compressed response — the header vec could be built with the encoding decision already made.

### 6. `vary_of` allocates a String per request for the cache key

`CacheKey::canonical()` then blake3-hashes the concatenation and `.to_hex().to_string()` — **two allocations + a hash per lookup**. For an L0 hit (target p99 < 1 ms) this is fine, but for L1-heavy traffic it adds up. **Fix:** hash directly into a `[u8; 16]` key (no hex string), and make `CacheKey` borrow `&str` instead of owning `String`.

### 7. Tag index is `Vec<String>` with `iter().any()` dedupe

```rust
for tag in &entry.tags {
    let mut list = self.tags.entry(tag.clone()).or_default();
    if !list.iter().any(|k| k == key) { list.push(key.to_string()); }   // O(n) per insert
```

Writing an entry with 5 tags into a tag holding 10k keys is 50k string compares. **Fix:** `HashSet<String>` per tag (ordering isn't needed), or a second `key→tags` reverse index to avoid the dedupe scan entirely.

The disk tier is worse: **`serde_json` round-trip of the whole tag list on every `set`** (line 240-251). That's read-decode-mutate-encode per tag, per cache write.

---

## Tier 4 — build-time (pairs with your compiler work)

### 8. `precompile_entries` is sequential

Already flagged in the compiler discussion — `while let Some(path) = queue.pop_front()` awaits each file. Rayon `par_iter` is worth **~8.5×** on a 12-core box independent of any compiler swap.

### 9. `html.replace(&nonce, NONCE_SENTINEL)` copies the whole document

Lines 2864, 2943, 3023 — once per prerendered page. `str::replace` allocates a fresh `String` of the full HTML. For a 200 KB page × 5,000 prerendered pages that's 1 GB of transient allocation. **Fix:** only replace if `html.contains(&nonce)` (it always does, but the check enables a fast path when the nonce is already the sentinel), or do the substitution during the `lol_html` rewrite pass you're already running.

### 10. `layout_ids` recomputed in the prerender loop

Lines 2760/2789 in the prerender path do the same per-route scans as the runtime path — same fix (precompute the map once).

---

## Tier 5 — structural / lower certainty

- **`mimalloc` is configured but is it actually the global allocator?** Your spec (§12.2) calls for it. Worth verifying `#[global_allocator]` is present — if it isn't, that alone is often 5–15% on allocation-heavy servers.
- **`tokio::spawn` per streamed response** (line 4465) — a task per request for the tee/accumulate. Fine, but the 8-slot channel means backpressure stalls at 8 chunks; the spec's TTFB tuning may want a larger buffer or `try_send` fast path.
- **`Metrics::observe` takes a write lock on first sight of a route** — that's a `parking_lot` write lock on the request path the first time. Pre-register route labels at boot from the route table.
- **`req.clone()`** at 3802 clones the header vec + body per request. `Bytes` is cheap but the `Vec<(String,String)>` isn't. Consider `Arc<[Header]>`.

---

## Suggested order

1. **Move the 5 endpoint inputs inside their branch** — 10 min, zero risk, every page request.
2. **Precompute the route-ancestor map into the `OnceLock`** — kills 4 scans/request.
3. **Hoist `nonce` to one call per request** — 5 min.
4. **`compress_body` → `spawn_blocking`** — biggest throughput win under load, ~20 min.
5. **Verify `mimalloc` is actually installed as `#[global_allocator]`** — free 5–15% if missing.
6. **`SmallVec` for `prios`/`acc` in `match_in`** — removes router allocations.
7. **Radix trie for `match_in`** — the real algorithmic fix; bigger effort, biggest ceiling.
8. **Rayon `par_iter` in `precompile_entries`** — pairs with the native-compiler work.

Items 1–5 are all local, low-risk, and together are probably **10–25% on dynamic SSR throughput** without touching the router's architecture.
