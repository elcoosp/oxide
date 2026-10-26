<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-assets

**CSS, images, fonts, i18n catalogs, and Tailwind v4 — the asset pipeline.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-assets?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-assets) [![Downloads](https://img.shields.io/crates/d/oxide-fw-assets?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-assets) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-assets?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-assets) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-assets?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-assets?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-assets)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Everything Oxide does to turn source assets into runtime-ready files. CSS is processed through Lightning CSS, images through the `image` + `ravif` + `resvg` crates, fonts through subsetting, i18n catalogs are parsed and negotiated per request, and Tailwind v4 is bridged from its Rust scanner to the CLI.

## What it provides

- **`LightningCssBackend`** — bundling, nesting, autoprefixing, minification against the project's browser target
- **`RustImageBackend`** — decode, resize, encode to PNG/JPEG/WebP/AVIF
- **`SvgOgBackend`** — OG-image rasterization (resvg + taffy + parley)
- **Font subsetting** — via `subsetter` + `skrifa`
- **i18n** — catalog parsing, locale negotiation, content negotiation
- **Content collections** — frontmatter, markdown, per-route typegen
- **Tailwind v4** — the runtime bridge to `@tailwindcss/oxide`

## Usage

```rust
use oxide_fw_assets::{CssBackend, CssInput, CssKind, LightningCssBackend};

let out = LightningCssBackend.transform(&CssInput {
    code: ".card { color: var(--fg); }".into(),
    file: None,
    kind: CssKind::Global,
    minify: true,
    targets: vec!["chrome 100".into()],
})?;
println!("{}", out.code);
```

> **Tip:** Enable the `image`, `font`, or `i18n` features to keep the default build lean if you only need one of them.

## How it fits in Oxide

`oxide-fw-bundler` builds a `CssTransformHook` from `LightningCssBackend`. `oxide-fw-server` uses the image endpoint from this crate for on-demand transformation. `oxide-fw-cli` wires Tailwind's Rust scanner via the tooling pool.

## See also

- [`oxide-fw-bundler`](https://crates.io/crates/oxide-fw-bundler) — consumes the CSS transform
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — serves the image endpoint
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — orchestrates the pipeline

---

Full API reference: [docs.rs/oxide-fw-assets](https://docs.rs/oxide-fw-assets)
