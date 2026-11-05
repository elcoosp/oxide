// src/lib/md.server.ts — server-only content helpers for the site.
//
// The collection query surface (`getCollection` / `getEntry`) reads the
// entries emitted by `oxide build` / `oxide dev`; this module maps those
// typed entries onto the site's post/doc shape and renders the Markdown
// subset below.
//
// Supported markdown subset (keep posts inside it): headings #..####,
// paragraphs, -/* and 1. lists, pipe tables, > blockquotes, ``` fences,
// --- rules, inline `code`, **bold**, [links](https://… or /relative).
import { getCollection, getEntry } from '@oxide-fw/core/server'

export interface Frontmatter {
  [key: string]: string
}

export interface PostMeta {
  slug: string
  title: string
  description: string
  date: string
  tags: string[]
}

export type DocBlock = MdBlock

export interface DocMeta {
  slug: string
  title: string
  description: string
  order: number
}

const SLUG_RE = /^[a-z0-9][a-z0-9-]*$/

function esc(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function parseTags(raw: string | undefined): string[] {
  if (!raw) return []
  return raw
    .replace(/^\[|\]$/g, '')
    .split(',')
    .map((t) => t.trim())
    .filter(Boolean)
}

// Compiled once per process: `inline()` runs per markdown line on every
// (re)generation of a docs/blog page — per-call regex literals re-allocate
// the compiled regex, which is avoidable work on ISR regens.
const SAFE_HREF_RE = /^(https?:\/\/|\/|#|mailto:)/
const INLINE_CODE_RE = /`([^`]+)`/g
const INLINE_CODE_RESTORE_RE = /\u0000(\d+)\u0000/g
const BOLD_RE = /\*\*([^*]+)\*\*/g
const LINK_RE = /\[([^\]]+)\]\(([^)\s]+)\)/g

function safeHref(href: string): string {
  return SAFE_HREF_RE.test(href) ? href : '#'
}

/** Inline markdown → HTML. Input is raw; output is escaped-safe. */
function inline(s: string): string {
  let out = esc(s)
  // stash inline code first so other rules can't touch it
  const codes: string[] = []
  out = out.replace(INLINE_CODE_RE, (_m, c: string) => {
    codes.push(c)
    return `\u0000${codes.length - 1}\u0000`
  })
  out = out.replace(BOLD_RE, '<strong>$1</strong>')
  out = out.replace(LINK_RE, (_m, text: string, href: string) => {
    return `<a href="${safeHref(href)}">${text}</a>`
  })
  out = out.replace(INLINE_CODE_RESTORE_RE, (_m, i: string) => `<code>${codes[Number(i)]}</code>`)
  return out
}

function splitRow(line: string): string[] {
  let t = line.trim()
  if (t.startsWith('|')) t = t.slice(1)
  if (t.endsWith('|')) t = t.slice(0, -1)
  return t.split('|').map((c) => c.trim())
}


/** Typed indexed access; the parser's while guards prove the index is
 *  valid but noUncheckedIndexedAccess doesn't narrow it. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`md.server: index ${i} out of bounds (len=${arr.length})`)
  return v
}

/** Markdown block subset → HTML string. */

export type MdBlock = string | { type: 'framework-tabs'; data: Record<string, string> }

export async function mdToHtml(md: string): Promise<MdBlock[]> {
  const lines = md.replace(/\r\n/g, '\n').split('\n')
  const out: MdBlock[] = []
  let i = 0
  while (i < lines.length) {
    const line = mustEl(lines, i)
    if (line.startsWith('```')) {
      // The info string is everything after the backticks on the opening line.
      const info = line.slice(3).trim()
      const buf: string[] = []
      i++
      while (i < lines.length && !mustEl(lines, i).startsWith('```')) buf.push(mustEl(lines, i++))
      i++ // closing fence

      if (info === 'framework-tabs') {
        const parts: Record<'react' | 'vue' | 'svelte' | 'solid', string> = { react: '', vue: '', svelte: '', solid: '' }
        let cur: 'react' | 'vue' | 'svelte' | 'solid' | null = null
        let sec: string[] = []
        const flush = () => {
          if (cur) parts[cur] = sec.join('\n').replace(/\s+$/, '')
          sec = []
        }
        for (const l of buf) {
          const m = /^---(react|vue|svelte|solid)\s*$/.exec(l)
          if (m) {
            flush()
            cur = m[1] as 'react' | 'vue' | 'svelte' | 'solid'
            continue
          }
          if (cur) sec.push(l)
        }
        flush()

        out.push({ type: 'framework-tabs', data: parts })
        continue
      }

      out.push(`<pre><code>${esc(buf.join('\n'))}</code></pre>`)
      continue
    }
    const h = /^(#{1,4})\s+(.*)$/.exec(line)
    if (h) {
      const level = mustEl(h, 1).length
      out.push(`<h${level}>${inline(mustEl(h, 2))}</h${level}>`)
      i++
      continue
    }
    if (/^(---|\*\*\*)\s*$/.test(line)) {
      out.push('<hr>')
      i++
      continue
    }
    if (line.startsWith('>')) {
      // Blockquote: `>` + optional space (CommonMark allows `>text`). The
      // previous `startsWith('> ')` requirement made a `>`-without-space line
      // fall through EVERY block branch into the paragraph guard (which also
      // refuses `>` lines) — the walk never advanced and the render looped
      // forever at 100% CPU, wedging the SSR worker synchronously.
      const buf: string[] = []
      while (i < lines.length && mustEl(lines, i).startsWith('>'))
        buf.push(mustEl(lines, i++).replace(/^>[ ]?/, ''))
      out.push(`<blockquote><p>${buf.map(inline).join(' ')}</p></blockquote>`)
      continue
    }
    if (/^[-*]\s+/.test(line)) {
      const items: string[] = []
      while (i < lines.length && /^[-*]\s+/.test(mustEl(lines, i))) items.push(mustEl(lines, i++).replace(/^[-*]\s+/, ''))
      out.push(`<ul>${items.map((it) => `<li>${inline(it)}</li>`).join('')}</ul>`)
      continue
    }
    if (/^\d+[.)]\s+/.test(line)) {
      const items: string[] = []
      while (i < lines.length && /^\d+[.)]\s+/.test(mustEl(lines, i))) items.push(mustEl(lines, i++).replace(/^\d+[.)]\s+/, ''))
      out.push(`<ol>${items.map((it) => `<li>${inline(it)}</li>`).join('')}</ol>`)
      continue
    }
    if (
      line.trim().startsWith('|') &&
      i + 1 < lines.length &&
      /^\|[\s:|-]+\|?\s*$/.test(mustEl(lines, i + 1).trim())
    ) {
      const head = splitRow(line)
      i += 2
      const rows: string[][] = []
      while (i < lines.length && mustEl(lines, i).trim().startsWith('|')) rows.push(splitRow(mustEl(lines, i++)))
      const th = head.map((c) => `<th>${inline(c)}</th>`).join('')
      const trs = rows
        .map((r) => `<tr>${r.map((c) => `<td>${inline(c)}</td>`).join('')}</tr>`)
        .join('')
      out.push(`<div class="table-wrap"><table><thead><tr>${th}</tr></thead><tbody>${trs}</tbody></table></div>`)
      continue
    }
    if (line.trim() === '') {
      i++
      continue
    }
    const para: string[] = []
    while (
      i < lines.length &&
      mustEl(lines, i).trim() !== '' &&
      !/^(#{1,4}\s|```|>|[-*]\s|\d+[.)]\s|\|)/.test(mustEl(lines, i))
    ) {
      para.push(mustEl(lines, i++))
    }
    if (para.length === 0) {
      // Forward-progress guarantee: a line that matched NO block branch above
      // and is refused by the paragraph guard (a stray `|` row that is not a
      // table, `>` handled above, …) must still be consumed — otherwise the
      // outer while never advances and rendering hangs at 100% CPU. Consume
      // it as literal paragraph text; the escape inside `inline()` keeps it
      // inert.
      para.push(mustEl(lines, i++))
    }
    out.push(`<p>${para.map(inline).join(' ')}</p>`)
  }
  return out
}

