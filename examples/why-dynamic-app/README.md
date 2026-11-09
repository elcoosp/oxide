# why-dynamic-app — the V2-F16 `oxide why` demonstration

Four routes whose loaders read request state in four different ways, so a
single `oxide why` session can exercise every reason kind the engine
reports. Nothing here needs Node or a browser: the why walk is a
build-time AST pass over `src/routes/**/page.server.ts`.

| Route | Loader read | `oxide routes` mode | `oxide why` reasons |
| --- | --- | --- | --- |
| `routes/home` | no `page.server.ts` | `static` | none |
| `routes/dash` | `cookies` (bind + `.get`) | `dynamic` | 2 × `cookieRead` |
| `routes/search` | `searchParams` | `dynamic` | 2 × `searchParamRead` |
| `routes/feed` | `fetch(` outside `'use cache'` | `static` | 1 × `uncachedFetch` |

## Walkthrough

```bash
cargo build --release -p oxide-fw-cli   # from the workspace root

./target/release/oxide routes --root examples/why-dynamic-app
# PATTERN | KIND | MODE | CACHE | TAGS
# /            page | static | L0   | -
# /dash        page | dynamic | none | -
# /feed        page | static | L0   | -
# /search      page | dynamic | none | -

./target/release/oxide why --root examples/why-dynamic-app --route routes/dash
```

The `why --route` surface prints one `OX-DYN` diagnostic per reason
(Appendix V2.K), each with the 1-based line, a code frame and the fix
help text:

```
route routes/dash — mode: dynamic
[OX-DYN] route is dynamic: cookies read at src/routes/dash/page.server.ts:4
4 | export async function load({ cookies }) {
  |                             ^
  fix: read the cookie in the loader and return it as data — render code
       consumes props, never the request
...
```

`routes/feed` is the interesting one: its mode stays `static` (a fetch
reads no request state) but the report still flags the call because the
response cannot be cached while the fetch is uncached — the fix suggests
wrapping the fetch in a `'use cache'` function.

## The side-car

`oxide routes` and `oxide build` write `.oxide/routes.reasons.json` next
to the routes manifest — a plain JSON map of route id → reason rows
(`kind`, `line`, `snippet`, `span`), deliberately **not** inside the rkyv
archive the request hot path reads. Inspectors and the MCP `routes.list`
tool consume it instead of re-running the AST pass:

```bash
cat examples/why-dynamic-app/.oxide/routes.reasons.json
```

The `--fix` codemods (cookie read → loader data, `searchParams` →
declared `searchSchema`, uncached fetch → `'use cache'`) run in the
tooling pool and are tracked in `docs/deviations.md`.
