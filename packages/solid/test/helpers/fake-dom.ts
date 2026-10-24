// Hand-rolled fake DOM for adapter client tests (§7 hydrate/update) — no
// jsdom, no deps. Supports the subset the adapters + core client modules
// need: an HTML parser/serializer (elements, attributes, text, comments,
// script/style/template raw text, void elements, basic entities), event
// listeners with dispatch, compound selectors (`tag`, `#id`, `.class`,
// `[attr]`, `[attr="v"]`, `tag.class`, comma lists), `closest`, and
// innerHTML round-trips (the hydration no-mismatch check compares strings).
//
// Each adapter package carries its own copy (identical) per task contract.

export interface FakeNodeCommon {
  parent: FakeElement | null
  ownerDocument: FakeDocument | null
}

export interface FakeText extends FakeNodeCommon {
  kind: 'text'
  text: string
}

export interface FakeComment extends FakeNodeCommon {
  kind: 'comment'
  text: string
}

export type FakeEventListener = (ev: FakeEvent) => void

export interface FakeEvent {
  type: string
  detail?: unknown
  target?: FakeElement
  preventDefault?: () => void
}

export interface FakeElement extends FakeNodeCommon {
  kind: 'element'
  tag: string
  attrs: Record<string, string>
  childNodes: Array<FakeElement | FakeText | FakeComment>
  listeners: Map<string, FakeEventListener[]>
  getAttribute(name: string): string | null
  setAttribute(name: string, value: string): void
  removeAttribute(name: string): void
  hasAttribute(name: string): boolean
  addEventListener(type: string, fn: FakeEventListener): void
  removeEventListener(type: string, fn: FakeEventListener): void
  dispatchEvent(ev: { type: string; detail?: unknown }): boolean
  innerHTML: string
  textContent: string
  querySelector(sel: string): FakeElement | null
  querySelectorAll(sel: string): FakeElement[]
  matches(sel: string): boolean
  closest(sel: string): FakeElement | null
  replaceWith(...nodes: FakeElement[]): void
}

export interface FakeDocument {
  documentElement: FakeElement | null
  head: FakeElement | null
  body: FakeElement | null
  listeners: Map<string, FakeEventListener[]>
  addEventListener(type: string, fn: FakeEventListener): void
  removeEventListener(type: string, fn: FakeEventListener): void
  dispatchEvent(ev: { type: string; detail?: unknown }): boolean
  querySelector(sel: string): FakeElement | null
  querySelectorAll(sel: string): FakeElement[]
  createElement(tag: string): FakeElement
  createTextNode(text: string): FakeText
}

export interface FakeDom {
  doc: FakeDocument
  documentElement: FakeElement | null
  head: FakeElement | null
  body: FakeElement | null
  /** The `#root` element when present (convention for adapter roots). */
  root: FakeElement | null
  /** Serialize the whole document back to HTML. */
  html: string
}

const VOID_TAGS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'])
const RAW_TEXT_TAGS = new Set(['script', 'style'])

const ENTITIES: Record<string, string> = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", '#39': "'" }