function fmString(v: unknown, fallback = ''): string {
  if (typeof v === 'string') return v
  if (v == null) return fallback
  if (Array.isArray(v)) return v.map(String).join(', ')
  return String(v)
}

function fmTags(v: unknown): string[] {
  if (Array.isArray(v)) return v.map(String).map((t) => t.trim()).filter(Boolean)
  return parseTags(fmString(v))
}

export async function listPosts(): Promise<PostMeta[]> {
  const entries = await getCollection('blog')
  return entries
    .map((e) => ({
      slug: e.slug,
      title: fmString(e.frontmatter.title, e.slug),
      description: fmString(e.frontmatter.description),
      date: fmString(e.frontmatter.date),
      tags: fmTags(e.frontmatter.tags),
    }))
    // Newest first. Posts with an unparseable/missing date (`new Date('')`)
    // sort LAST — the previous comparator treated `''` as newer than every
    // real date, so an undated draft floated to the top of the blog index
    // (and the RSS feed would have emitted `<pubDate>Invalid Date</pubDate>`).
    .sort((a, b) => {
      const ta = Date.parse(a.date)
      const tb = Date.parse(b.date)
      if (Number.isNaN(ta) && Number.isNaN(tb)) return 0
      if (Number.isNaN(ta)) return 1
      if (Number.isNaN(tb)) return -1
      return ta < tb ? 1 : -1
    })
}

