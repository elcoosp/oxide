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
