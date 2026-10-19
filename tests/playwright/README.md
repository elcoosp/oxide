# @oxide/playwright

Real-browser E2E for the framework adapter read path (§7 / §22). The suite
boots `oxide start` against `examples/smoke-app-react` and drives Chromium against
it via Playwright.

Not run by the default `pnpm test` — the workspace dev host has ~4 GB RAM
and Playwright's Chromium alone needs ~500 MB. Run it on a machine with
more headroom.

```sh
cargo build -p oxide-cli                  # one-time, produces target/debug/oxide-cli
pnpm test:playwright:install              # downloads Chromium
pnpm test:playwright                      # boots oxide start + drives the browser

## Specs

| Spec | What it covers | Smoke-app route |
| --- | --- | --- |
| `smoke.spec.ts`      | Wire + browser for `/`, health, `/blog/:slug`         | `index`, `blog/[slug]` |
| `island.spec.ts`     | `<ox-island>` markup + browser hydration + click     | `index` (Counter) |
| `middleware.spec.ts` | Middleware `locals` reach the loader; `redirect` short-circuits | `greeting`, `middleware` |
| `loading.spec.ts`    | §8.4 loading overlay during client navigation        | `slow` |
| `socket.spec.ts`     | §9.6 WebSocket greet + echo                          | `chat/socket` |
| `sse.spec.ts`        | §9.6 SSE headers + progressive streaming             | `api/events` |

Each spec boots its own `oxide start` server on its own port (3100-3105)
in `beforeAll`, so the six specs can run in any order (or in parallel).

## Running one spec

    pnpm --filter @oxide/playwright test:middleware
    pnpm --filter @oxide/playwright test:loading
    pnpm --filter @oxide/playwright test:socket
    pnpm --filter @oxide/playwright test:sse

## CI

Not wired into the default `.github/workflows/ci.yml` (Chromium install +
six server boots make the job slow). When ready, add a separate job that
runs `pnpm install`, `pnpm --filter @oxide/playwright install-browsers`,
then `pnpm --filter @oxide/playwright test`. It is orthogonal to the Rust
and node jobs and can run on a longer timeout.