export async function readPost(
  slug: string,
): Promise<{ meta: PostMeta; html: MdBlock[] } | null> {
  if (!SLUG_RE.test(slug)) return null
  const hit = await getEntry('blog', slug)
  if (!hit) return null
  return {
    meta: {
      slug: hit.slug,
      title: fmString(hit.frontmatter.title, hit.slug),
      description: fmString(hit.frontmatter.description),
      date: fmString(hit.frontmatter.date),
      tags: fmTags(hit.frontmatter.tags),
    },
    html: await mdToHtml(hit.body),
  }
}

export async function listDocs(): Promise<DocMeta[]> {
  const entries = await getCollection('docs')
  return entries
    .map((e) => ({
      slug: e.slug,
      title: fmString(e.frontmatter.title, e.slug),
      description: fmString(e.frontmatter.description),
      order: Number(e.frontmatter.order ?? 99),
    }))
    .sort((a, b) => a.order - b.order)
}

export async function readDoc(
  slug: string,
): Promise<{ meta: DocMeta; html: MdBlock[] } | null> {
  if (!SLUG_RE.test(slug)) return null
  const hit = await getEntry('docs', slug)
  if (!hit) return null
  return {
    meta: {
      slug: hit.slug,
      title: fmString(hit.frontmatter.title, hit.slug),
      description: fmString(hit.frontmatter.description),
      order: Number(hit.frontmatter.order ?? 99),
    },
    html: await mdToHtml(hit.body),
  }
}

/** Minimal XML escaping for the RSS endpoint. */
export function xmlEsc(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

// ---------------------------------------------------------------------------
// Runtime-narrowing helpers for page meta callbacks.
//
// `defineMeta`'s callback receives `data: unknown` -- the framework can't
// connect it to the return type of a separately-declared `defineLoad`. These
// helpers do the `typeof`/`in` narrowing once, in one place, returning the
// typed shape (or defaults). Zero casts: TypeScript 4.9 narrows `data` via
// `'post' in data` to `object & Record<'post', unknown>`.
// ---------------------------------------------------------------------------

export interface SiteMeta {
  title: string
  description: string
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null
}

function readMetaField(data: unknown, field: 'post' | 'doc'): SiteMeta {
  if (!isRecord(data) || !(field in data)) {
    return { title: field === 'post' ? 'Blog' : 'Docs', description: '' }
  }
  const entity = data[field]
  if (!isRecord(entity) || !('meta' in entity)) {
    return { title: field === 'post' ? 'Blog' : 'Docs', description: '' }
  }
  const meta = entity.meta
  if (!isRecord(meta)) {
    return { title: field === 'post' ? 'Blog' : 'Docs', description: '' }
  }
  return {
    title: typeof meta.title === 'string' ? meta.title : (field === 'post' ? 'Blog' : 'Docs'),
    description: typeof meta.description === 'string' ? meta.description : '',
  }
}

/** Extract typed site metadata from a `post`-shaped load result. */
export function readPostMeta(data: unknown): SiteMeta {
  return readMetaField(data, 'post')
}

/** Extract typed site metadata from a `doc`-shaped load result. */
export function readDocMeta(data: unknown): SiteMeta {
  return readMetaField(data, 'doc')
}

