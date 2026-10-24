# @oxide/playwright

Real-browser E2E for the framework adapter read path (§7 / §22). The suite
boots `oxide start` against `examples/smoke-app-react` and drives Chromium against
it via Playwright.

Not run by the default `pnpm test` — the workspace dev host has ~4 GB RAM
and Playwright's Chromium alone needs ~500 MB. Run it on a machine with
more headroom.

```sh
cargo build -p oxide-fw-cli                # one-time, produces target/debug/oxide
pnpm test:playwright:install              # downloads Chromium
pnpm test:playwright                      # boots oxide start + drives the browser
```

## Specs

| Spec | What it covers | Smoke-app route |
| --- | --- | --- |
| `smoke.spec.ts`      | Wire + browser for `/`, health, `/blog/:slug`         | `index`, `blog/[slug]` |
| `island.spec.ts`     | `<ox-island>` markup + browser hydration + click     | `index` (Counter) |
| `middleware.spec.ts` | Middleware `locals` reach the loader; `redirect` short-circuits | `greeting`, `middleware` |
| `loading.spec.ts`    | §8.4 loading overlay during client navigation        | `slow` |
| `socket.spec.ts`     | §9.6 WebSocket greet + echo                          | `chat/socket` |
| `og.spec.ts`         | S-09 OG endpoint rasterizes a real PNG              | `/_oxide/og` |
| `sse.spec.ts`        | §9.6 SSE headers + progressive streaming             | `api/events` |

Each spec boots its own `oxide start` server on its own port (3100-3105)
in `beforeAll`, so the seven specs can run in any order (or in parallel).

## Running one spec

    pnpm --filter @oxide/playwright test:middleware
    pnpm --filter @oxide/playwright test:loading
    pnpm --filter @oxide/playwright test:socket
    pnpm --filter @oxide/playwright test:sse

## CI

Wired into `.github/workflows/ci.yml` as the `playwright` job: cargo
build, install workspace deps, `playwright install --with-deps chromium`,
then `playwright test`. One job, not one-per-spec; the suite boots each
spec's server on its own port (3100-3106) so a shared Chromium does not
make the job slower than a serialised split would.
then `pnpm --filter @oxide/playwright test`. It is orthogonal to the Rust
and node jobs and can run on a longer timeout.
