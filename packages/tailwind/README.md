<div align="center">
  <img src="../../docs/logo.png" alt="Oxide" width="140"/>
  <p>
    <strong>The Tailwind v4 native plugin for Oxide.</strong><br/>
    Build-time CSS generation through Tailwind's own Rust scanner and
    compiler — no PostCSS, no config file, no CSS-in-JS. The plugin
    reads the project's source, scans for utility classes, and hands
    back a single stylesheet ready for Lightning CSS.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Tailwind-v4-38BDF8?style=flat-square&logo=tailwindcss" alt="Tailwind v4"/>
    <img src="https://img.shields.io/badge/Node-24%20type%20stripping-339933?style=flat-square&logo=node.js" alt="Node 24 type stripping"/>
    <img src="https://img.shields.io/badge/PostCSS-not%20needed-228B22?style=flat-square" alt="No PostCSS"/>
    <img src="https://img.shields.io/badge/Licence-MIT%20OR%20Apache--2.0-228B22?style=flat-square" alt="Licence"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#usage">Usage</a> · <a href="#what-it-does">What it does</a> · <a href="#how-it-fits">How it fits</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
npm install -D @oxide-fw/tailwind
```

Add it to `oxide.config.ts`:

```ts
import { defineConfig } from "oxide"
import tailwind from "@oxide-fw/tailwind"

export default defineConfig({
  plugins: [tailwind()],
})
```

Add a single stylesheet — `src/styles/tailwind.css`:

```css
@import "tailwindcss";

/* Optional: project-specific theme overrides. */
@theme {
  --color-brand: #38BDF8;
}
```

Import it once in the layout:

```tsx
import "../styles/tailwind.css"
```

## Usage

Utility classes work exactly as they do in a normal Tailwind project:

```tsx
export default function Home() {
  return (
    <main className="flex min-h-screen bg-slate-50">
      <h1 className="text-4xl font-bold text-brand">Hello Oxide</h1>
    </main>
  )
}
```

The plugin scans every source file (`.tsx`, `.vue`, `.svelte`, `.html`,
`.js`, `.jsx`) and emits exactly the utility classes it finds — plus
Tailwind's preflight reset and any `@theme` variables you declared.

## What it does

- **Build-time scan** — every `.ts/.tsx/.vue/.svelte/.html` file under `src/`
  is read; only the utility classes that appear are emitted.
- **Compile through Tailwind's own pipeline** — uses `@tailwindcss/oxide`'s
  Rust scanner and `tailwindcss`'s JS compiler. No reimplementation.
- **Lightning CSS handoff** — the emitted stylesheet is passed to
  `oxide-fw-assets` for prefixing, nesting, minification, and browser targeting.
- **HMR in dev** — editing any source file rebuilds the CSS in place;
  the client HMR runtime swaps the `<link>` with a cache-bust query.
- **Zero config** — no `tailwind.config.js`, no `postcss.config.js`. Theme
  overrides live in the CSS file via `@theme { ... }`.
- **Preflight included** — the emitted stylesheet starts with Tailwind's
  CSS reset, so no normalize.css is needed.

## How it fits

Oxide builds CSS through this pipeline:

```
source files → tailwind_scan() → tailwind_build()
             → Lightning CSS   → dist/client/tailwind-<hash>.css
```

`crates/cli/src/tailwind.rs` orchestrates the scan; `crates/jshost/src/tooling_pool.rs`
runs it inside the Node tooling pool. The CSS is served as a static asset by
`oxide-fw-server`. In dev, `crates/cli/src/lib.rs`'s HMR loop calls
`rebuild_for_dev()` on every source edit.

The package does not import React, Vue, Svelte, or Solid — the emitted
classes work with any of them, because the scan is class-string based,
not framework-aware.

## Licence

MIT OR Apache-2.0 — see the [workspace licence](https://github.com/elcoosp/oxide#licence)
for the full text.

