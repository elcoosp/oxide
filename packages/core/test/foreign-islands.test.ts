// Foreign-island stream transform tests (§V2-F01 item 4): the core
// `resolveForeign` wrapper finds `data-ox-pending='1'` placeholders,
// renders each with the island's own adapter and splices the HTML in
// before the chunk is flushed — including a placeholder split across
// chunks, exercised at EVERY byte offset.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { resolveForeign } from '../src/server/foreign-islands.ts'

async function collect(src: ReadableStream<Uint8Array>): Promise<string> {
  const dec = new TextDecoder()
  let out = ''
  for await (const chunk of src) out += dec.decode(chunk, { stream: true })
  return out
}

function streamOf(text: string, splitAt?: number[]): ReadableStream<Uint8Array> {
  const bytes = new TextEncoder().encode(text)
  const chunks: Uint8Array[] = []
  if (!splitAt) {
    chunks.push(bytes)
  } else {
    let last = 0
    for (const cut of splitAt) {
      chunks.push(bytes.slice(last, cut))
      last = cut
    }
    chunks.push(bytes.slice(last))
  }
  let i = 0
  return new ReadableStream({
    pull(ctl) {
      const c = chunks[i]
      i += 1
      if (c === undefined) ctl.close()
      else ctl.enqueue(c)
    },
  })
}

const PLACEHOLDER =
  "<ox-island data-ox-island='Price' data-ox-fw='vue' data-ox-pending='1' data-ox-props='eyJhIjoxfQ=='></ox-island>"

test('resolveForeign splices the rendered html and drops the pending flag', async () => {
  const src = streamOf(`<main>${PLACEHOLDER}</main>`)
  const out = await collect(
    resolveForeign(src, async (fw, name, propsB64) => {
      assert.equal(fw, 'vue')
      assert.equal(name, 'Price')
      assert.equal(propsB64, 'eyJhIjoxfQ==')
      return '<span>rendered</span>'
    }),
  )
  assert.equal(
    out,
    '<main><ox-island data-ox-island=\'Price\' data-ox-fw=\'vue\' data-ox-props=\'eyJhIjoxfQ==\'><span>rendered</span></ox-island></main>',
  )
  assert.ok(!out.includes('data-ox-pending'), 'the pending flag is stripped from the resolved island')
  assert.ok(!/<\/ox-island><\/ox-island>/.test(out), 'the placeholder shell is not duplicated')
})

test('resolveForeign leaves non-pending islands untouched', async () => {
  const hydrated = "<ox-island data-ox-island='Counter' data-ox-fw='react'><b>already</b></ox-island>"
  let calls = 0
  const out = await collect(resolveForeign(streamOf(hydrated), async () => {
    calls += 1
    return 'nope'
  }))
  assert.equal(out, hydrated)
  assert.equal(calls, 0)
})

test('resolveForeign renders a placeholder split at every byte offset (§V2-F01)', async () => {
  const text = `<p>before</p>${PLACEHOLDER}<p>after</p>`
  let whole = ''
  let callsWhole = 0
  const expected = await collect(
    resolveForeign(streamOf(text), async (fw, name, props) => {
      callsWhole += 1
      return `<r fw=${fw} name=${name} props=${props} />`
    }),
  )
  void whole
  assert.equal(callsWhole, 1)

  for (let cut = 1; cut < text.length; cut++) {
    let calls = 0
    const out = await collect(
      resolveForeign(streamOf(text, [cut]), async (fw, name, props) => {
        calls += 1
        return `<r fw=${fw} name=${name} props=${props} />`
      }),
    )
    assert.equal(out, expected, `split at byte ${cut}`)
    assert.equal(calls, 1, `render ran once for the split at byte ${cut}`)
  }
})

test('resolveForeign flushes a trailing carry (the split placeholder at the end)', async () => {
  const text = `x${PLACEHOLDER}`
  for (let cut = 1; cut < text.length; cut++) {
    const out = await collect(resolveForeign(streamOf(text, [cut]), async () => '<r/>'))
    assert.ok(out.includes('<r/>'), `split at ${cut} still renders`)
    assert.ok(out.endsWith('</ox-island>'), `split at ${cut} ends with the island shell`)
  }
})
