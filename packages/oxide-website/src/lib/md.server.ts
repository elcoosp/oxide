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
import { createHighlighter } from 'shiki'


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

function safeHref(href: string): string {
  return /^(https?:\/\/|\/|#|mailto:)/.test(href) ? href : '#'
}

/** Inline markdown → HTML. Input is raw; output is escaped-safe. */
function inline(s: string): string {
  let out = esc(s)
  // stash inline code first so other rules can't touch it
  const codes: string[] = []
  out = out.replace(/`([^`]+)`/g, (_m, c: string) => {
    codes.push(c)
    return `\u0000${codes.length - 1}\u0000`
  })
  out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
  out = out.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_m, text: string, href: string) => {
    return `<a href="${safeHref(href)}">${text}</a>`
  })
  out = out.replace(/\u0000(\d+)\u0000/g, (_m, i: string) => `<code>${codes[Number(i)]}</code>`)
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

// Setup Shiki once per build session
let shikiPromise: Promise<any> | null = null
function getHighlighter() {
  if (!shikiPromise) {
    shikiPromise = createHighlighter({
      themes: [
        {
          name: 'oxide',
          type: 'dark',
          colors: {
            'editor.background': '#09090b',
            'editor.foreground': '#fafafa',
          },
          tokenColors: [
            { scope: ['keyword', 'storage.type', 'storage.modifier', 'keyword.control'], settings: { foreground: '#e45631' } },
            { scope: ['string', 'constant.numeric', 'constant.language'], settings: { foreground: '#a1a1aa' } },
            { scope: ['comment'], settings: { foreground: '#52525b', fontStyle: 'italic' } },
            { scope: ['entity.name.function', 'support.function', 'entity.name.tag'], settings: { foreground: '#fafafa' } },
            { scope: ['variable', 'variable.parameter', 'variable.other'], settings: { foreground: '#d4d4d8' } },
            { scope: ['entity.name.type', 'support.type', 'support.class'], settings: { foreground: '#fafafa' } },
            { scope: ['punctuation', 'meta.brace'], settings: { foreground: '#a1a1aa' } }
          ]
        }
      ],
      langs: ['tsx', 'vue', 'svelte', 'html', 'typescript', 'bash']
    })
  }
  return shikiPromise
}

export async function mdToHtml(md: string): Promise<string> {
  const lines = md.replace(/\r\n/g, '\n').split('\n')
  const out: string[] = []
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
        // Sections delimited by ---react / ---vue / ---svelte / ---solid.
        // Emit static HTML: React sample rendered server-side so a
        // no-JS reader sees real code, all four carried as JSON in a
        // data attribute for the bootstrap island to build the tabs from.
        const parts: Partial<Record<'react' | 'vue' | 'svelte' | 'solid', string>> = {}
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
            cur = mustEl(m, 1) as 'react' | 'vue' | 'svelte' | 'solid'
            continue
          }
          if (cur) sec.push(l)
        }
        flush()

        const LANGS = ['react', 'vue', 'svelte', 'solid'] as const
        const missing = LANGS.filter((k) => typeof parts[k] !== 'string' || parts[k] === '')
        if (missing.length > 0) {
          out.push(`<p class="fw-code-error">framework-tabs: missing section(s): ${missing.join(', ')}</p>`)
          continue
        }

        const highlighter = await getHighlighter()
        const highlighted: Record<string, string> = {}
        for (const lang of LANGS) {
          const shikiLang = lang === 'react' ? 'tsx' : lang === 'solid' ? 'tsx' : lang
          highlighted[lang] = highlighter.codeToHtml(parts[lang] ?? '', { lang: shikiLang, theme: 'oxide' })
        }

        const json = JSON.stringify(highlighted)
          .replace(/&/g, '&amp;')
          .replace(/"/g, '&quot;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')

        out.push(
          `<div class="fw-code" data-fw-snippets="${json}">` +
            highlighted.react +
          `</div>`
        )
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
    if (line.startsWith('> ')) {
      const buf: string[] = []
      while (i < lines.length && mustEl(lines, i).startsWith('> ')) buf.push(mustEl(lines, i++).slice(2))
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
    out.push(`<p>${para.map(inline).join(' ')}</p>`)
  }
  return out.join('\n')
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
    .sort((a, b) => (a.date < b.date ? 1 : -1))
}

export async function readPost(
  slug: string,
): Promise<{ meta: PostMeta; html: string } | null> {
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
): Promise<{ meta: DocMeta; html: string } | null> {
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

