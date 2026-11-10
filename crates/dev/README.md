<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-dev

**Development server: file watching, HMR, proxies, and the waterfall inspector.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-dev?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-dev) [![Downloads](https://img.shields.io/crates/d/oxide-fw-dev?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-dev) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-dev?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-dev) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-dev?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-dev?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-dev)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Everything `oxide dev` needs beyond the production server. This crate does not render anything — it schedules reloads, publishes HMR messages, and tracks which file changed so only the affected route re-renders.

The design goal is a dev loop that never restarts the process. A source edit triggers a rebuild of only the affected route's CSS or module, then the HMR client swaps it in place. A config or server-module edit triggers a full reload with the stale-guard active — no serving pre-edit output.

## What it provides

- **`Watcher`** — debounced, content-hash-confirmed watch of source, config, and content dirs (seeded boot baseline; OS-native `notify` backend with a poll fallback)
- **`HmrHub`** — WebSocket fan-out with message kinds: `css-update`, `module-update`, `batch-module-update`, `route-data-update`, `server-module-swap`, `full-reload`, `error`, `connected`
- **`plan` / `Plan`** — the V2-E01 HMR boundary planner: a breadth-first walk-up over the module graph's importers that stops at self-accepting boundaries and falls back to a reasoned full reload (§V2-E01)
- **`boundary::plan_classified` / `boundary::ClassifiedPlan`** — the phase-2 classified planner: the watcher's `ChangeKind` and the walk in one call, with the §13.3 CSS terminal — a stylesheet change plans `kind: 'css'` and never reloads (the swap owns the update)
- **`boundary::UpdateKind` / `boundary::update_kind`** — the §13.3 wire discriminant (`js`/`css`/`data`) and the SPEC-14 bridge from the watcher's `ChangeKind`
- **`source_self_accepts`** — the `import.meta.hot.accept` probe that feeds the planner's accepting set
- **`boundary::css_self_accepts`** — the extension-level style-injection probe the harvest uses to mark stylesheets as boundaries
- **`full_reload_diagnostic`** — the `DEV-HMR-001` warning carrying the fix help text the §13.4 overlay renders
- **`boundary::css_orphan_diagnostic`** — the `DEV-HMR-002` warning for a stylesheet the module graph cannot see (the swap still runs; the fan-out never includes it)
- **`apply_proxy`** — longest-prefix proxy rule matching with optional path strip
- **`WaterfallRecorder`** — per-request timing rows for the inspector
- **`RouteTableInspector`** — the `/__oxide` route table snapshot
- **`classify_path` / `classify_event`** — the §13.3 change-kind classification that selects the HMR behavior (path-only, or event-aware — prefer `classify_event` at call sites that have the event kind)

## Usage

```rust
use oxide_fw_dev::{HmrHub, HmrMessage};

let hub = HmrHub::new();
hub.publish(HmrMessage::FullReload {
    reason: "config changed".into(),
});
```

The boundary planner is the other half of the contract — given the module
graph (reverse deps, self-accepting set, entry set), it decides whether an
edit updates a boundary set in place or falls back to a full reload:

```rust
use oxide_fw_dev::{Plan, plan, source_self_accepts};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

let changed = Path::new("src/lib/util.ts");
let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
importers.insert(
    PathBuf::from("src/lib/util.ts"),
    vec![PathBuf::from("src/routes/index/page.tsx")],
);
let accepting: HashSet<PathBuf> =
    [PathBuf::from("src/routes/index/page.tsx")].into_iter().collect();
let entries: HashSet<PathBuf> = HashSet::new();

// The walk-up stops at the component boundary: the page re-imports, the
// util is re-fetched through it.
assert_eq!(
    plan(changed, &importers, &accepting, &entries),
    Plan::Update(vec![PathBuf::from("src/routes/index/page.tsx")]),
);
assert!(source_self_accepts("import.meta.hot.accept(() => {});"));
```

> **Tip:** The planner is pure — the same graph always produces the same
> `Plan` (boundaries sorted and deduplicated), and `plan_classified` adds
> the §13.3 wire classification on top without changing that contract. The
> dev loop's stale-guard (`wrap_dispatch_with_stale_guard`, in
> `oxide-fw-cli`) sits one layer up: it aborts a mid-flight render when a
> file changed between dispatch entry and response completion.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_dev` creates the `Watcher` and `HmrHub`, then loops: on each file event, it classifies the path, triggers the appropriate rebuild (Tailwind CSS, plugin `handleHotUpdate`, route re-scan), runs the V2-E01 boundary planner over the harvested module graph (`build_hmr_graph`), and publishes the HMR message batch. The production `cmd_serve` path does not touch this crate.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — the dev loop that drives the watcher and hub
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the production pipeline
- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — the error-boundary markup a dev error overlay renders

---

Full API reference: [docs.rs/oxide-fw-dev](https://docs.rs/oxide-fw-dev)
