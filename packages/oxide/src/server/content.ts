// §16 / K1: content collections as a data surface.
//
// `oxide build` scans each collection (see `content.collections` in
// `oxide.config.ts`) and emits two files to `dist/content/`:
//
//   <name>.index.json    — the search projection: slugs + a term map.
//                          Consumed by a client-side search island.
//   <name>.entries.json  — the full entries: slug, frontmatter, body.
//                          Consumed by this module.
//
// A loader calls:
//
//   import { getCollection, getEntry } from 'oxide/server'
//   export async function load() {
//     const posts = await getCollection('blog')
//     const hello = await getEntry('blog', 'hello-world')
//     return { posts, hello }
//   }
//
// Root resolution: prefer OXIDE_ROOT (set by the CLI when it spawns
// workers for both `oxide build` and `oxide start`), fall back to
// process.cwd(). The env var is what a `--root` invocation needs — the
// worker's cwd is inherited from the parent shell, not from the flag.

import { readFile, stat } from 'node:fs/promises'
import { join } from 'node:path'

/** One entry as emitted by the build. `frontmatter` is the raw parsed
 * map; `body` is the Markdown/MDX source (unrendered — a caller that
 * wants HTML calls `renderMarkdown` from `oxide/server`). */
export interface ContentEntry {
  slug: string
  frontmatter: Record<string, unknown>
  body: string
  /** `true` when the source is `.mdx` — the body needs MDX compilation
   * to render. `renderMarkdown` handles both; a caller doing its own
   * rendering should branch on this. */
  is_mdx?: boolean
}

interface EntriesFile {
  name: string
  entries: ContentEntry[]
}

// Per-collection cache keyed by (path, mtime_ms, size). A rebuild
// bumps mtime; a read of the same file twice in one request is free.
const cache = new Map<
  string,
  { mtimeMs: number; size: number; entries: ContentEntry[] }
>()

function projectRoot(): string {
  return process.env.OXIDE_ROOT ?? process.cwd()
}

/**
 * Read every entry in a collection. Throws when the collection was not
 * built (a typo in the name, or a project that has never run `oxide
 * build`) — the error message names the expected file so a fix is one
 * step away.
 */
export async function getCollection(name: string): Promise<ContentEntry[]> {
  if (!name || name.length === 0) {
    throw new Error('getCollection: name is required')
  }
  const path = join(projectRoot(), 'dist', 'content', `${name}.entries.json`)
  let st
  try {
    st = await stat(path)
  } catch {
    throw new Error(
      `getCollection(${JSON.stringify(name)}): no entries file at ${path}. ` +
        `Is "${name}" in content.collections in oxide.config.ts, and has ` +
        `\`oxide build\` run since it was added?`,
    )
  }
  const cached = cache.get(path)
  if (cached && cached.mtimeMs === st.mtimeMs && cached.size === st.size) {
    return cached.entries
  }
  const raw = await readFile(path, 'utf8')
  const doc = JSON.parse(raw) as EntriesFile
  const entries = Array.isArray(doc.entries) ? doc.entries : []
  cache.set(path, { mtimeMs: st.mtimeMs, size: st.size, entries })
  return entries
}

/**
 * Read one entry by slug. Returns `undefined` when the slug is absent
 * from the collection (a miss in a loader is often a 404, not an error).
 * Throws only when the *collection* itself is missing — the same
 * contract as `getCollection`.
 */
export async function getEntry(
  collection: string,
  slug: string,
): Promise<ContentEntry | undefined> {
  const entries = await getCollection(collection)
  return entries.find((e) => e.slug === slug)
}

/**
 * Test-only: clear the mtime cache. The public surface is
 * `getCollection` / `getEntry`; a test that reads the same file after
 * rewriting it needs a fresh stat.
 */
export function __clearContentCache(): void {
  cache.clear()
}
