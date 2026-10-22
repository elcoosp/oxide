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
import { getCollection, getEntry } from 'oxide/server'

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

/** Markdown block subset → HTML string. */
export function mdToHtml(md: string): string {
  const lines = md.replace(/\r\n/g, '\n').split('\n')
  const out: string[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (line.startsWith('```')) {
      const buf: string[] = []
      i++
      while (i < lines.length && !lines[i].startsWith('```')) buf.push(lines[i++])
      i++ // closing fence
      out.push(`<pre><code>${esc(buf.join('\n'))}</code></pre>`)
      continue
    }
    const h = /^(#{1,4})\s+(.*)$/.exec(line)
    if (h) {
      const level = h[1].length
      out.push(`<h${level}>${inline(h[2])}</h${level}>`)
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
      while (i < lines.length && lines[i].startsWith('> ')) buf.push(lines[i++].slice(2))
      out.push(`<blockquote><p>${buf.map(inline).join(' ')}</p></blockquote>`)
      continue
    }
    if (/^[-*]\s+/.test(line)) {
      const items: string[] = []
      while (i < lines.length && /^[-*]\s+/.test(lines[i])) items.push(lines[i++].replace(/^[-*]\s+/, ''))
      out.push(`<ul>${items.map((it) => `<li>${inline(it)}</li>`).join('')}</ul>`)
      continue
    }
    if (/^\d+[.)]\s+/.test(line)) {
      const items: string[] = []
      while (i < lines.length && /^\d+[.)]\s+/.test(lines[i])) items.push(lines[i++].replace(/^\d+[.)]\s+/, ''))
      out.push(`<ol>${items.map((it) => `<li>${inline(it)}</li>`).join('')}</ol>`)
      continue
    }
    if (
      line.trim().startsWith('|') &&
      i + 1 < lines.length &&
      /^\|[\s:|-]+\|?\s*$/.test(lines[i + 1].trim())
    ) {
      const head = splitRow(line)
      i += 2
      const rows: string[][] = []
      while (i < lines.length && lines[i].trim().startsWith('|')) rows.push(splitRow(lines[i++]))
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
      lines[i].trim() !== '' &&
      !/^(#{1,4}\s|```|>|[-*]\s|\d+[.)]\s|\|)/.test(lines[i])
    ) {
      para.push(lines[i++])
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
    html: mdToHtml(hit.body),
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
    html: mdToHtml(hit.body),
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