function decodeEntities(s: string): string {
  return s.replace(/&(#39|amp|lt|gt|quot|apos);/g, (_, e: string) => ENTITIES[e] ?? `&${e};`)
}

function escapeText(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;')
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

function parseHtml(src: string, doc: FakeDocument | null, parent: FakeElement | null): Array<FakeElement | FakeText | FakeComment> {
  const nodes: Array<FakeElement | FakeText | FakeComment> = []
  let i = 0
  let textStart = 0
  const flushText = (end: number): void => {
    if (end > textStart) {
      const rawText = src.slice(textStart, end)
      nodes.push(makeText(decodeEntities(rawText), doc, parent))
    }
  }
  while (i < src.length) {
    if (src[i] !== '<') { i++; continue }
    // comment
    if (src.startsWith('<!--', i)) {
      flushText(i)
      const end = src.indexOf('-->', i + 4)
      const text = end === -1 ? src.slice(i + 4) : src.slice(i + 4, end)
      nodes.push({ kind: 'comment', text, parent, ownerDocument: doc } as FakeComment)
      i = end === -1 ? src.length : end + 3
      textStart = i
      continue
    }
    // doctype — skip
    if (/^<!doctype/i.test(src.slice(i, i + 9))) {
      const end = src.indexOf('>', i)
      i = end === -1 ? src.length : end + 1
      textStart = i
      continue
    }
    // close tag
    if (src[i + 1] === '/') {
      flushText(i)
      const end = src.indexOf('>', i)
      i = end === -1 ? src.length : end + 1
      textStart = i
      continue
    }
    const tagMatch = /^<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^<>]*?)?)(\/?)>/.exec(src.slice(i))
    if (!tagMatch) { i++; continue } // stray '<' is text
    flushText(i)
    const tag = tagMatch[1]
    const attrsSrc = tagMatch[2]
    if (tag === undefined || attrsSrc === undefined) {
      throw new Error('fake-dom: unexpected tag/attrs match groups')
    }
    const element = makeElement(tag.toLowerCase(), doc, parent)
    const attrRe = /([^\s=/]+)(?:\s*=\s*("([^"]*)"|'([^']*)'|([^\s"'>]*)))?/g
    let m: RegExpExecArray | null
    while ((m = attrRe.exec(attrsSrc.replace(/^\s+/, ''))) !== null) {
      const nameRaw = m[1]
      if (nameRaw === undefined) continue
      const name = nameRaw.toLowerCase()
      const value = m[2] !== undefined ? decodeEntities(m[3] ?? m[4] ?? m[5] ?? '') : ''
      element.attrs[name] = value
    }
    nodes.push(element)
    i += tagMatch[0].length
    textStart = i
    const selfClosing = tagMatch[3] === '/' || VOID_TAGS.has(element.tag)
    if (element.tag === 'template') {
      // template content parses as normal HTML but serializes raw
      const close = src.toLowerCase().indexOf('</template>', i)
      const inner = close === -1 ? src.slice(i) : src.slice(i, close)
      element.childNodes.push(...parseHtml(inner, doc, element))
      i = close === -1 ? src.length : close + '</template>'.length
      textStart = i
    } else if (RAW_TEXT_TAGS.has(element.tag)) {
      const close = src.toLowerCase().indexOf(`</${element.tag}>`, i)
      const inner = close === -1 ? src.slice(i) : src.slice(i, close)
      if (inner) element.childNodes.push(makeText(inner, doc, element))
      i = close === -1 ? src.length : close + element.tag.length + 3
      textStart = i
    } else if (!selfClosing) {
      // find matching close tag with nesting of the same tag name
      let depth = 1
      let j = i
      const openRe = new RegExp(`<${element.tag}(\\s|>|/)`, 'gi')
      const closeRe = new RegExp(`</${element.tag}>`, 'gi')
      openRe.lastIndex = i
      closeRe.lastIndex = i
      while (depth > 0 && j < src.length) {
        const nextOpen = openRe.exec(src)
        const nextClose = closeRe.exec(src)
        if (!nextClose) { j = src.length; break }
        if (nextOpen && nextOpen.index < nextClose.index) {
          depth++
          j = nextOpen.index
        } else {
          depth--
          j = nextClose.index
          if (depth === 0) {
            const inner = src.slice(i, nextClose.index)
            element.childNodes.push(...parseHtml(inner, doc, element))
            i = nextClose.index + nextClose[0].length
            textStart = i
          }
        }
      }
    }
  }
  flushText(src.length)
  return nodes
}

// ---------------------------------------------------------------------------
// Node factories + DOM behavior
// ---------------------------------------------------------------------------

function makeText(text: string, doc: FakeDocument | null, parent: FakeElement | null): FakeText {
  return { kind: 'text', text, parent, ownerDocument: doc }
}

function serializeChildren(el: FakeElement): string {
  let out = ''
  for (const child of el.childNodes) {
    if (child.kind === 'text') {
      out += el.tag === 'script' || el.tag === 'style' || el.tag === 'template' ? child.text : escapeText(child.text)
    } else if (child.kind === 'comment') {
      out += `<!--${child.text}-->`
    } else {
      out += serializeElement(child)
    }
  }
  return out
}

function serializeElement(el: FakeElement): string {
  const attrs = Object.entries(el.attrs)
    .map(([k, v]) => (v === '' ? ` ${k}` : ` ${k}="${escapeAttr(v)}"`))
    .join('')
  if (VOID_TAGS.has(el.tag)) return `<${el.tag}${attrs}>`
  const inner = serializeChildren(el)
  return `<${el.tag}${attrs}>${inner}</${el.tag}>`
}

