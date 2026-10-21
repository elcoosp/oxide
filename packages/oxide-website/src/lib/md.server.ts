// src/lib/md.server.ts — server-only content helpers for the site.
//
// Why a mini renderer instead of markdown imports? Oxide's build-time
// content collections already scan/validate/index these files (see
// `content.collections` in oxide.config.ts), but the runtime query surface
// (`content.blog.all()` etc.) is still being wired, so the loaders read the
// .md files directly. This module is the interim glue — when the typed
// query surface lands, it shrinks to a thin wrapper.
//
// Supported markdown subset (keep posts inside it): headings #..####,
// paragraphs, -/* and 1. lists, pipe tables, > blockquotes, ``` fences,
// --- rules, inline `code`, **bold**, [links](https://… or /relative).
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const ROOT = process.cwd()
const BLOG_DIR = join(ROOT, 'content', 'blog')
const DOCS_DIR = join(ROOT, 'content', 'docs')

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

function parseFrontmatter(src: string): { fm: Frontmatter; body: string } {
  const text = src.replace(/\r\n/g, '\n')
  if (!text.startsWith('---\n')) return { fm: {}, body: text }
  const end = text.indexOf('\n---', 3)
  if (end === -1) return { fm: {}, body: text }
  const head = text.slice(4, end)
  const body = text.slice(end + 4).replace(/^\n+/, '')
  const fm: Frontmatter = {}
  for (const line of head.split('\n')) {
    const m = /^([A-Za-z][\w-]*)\s*:\s*(.*)$/.exec(line)
    if (m) fm[m[1]] = m[2].trim()
  }
  return { fm, body }
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

function readDirMd(dir: string): { slug: string; fm: Frontmatter; body: string }[] {
  let files: string[] = []
  try {
    files = readdirSync(dir)
  } catch {
    return []
  }
  const entries: { slug: string; fm: Frontmatter; body: string }[] = []
  for (const f of files) {
    if (!f.endsWith('.md') || f.startsWith('_')) continue
    const slug = f.slice(0, -3)
    if (!SLUG_RE.test(slug)) continue
    const { fm, body } = parseFrontmatter(readFileSync(join(dir, f), 'utf8'))
    entries.push({ slug, fm, body })
  }
  return entries
}

export function listPosts(): PostMeta[] {
  return readDirMd(BLOG_DIR)
    .map(({ slug, fm }) => ({
      slug,
      title: fm.title ?? slug,
      description: fm.description ?? '',
      date: fm.date ?? '',
      tags: parseTags(fm.tags),
    }))
    .sort((a, b) => (a.date < b.date ? 1 : -1))
}

export function readPost(slug: string): { meta: PostMeta; html: string } | null {
  if (!SLUG_RE.test(slug)) return null
  const hit = readDirMd(BLOG_DIR).find((e) => e.slug === slug)
  if (!hit) return null
  return {
    meta: {
      slug: hit.slug,
      title: hit.fm.title ?? hit.slug,
      description: hit.fm.description ?? '',
      date: hit.fm.date ?? '',
      tags: parseTags(hit.fm.tags),
    },
    html: mdToHtml(hit.body),
  }
}

export function listDocs(): DocMeta[] {
  return readDirMd(DOCS_DIR)
    .map(({ slug, fm }) => ({
      slug,
      title: fm.title ?? slug,
      description: fm.description ?? '',
      order: Number(fm.order ?? 99),
    }))
    .sort((a, b) => a.order - b.order)
}

export function readDoc(slug: string): { meta: DocMeta; html: string } | null {
  if (!SLUG_RE.test(slug)) return null
  const hit = readDirMd(DOCS_DIR).find((e) => e.slug === slug)
  if (!hit) return null
  return {
    meta: {
      slug: hit.slug,
      title: hit.fm.title ?? hit.slug,
      description: hit.fm.description ?? '',
      order: Number(hit.fm.order ?? 99),
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
