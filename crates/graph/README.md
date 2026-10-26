# oxide-fw-graph

**Incremental-build demand graph.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-graph.svg)](https://crates.io/crates/oxide-fw-graph) [![docs.rs](https://docs.rs/oxide-fw-graph/badge.svg)](https://docs.rs/oxide-fw-graph)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Records what a build read and what it computed, so a change invalidates only the nodes that actually depend on the changed input. The graph is what a future incremental `oxide build` will diff against — today, the pipeline runs to completion, but the graph makes the invalidation decision measurable.

Each node is a compute unit: one per module, chunk, manifest, or typegen entry. A compute reads files through the graph's `ComputeCtx`, and its output hash doubles as the "unchanged" signal — an input file whose content hash is unchanged is a survivor (early cutoff), so dependents are not re-run.

## What it provides

- **`DemandGraph`** — the graph itself; one node per compute unit
- **`Compute` trait** — what a node does; the framework registers one per module, chunk, manifest, and typegen entry
- **`ComputeCtx`** — the context a compute receives (filesystem, injected config, env vars)
- **`NodeOutput`** — the result of a compute: a hash, an optional artifact, and dependent-node references
- **`NodeKey`** — the stable identifier for a node across builds
- **`ContentStore`** — content-addressed storage for artifacts (rkyv-serialized, zstd-compressed)
- **`CancelToken`** — cooperative cancellation for a compute in flight

## Usage

```rust
use oxide_fw_graph::{Compute, ComputeCtx, DemandGraph, NodeKey, NodeOutput};
use std::sync::Arc;

let graph = Arc::new(DemandGraph::new());
graph.register(
    NodeKey::new("src/index.ts"),
    Arc::new(/* a Compute impl */),
);
let out = graph.get(NodeKey::new("src/index.ts"), &compute).await?;
```

> [!TIP]
> The graph is deliberately over-built for the current pipeline (which runs to completion). Its purpose is to make the question "would an incremental build skip this?" measurable now, before the incremental build exists. `oxide analyze` reads the last build's snapshot to report node counts and recomputed counts.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_build` records a `DemandGraph` over the build's inputs (routes, chunks, manifests, typegen) and persists it to `.oxide/cache/build-graph`. `oxide analyze` reads that snapshot to report how many nodes a build recomputed. The MCP `oxide_fw_graph` tool exposes the same data for editor integrations.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — records and persists the graph
- [`oxide-fw-cache`](https://crates.io/crates/oxide-fw-cache) — a different cache layer (responses, not build artifacts)

---

Full API reference: [docs.rs/oxide-fw-graph](https://docs.rs/oxide-fw-graph)
