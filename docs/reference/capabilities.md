<!-- GENERATED FILE — `oxide docs generate` (spec v0.2 §V2-E11). DO NOT EDIT.
Regenerate: cargo run -p oxide-fw-cli -- docs generate
Drift gate: crates/cli/tests/docs_tdd.rs -->

# Capability matrix

What each deploy target honours for every capability (spec §18). `F` full · `D` degraded (the reason is in the table below) · `U` unsupported — the §18 refusal rule fails the build at packaging time.
Generated from `oxide_fw_adapters::capability_markdown_table()`; the crate README embeds the same bytes.

| target | l0-cache | isr | sockets | image-optimization | native-rules | snapshots | tls | live | mixed-islands | resume | skew | delta-deploy | rust-fns | search | offline | brownout | permissions | attestation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| binary | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F |
| node | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |
| static | D | U | U | D | D | U | U | U | F | U | D | F | U | D | F | U | U | F |
| docker | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F |
| cloudflare | D | D | U | U | F | U | F | D | D | U | D | D | U | U | F | U | U | F |
| cloudflare-pages | D | D | U | U | F | U | F | D | D | U | D | D | U | U | F | U | U | F |
| lambda | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| vercel | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| vercel-functions | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| netlify | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| deno | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |
| bun | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |

## Degraded cells

Every `D` above with its documented reason (`oxide_fw_adapters::degradation` — a `D` cell without a reason is a red pin).

| target | capability | reason |
| --- | --- | --- |
| node | snapshots | no v8 startup snapshot on this runtime; cold start warmed via code cache |
| node | skew | the build-id handshake detects skew but the runtime cannot block a stale worker atomically |
| node | delta-deploy | the delta manifest is computed, but the platform upload remains full |
| node | search | the index lives in the Node worker; memory-bound by the isolate |
| node | brownout | the queue-depth signal is per-instance, so admission control is local only |
| node | permissions | the worker checks the grant table; the runtime cannot sandbox a violation |
| static | l0-cache | cache served from the CDN; no server-side L0 layer |
| static | image-optimization | build-time optimization only; no runtime /_oxide/image endpoint |
| static | native-rules | subset: redirects/headers via _redirects/_headers only (no rewrites/auth) |
| static | skew | static hosts swap files non-atomically; skew detection warns instead of blocking |
| static | search | a prebuilt index ships with the bundle; queries are client-side |
| cloudflare | l0-cache | KV/R2-backed cache with eventually-consistent fill |
| cloudflare | isr | ISR via stale-while-revalidate at the edge |
| cloudflare | live | Workers stream responses but do not guarantee long-lived connections; the client falls back to transport: 'poll' |
| cloudflare | mixed-islands | every framework's client runtime ships in the worker bundle; no cross-island shared chunk |
| cloudflare | skew | edge deploys propagate gradually; skew detection warns instead of blocking |
| cloudflare | delta-deploy | the delta manifest is computed, but Workers upload the full bundle |
| cloudflare-pages | l0-cache | KV/R2-backed cache with eventually-consistent fill |
| cloudflare-pages | isr | ISR via stale-while-revalidate at the edge |
| cloudflare-pages | live | Workers stream responses but do not guarantee long-lived connections; the client falls back to transport: 'poll' |
| cloudflare-pages | mixed-islands | every framework's client runtime ships in the worker bundle; no cross-island shared chunk |
| cloudflare-pages | skew | edge deploys propagate gradually; skew detection warns instead of blocking |
| cloudflare-pages | delta-deploy | the delta manifest is computed, but Workers upload the full bundle |
| lambda | l0-cache | remote cache handler; L0 is cold per instance |
| lambda | isr | ISR via on-demand regeneration where the platform supports it |
| lambda | image-optimization | build-time optimization plus the platform image service |
| lambda | snapshots | snapshot-friendly cold start; isolate reuse not guaranteed |
| lambda | live | serverless functions cannot hold long-lived SSE connections; the client falls back to transport: 'poll' |
| lambda | skew | function versions overlap across a deploy window; skew detection warns instead of blocking |
| lambda | delta-deploy | the delta manifest is computed, but the platform zip upload remains full |
| lambda | search | a read-only index ships with the bundle; reindexing is a deploy |
| vercel | l0-cache | remote cache handler; L0 is cold per instance |
| vercel | isr | ISR via on-demand regeneration where the platform supports it |
| vercel | image-optimization | build-time optimization plus the platform image service |
| vercel | snapshots | snapshot-friendly cold start; isolate reuse not guaranteed |
| vercel | live | serverless functions cannot hold long-lived SSE connections; the client falls back to transport: 'poll' |
| vercel | skew | function versions overlap across a deploy window; skew detection warns instead of blocking |
| vercel | delta-deploy | the delta manifest is computed, but the platform zip upload remains full |
| vercel | search | a read-only index ships with the bundle; reindexing is a deploy |
| vercel-functions | l0-cache | remote cache handler; L0 is cold per instance |
| vercel-functions | isr | ISR via on-demand regeneration where the platform supports it |
| vercel-functions | image-optimization | build-time optimization plus the platform image service |
| vercel-functions | snapshots | snapshot-friendly cold start; isolate reuse not guaranteed |
| vercel-functions | live | serverless functions cannot hold long-lived SSE connections; the client falls back to transport: 'poll' |
| vercel-functions | skew | function versions overlap across a deploy window; skew detection warns instead of blocking |
| vercel-functions | delta-deploy | the delta manifest is computed, but the platform zip upload remains full |
| vercel-functions | search | a read-only index ships with the bundle; reindexing is a deploy |
| netlify | l0-cache | remote cache handler; L0 is cold per instance |
| netlify | isr | ISR via on-demand regeneration where the platform supports it |
| netlify | image-optimization | build-time optimization plus the platform image service |
| netlify | snapshots | snapshot-friendly cold start; isolate reuse not guaranteed |
| netlify | live | serverless functions cannot hold long-lived SSE connections; the client falls back to transport: 'poll' |
| netlify | skew | function versions overlap across a deploy window; skew detection warns instead of blocking |
| netlify | delta-deploy | the delta manifest is computed, but the platform zip upload remains full |
| netlify | search | a read-only index ships with the bundle; reindexing is a deploy |
| deno | snapshots | no v8 startup snapshot on this runtime; cold start warmed via code cache |
| deno | skew | the build-id handshake detects skew but the runtime cannot block a stale worker atomically |
| deno | delta-deploy | the delta manifest is computed, but the platform upload remains full |
| deno | search | the index lives in the Node worker; memory-bound by the isolate |
| deno | brownout | the queue-depth signal is per-instance, so admission control is local only |
| deno | permissions | the worker checks the grant table; the runtime cannot sandbox a violation |
| bun | snapshots | no v8 startup snapshot on this runtime; cold start warmed via code cache |
| bun | skew | the build-id handshake detects skew but the runtime cannot block a stale worker atomically |
| bun | delta-deploy | the delta manifest is computed, but the platform upload remains full |
| bun | search | the index lives in the Node worker; memory-bound by the isolate |
| bun | brownout | the queue-depth signal is per-instance, so admission control is local only |
| bun | permissions | the worker checks the grant table; the runtime cannot sandbox a violation |
