// §16.5: content collections as a data surface.
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
//   import { getCollection, getEntry } from '@oxide-fw/oxide/server'
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

import { readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
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
const cache = new Map<string, { hash: string; entries: ContentEntry[] }>()

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
  // Read once; a missing file becomes the "no entries" error below.
  let raw: string
  try {
    raw = await readFile(path, 'utf8')
  } catch {
    throw new Error(
      `getCollection(${JSON.stringify(name)}): no entries file at ${path}. ` +
        `Is "${name}" in content.collections in oxide.config.ts, and has ` +
        `\`oxide build\` run since it was added?`,
    )
  }
  // §16.5 / WP-r9-1: key on a content hash, not (mtime, size). Two
  // same-size rewrites within one mtime quantum — or any mtime rounding
  // on tmpfs — used to serve stale entries. The file is small (a few KB
  // per collection); hashing it is cheaper than a wrong answer.
  const hash = createHash('sha256').update(raw).digest('hex')
  const cached = cache.get(path)
  if (cached && cached.hash === hash) {
    return cached.entries
  }
  const doc = JSON.parse(raw) as EntriesFile
  const entries = Array.isArray(doc.entries) ? doc.entries : []
  cache.set(path, { hash, entries })
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
