// Neutral element-tree IR (spec §7 adapter contract). The adapter IS the
// renderer over this tree while framework libs are absent: components are
// plain functions (props) => Node, rendered recursively. Each adapter package
// owns a copy of this file; the shape is identical across the four so the
// conformance suite (§22) can drive them uniformly.
//
// Escaping/hashing helpers live here too — framework flavors (vue
// `data-v-<scope>`, svelte `svelte-<hash>` classes, solid `data-hk` keys)
// derive their scope ids from the deterministic FNV-1a hash.

/** A component: a plain function from props to a Node (§7 component model). */
export type ComponentFn = (props: Record<string, unknown>) => Node

/** The tree kinds: fragment / element / text / raw / defer / island. */
export type Node = FragmentNode | ElementNode | TextNode | RawNode | DeferNode | IslandNode

/** A fragment: children rendered without a wrapper (React `<></>` analog). */
export interface FragmentNode {
  kind: 'fragment'
  children: Node[]
}

export interface ElementNode {
  kind: 'element'
  /** HTML tag name, or a component function (rendered recursively). */
  tag: string | ComponentFn
  /** Literal attribute names (attribute names are NOT JSX-magified). */
  props: Record<string, unknown>
  children: Node[]
}

export interface TextNode {
  kind: 'text'
  value: string
}

/** Pre-escaped HTML inserted verbatim (protocol templates, JSON-LD). */
export interface RawNode {
  kind: 'raw'
  html: string
}

/**
 * A deferred boundary (§7.1). `value` is the promise-like from the load data
 * (RenderInput data may carry promise-likes per packages/oxide types.ts).
 * The shell renders `fallback`; when `value` resolves the adapter streams a
 * `template[data-ox-c]` resolution + swapper (see src/protocol.ts).
 * `render` maps the resolved value into the boundary's final tree.
 */
export interface DeferNode {
  kind: 'defer'
  value: DeferSource
  fallback: Node
  render: (resolved: unknown) => Node
}

export type DeferSource =
  | PromiseLike<unknown>
  | { __oxide_defer: true; promise: PromiseLike<unknown> }

/** Islands mode host (§10.3): <ox-island> with props + chunk reference. */
export interface IslandNode {
  kind: 'island'
  component: string
  chunk: string
  strategy: 'load' | 'idle' | 'visible' | `media(${string})`
  props: unknown
  children: Node
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/** Element or component node. `el(Component, props, ...children)` renders the
 * component recursively; `el('div', props, ...children)` is a plain element. */
export function el(
  tag: string | ComponentFn,
  props?: Record<string, unknown> | null,
  ...children: Array<Node | Node[] | null | undefined | false | ''>
): Node {
  return {
    kind: 'element',
    tag,
    props: props ?? {},
    children: children.flat().filter((c): c is Node => Boolean(c)),
  }
}

/** Fragment node: children render unwrapped (layouts compose via fragments). */
export function frag(...children: Array<Node | Node[] | null | undefined | false | ''>): Node {
  return { kind: 'fragment', children: children.flat().filter((c): c is Node => Boolean(c)) }
}

/** Text node — stringified and escaped at render time. */
export function t(value: unknown): Node {
  return { kind: 'text', value: String(value) }
}

/** Pre-escaped HTML (use only for trusted/protocol strings). */
export function raw(html: string): Node {
  return { kind: 'raw', html }
}

/** Deferred boundary (§7.1). Accepts a promise or the runtime `defer()` marker. */
export function deferred(
  value: DeferSource,
  fallback: Node,
  render: (resolved: unknown) => Node,
): Node {
  return { kind: 'defer', value, fallback, render }
}

/** Islands host node (§10.3); rendered with the exact render-crate shape. */
export function island(
  opts: { component: string; chunk: string; strategy: IslandNode['strategy']; props: unknown },
  children: Node,
): Node {
  return { kind: 'island', ...opts, children }
}

/** Unwrap the runtime `defer()` marker to its promise. */
export function deferPromise(source: DeferSource): PromiseLike<unknown> {
  if (source && typeof source === 'object' && (source as { __oxide_defer?: unknown }).__oxide_defer === true) {
    return (source as { promise: PromiseLike<unknown> }).promise
  }
  return source as PromiseLike<unknown>
}

// ---------------------------------------------------------------------------
// Escaping + deterministic hashing
// ---------------------------------------------------------------------------

const TEXT_ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;' }
const ATTR_ESCAPES: Record<string, string> = { '&': '&amp;', '"': '&quot;', '<': '&lt;' }

/** Escape element text (`& < >`). */
export function escapeText(s: string): string {
  return s.replace(/[&<>]/g, (c) => TEXT_ESCAPES[c] as string)
}

/** Escape attribute values (`& " <`). */
export function escapeAttr(s: string): string {
  return s.replace(/[&"<]/g, (c) => ATTR_ESCAPES[c] as string)
}

/** `\\u003c` escaping inside script payloads (parity with render crate). */
export function escapeScript(s: string): string {
  return s.replace(/</g, '\\u003c')
}

/**
 * Deterministic 6-hex FNV-1a — scope/hash ids for framework flavors
 * (vue `data-v-<scope>`, svelte `svelte-<hash>`, solid hydration keys).
 * Derived from stable inputs (route ids) so markup is deterministic
 * (§7.2 "deterministic hydration markup").
 */
export function hash6(input: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i)
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return h.toString(16).padStart(8, '0').slice(0, 6)
}

// Base64 helpers live in the shared `oxide` package (single implementation
// across every adapter); re-exported here so existing `from './ir.ts'`
// imports keep working.
export { toBase64, fromBase64 } from '../../oxide/src/bytes.ts'
