<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-dev

**Development server: file watching, HMR, proxies, and the waterfall inspector.**

[![Crates.io](https://crates.io/crates/oxide-fw-dev) `soon`] [![docs.rs](https://docs.rs/oxide-fw-dev/badge.svg)](https://docs.rs/oxide-fw-dev)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Everything `oxide dev` needs beyond the production server. This crate does not render anything — it schedules reloads, publishes HMR messages, and tracks which file changed so only the affected route re-renders.

The design goal is a dev loop that never restarts the process. A source edit triggers a rebuild of only the affected route's CSS or module, then the HMR client swaps it in place. A config or server-module edit triggers a full reload with the stale-guard active — no serving pre-edit output.

## What it provides

- **`FileWatcher`** — debounced watch of source, config, and content dirs
- **`HmrHub`** — WebSocket fan-out with message kinds: `css-update`, `module-update`, `route-data-update`, `server-module-swap`, `full-reload`, `error`
- **`apply_proxy`** — longest-prefix proxy rule matching with optional path strip
- **`WaterfallRecorder`** — per-request timing rows for the inspector
- **`RouteTableInspector`** — the `/__oxide` route table snapshot
- **`classify_path`** — "is this a source file we should react to" predicate
- **`wrap_dispatch_with_stale_guard`** — abort a mid-flight render if a file changed between dispatch entry and response completion

## Usage

```rust
use oxide_fw_dev::{HmrHub, HmrMessage};

let hub = HmrHub::new();
hub.publish(HmrMessage::FullReload {
    reason: "config changed".into(),
});
```

> **Tip:** The stale-guard exists to prevent a narrow race: a render started before a file edit but finishing after it. Without the guard, that render would serve output computed from pre-change source, which can confuse HMR's swap logic.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_dev` creates the `FileWatcher` and `HmrHub`, then loops: on each file event, it classifies the path, triggers the appropriate rebuild (Tailwind CSS, plugin `handleHotUpdate`, route re-scan), and publishes the HMR message. The production `cmd_serve` path does not touch this crate.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — the dev loop that drives the watcher and hub
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the production pipeline
- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — the error-boundary markup a dev error overlay renders

---

Full API reference: [docs.rs/oxide-fw-dev](https://docs.rs/oxide-fw-dev)
