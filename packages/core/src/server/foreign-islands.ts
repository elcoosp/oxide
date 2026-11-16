// Foreign-island resolution (§V2-F01 item 4): a core stream transform
// that wraps the page adapter's render stream, finds the foreign
// island's self-contained placeholder
// (`<ox-island data-ox-island='Price' data-ox-fw='vue'
// data-ox-pending='1' data-ox-props='...'></ox-island>`), renders each
// with the island's own adapter and splices the HTML in before the
// chunk is flushed. The pending flag is stripped from the resolved
// output (the client reads `data-ox-hydrated`, and a stale pending
// flag would mislead tooling).

// The pending flag matches BOTH quote styles: the render crate's own
// element emits single-quoted attributes, while the adapters' `<Island>`
// twins (§V2-F01) emit the framework SSR's native double quotes (React 19,
// Vue 3, Svelte 5 and Solid SSR all render `attr="value"`). The captured
// attribute sides carry the original quote style through the splice
// unchanged.
const RE = /<ox-island([^>]*)data-ox-pending=(['"])1\2([^>]*)><\/ox-island>/g

export type ForeignRender = (fw: string, name: string, propsB64: string) => Promise<string>

/** Wrap `src`: every pending placeholder becomes the island's own
 *  SSR output. A placeholder split across chunks is held back (the
 *  unfinished `<ox-island` tag) so the regex always sees it whole. */
export function resolveForeign(
  src: ReadableStream<Uint8Array>,
  render: ForeignRender,
): ReadableStream<Uint8Array> {
  const dec = new TextDecoder()
  const enc = new TextEncoder()
  let carry = ''
  return src.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      async transform(chunk, ctl) {
        let s = carry + dec.decode(chunk, { stream: true })
        // Hold back an unfinished placeholder so a split across chunks
        // is still seen whole: either a found '<ox-island' with no
        // close yet, or a trailing PARTIAL prefix of the tag itself
        // (a cut that lands mid-`<ox-island`).
        let cut = -1
        const open = s.lastIndexOf('<ox-island')
        if (open >= 0 && s.indexOf('</ox-island>', open) < 0) {
          cut = open
        } else if (!s.includes('</ox-island>')) {
          const TAG = '<ox-island'
          for (let l = Math.min(TAG.length - 1, s.length); l >= 1; l--) {
            if (TAG.startsWith(s.slice(-l))) {
              cut = s.length - l
              break
            }
          }
        }
        carry = cut >= 0 ? s.slice(cut) : ''
        if (cut >= 0) s = s.slice(0, cut)
        const parts: string[] = []
        let last = 0
        let m: RegExpExecArray | null
        RE.lastIndex = 0
        while ((m = RE.exec(s))) {
          parts.push(s.slice(last, m.index))
          // Group 2 is the pending attr's quote (the new capture); the
          // attribute sides are groups 1 and 3.
          const attrs = m[1] + m[3]
          const get = (k: string) => new RegExp(k + "=([\"'])([^\"']*)\\1").exec(attrs)?.[2] ?? ''
          const html = await render(get('data-ox-fw'), get('data-ox-island'), get('data-ox-props'))
          // The pending flag sat BETWEEN the two captured sides — the
          // match consumed it, so the splice collapses the seam it left
          // behind (the before-side's trailing space against the
          // after-side's leading space) into the single separator the
          // resolved island carries. A pending flag that rode inside a
          // captured side is stripped too (either quote style).
          const resolved = `${m[1].trimEnd()} ${m[3].trimStart()}`.replace(
            /\s*data-ox-pending=(['"])1\1\s*/,
            ' ',
          )
          parts.push('<ox-island' + resolved + '>' + html + '</ox-island>')
          last = m.index + m[0].length
        }
        parts.push(s.slice(last))
        ctl.enqueue(enc.encode(parts.join('')))
      },
      flush(ctl) {
        if (carry) ctl.enqueue(enc.encode(carry))
      },
    }),
  )
}
