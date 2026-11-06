# Using Tailwind CSS v4

Oxide ships a native Tailwind v4 plugin.

## Setup

1. Add the plugin to `oxide.config.ts`:

~~~ts
import { tailwind } from '@oxide-fw/tailwind'
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  plugins: [tailwind()],
})
~~~

2. Create `src/styles/tailwind.css`:

~~~css
@import "tailwindcss";

@theme static {
  --color-primary: #e45631;
  --color-bg: #09090b;
}
~~~

**Use `@theme static`** (not plain `@theme`) when you consume tokens
from hand-written CSS like `body { background: var(--color-bg) }`.
Tailwind v4 tree-shakes variables that no utility references, so
without `static` your `--color-bg` vanishes from the compiled output
and the body falls back to white.

3. The CLI auto-compiles this entry and injects the `<link>` tag in both
   `oxide dev` and `oxide build`. No further wiring required.

## Dev HMR

Editing `src/styles/tailwind.css` OR any `.tsx`/`.vue`/`.svelte` with
utility classes triggers a rebuild of the compiled CSS file. The dev
server rewrites the hashed filename and deletes the old one so exactly
one `tailwind-*.css` exists in `dist/client/` at any time.

## Diagnosing missing styles

If styles are missing in dev:

1. Check the CLI output for `tailwind: tailwind-<hash>.css` — the hash
   line means the compiler ran.
2. Check that `dist/client/tailwind-<hash>.css` is > 20KB (small = raw
   CSS with no utilities = scanner failure).
3. Check that `tailwind.css` uses `@theme static` if you reference
   tokens from hand-written CSS.