function matchesCompound(el: FakeElement, sel: string): boolean {
  const m = /^([a-zA-Z][a-zA-Z0-9-]*|\*)?((?:[.#][a-zA-Z0-9_-]+)*)(\[.+?\])?$/.exec(sel.trim())
  if (!m) return false
  const [, tag, idClasses, attrPart] = m
  if (tag && tag !== '*' && el.tag !== tag.toLowerCase()) return false
  if (idClasses) {
    for (const token of idClasses.split(/(?=[.#])/).filter(Boolean)) {
      if (token.startsWith('#')) {
        if (el.attrs.id !== token.slice(1)) return false
      } else {
        if (!(el.attrs.class ?? '').split(/\s+/).includes(token.slice(1))) return false
      }
    }
  }
  if (attrPart) {
    const am = /^\[\s*([^\s=~^|$]+)\s*(?:([~^$|*]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\]]*?))\s*)?\]$/.exec(attrPart)
    if (!am) return false
    const name = am[1]
    if (name === undefined) return false
    const op = am[2]
    const values = am.slice(3)
    const value = values.find((v) => v !== undefined) ?? ''
    if (!op) { if (!(name in el.attrs)) return false }
    else {
      const actual = el.attrs[name.toLowerCase()]
      if (actual === undefined) return false
      if (op === '=') { if (actual !== value) return false }
      else if (op === '~=' || op === '^=' || op === '$=' || op === '*=' || op === '|=') {
        // support the common subset
        if (op === '^=' && !actual.startsWith(value)) return false
        if (op === '*=' && !actual.includes(value)) return false
        if (op === '$=' && !actual.endsWith(value)) return false
        if (op === '~=' && !actual.split(/\s+/).includes(value)) return false
        if (op === '|=' && actual !== value && !actual.startsWith(`${value}-`)) return false
      }
    }
  }
  return true
}

function selectorMatches(el: FakeElement, selector: string): boolean {
  return selector.split(',').some((s) => matchesCompound(el, s))
}

function queryAll(root: FakeElement, selector: string): FakeElement[] {
  const out: FakeElement[] = []
  const visit = (el: FakeElement): void => {
    for (const child of el.childNodes) {
      if (child.kind !== 'element') continue
      if (selectorMatches(child, selector)) out.push(child)
      visit(child)
    }
  }
  visit(root)
  return out
}

function makeElement(tag: string, doc: FakeDocument | null, parent: FakeElement | null): FakeElement {
  const el: FakeElement = {
    kind: 'element',
    tag,
    attrs: {},
    childNodes: [],
    parent,
    ownerDocument: doc,
    listeners: new Map(),
    innerHTML: '',
    textContent: '',
    getAttribute(name) {
      const v = el.attrs[name.toLowerCase()]
      return v === undefined ? null : v
    },
    setAttribute(name, value) {
      el.attrs[name.toLowerCase()] = value
    },
    removeAttribute(name) {
      delete el.attrs[name.toLowerCase()]
    },
    hasAttribute(name) {
      return name.toLowerCase() in el.attrs
    },
    addEventListener(type, fn) {
      const list = el.listeners.get(type) ?? []
      list.push(fn)
      el.listeners.set(type, list)
    },
    removeEventListener(type, fn) {
      el.listeners.set(type, (el.listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    dispatchEvent(ev) {
      const event: FakeEvent = { ...ev, target: el, preventDefault: () => {} }
      for (const fn of el.listeners.get(ev.type) ?? []) fn(event)
      return true
    },
    querySelector(sel) {
      return queryAll(el, sel)[0] ?? null
    },
    querySelectorAll(sel) {
      return queryAll(el, sel)
    },
    matches(sel) {
      return selectorMatches(el, sel)
    },
    closest(sel) {
      let cur: FakeElement | null = el
      while (cur) {
        if (selectorMatches(cur, sel)) return cur
        cur = cur.parent
      }
      return null
    },
    replaceWith(...nodes) {
      if (!el.parent) return
      const idx = el.parent.childNodes.indexOf(el)
      if (idx === -1) return
      el.parent.childNodes.splice(idx, 1, ...nodes)
      for (const n of nodes) {
        n.parent = el.parent
        n.ownerDocument = el.ownerDocument
      }
      el.parent = null
    },
  }
  Object.defineProperty(el, 'innerHTML', {
    get: () => serializeChildren(el),
    set(value: string) {
      el.childNodes = parseHtml(value, el.ownerDocument, el)
    },
  })
  Object.defineProperty(el, 'textContent', {
    get: () => el.childNodes.map((c) => (c.kind === 'text' ? c.text : c.kind === 'element' ? c.textContent : '')).join(''),
    set(value: string) {
      el.childNodes = [makeText(value, el.ownerDocument, el)]
    },
  })
  return el
}

/**
 * Build a fake document from HTML. A full document (`<!doctype html><html>…`)
 * wires documentElement/head/body; fragments go under a synthetic body.
 */
export function createFakeDom(html: string): FakeDom {
  const parsed = parseHtml(html, null, null)
  const doc: FakeDocument = {
    documentElement: null,
    head: null,
    body: null,
    listeners: new Map(),
    addEventListener(type, fn) {
      const list = doc.listeners.get(type) ?? []
      list.push(fn)
      doc.listeners.set(type, list)
    },
    removeEventListener(type, fn) {
      doc.listeners.set(type, (doc.listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    dispatchEvent(ev) {
      for (const fn of doc.listeners.get(ev.type) ?? []) fn({ ...ev, preventDefault: () => {} })
      return true
    },
    querySelector(sel) {
      return queryAllRoot(sel)[0] ?? null
    },
    querySelectorAll(sel) {
      return queryAllRoot(sel)
    },
    createElement(tag) {
      return makeElement(tag.toLowerCase(), doc, null)
    },
    createTextNode(text) {
      return makeText(text, doc, null)
    },
  }
  const rootLevel: Array<FakeElement | FakeText | FakeComment> = []
  const allTop: Array<FakeElement | FakeText | FakeComment> = []
  const docLike = parsed.filter((n): n is FakeElement => n.kind === 'element')
  const htmlEl = docLike.find((n) => n.tag === 'html')
  if (htmlEl) {
    htmlEl.ownerDocument = doc
    for (const child of htmlEl.childNodes) {
      child.ownerDocument = doc
      if (child.kind === 'element') for (const gc of child.childNodes) gc.ownerDocument = doc
    }
    doc.documentElement = htmlEl
    doc.head = htmlEl.childNodes.find((c): c is FakeElement => c.kind === 'element' && c.tag === 'head') ?? null
    doc.body = htmlEl.childNodes.find((c): c is FakeElement => c.kind === 'element' && c.tag === 'body') ?? null
    allTop.push(htmlEl)
  } else {
    const body = makeElement('body', doc, null)
    body.childNodes = parsed
    for (const child of body.childNodes) {
      child.ownerDocument = doc
      child.parent = body
      if (child.kind === 'element') for (const gc of child.childNodes) gc.ownerDocument = doc
    }
    doc.body = body
    rootLevel.push(body)
    allTop.push(body)
  }
  function queryAllRoot(sel: string): FakeElement[] {
    const out: FakeElement[] = []
    for (const top of allTop) {
      if (top.kind === 'element') {
        if (selectorMatches(top, sel)) out.push(top)
        out.push(...queryAll(top, sel))
      }
    }
    return out
  }
  // Wire parent pointers top-down (parseHtml only sets them within a level).
  const wire = (el: FakeElement): void => {
    for (const child of el.childNodes) {
      child.parent = el
      child.ownerDocument = doc
      if (child.kind === 'element') wire(child)
    }
  }
  for (const top of allTop) if (top.kind === 'element') wire(top)
  const root = doc.body?.querySelector('#root') ?? null
  const htmlOut = allTop.map((n) => (n.kind === 'element' ? serializeElement(n) : '')).join('')
  return { doc, documentElement: doc.documentElement, head: doc.head, body: doc.body, root, html: htmlOut }
}

/** Simulate the §7.1 swapper against the fake DOM: move `template[data-ox-c]`
 * content over the matching `template[data-ox-b]` placeholder, exactly like
 * render::SWAPPER_SRC (crates/render/src/lib.rs) does in the browser. */
export function simulateSwap(doc: FakeDocument, resolutionChunkHtml: string): boolean {
  const m = /<template data-ox-c='([^']+)'>([\s\S]*?)<\/template>/.exec(resolutionChunkHtml)
  if (!m) return false
  const id = m[1]
  const contentHtml = m[2]
  if (id === undefined || contentHtml === undefined) return false
  const placeholder = doc.querySelector(`[data-ox-b='${id}']`)
  if (!placeholder) return false
  const holder = doc.createElement('div')
  holder.innerHTML = contentHtml
  placeholder.replaceWith(...holder.childNodes.filter((n): n is FakeElement => n.kind === 'element'))
  doc.dispatchEvent({ type: 'ox:swap', detail: { id: Number(id) } })
  return true
}
