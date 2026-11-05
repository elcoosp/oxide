// Regression tests for src/lib/md.server.ts (bug hunt 2026-11).
//
// The block-level parser used to spin forever (synchronously, at 100% CPU,
// unkillable by request timeouts) on a `>`-without-space line or a stray `|`
// row — any typo in a content file wedged the SSR worker permanently. These
// tests pin the forward-progress guarantee plus the parser's escaping and
// edge-case behavior.
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mdToHtml, xmlEsc } from '../src/lib/md.server.ts'

test('mdToHtml: `>` without a trailing space terminates and renders a blockquote', async () => {
  const out = await mdToHtml('para one\n>quoted no space\npara two')
  assert.deepEqual(out, [
    '<p>para one</p>',
    '<blockquote><p>quoted no space</p></blockquote>',
    '<p>para two</p>',
  ])
})

test('mdToHtml: a bare `>` line terminates', async () => {
  const out = await mdToHtml('text\n>\nmore')
  assert.deepEqual(out, ['<p>text</p>', '<blockquote><p></p></blockquote>', '<p>more</p>'])
})

test('mdToHtml: a stray `|` line (not a table) terminates as literal text', async () => {
  const out = await mdToHtml('| just a pipe line\n\nnext para')
  assert.deepEqual(out, ['<p>| just a pipe line</p>', '<p>next para</p>'])
})

test('mdToHtml: a `|` block without a separator row terminates', async () => {
  // No `|---|` row follows, so this is not a table — the walk must still
  // consume every line instead of spinning forever. Each line degrades to a
  // literal-text paragraph.
  const out = await mdToHtml('| a |\n| b |')
  assert.equal(out.length, 2)
  assert.ok(out[0]?.startsWith('<p>') && out[1]?.startsWith('<p>'))
})

test('mdToHtml: pipe tables still parse after the blockquote fix', async () => {
  const out = await mdToHtml('| a | b |\n|---|---|\n| 1 | 2 |')
  assert.equal(out.length, 1)
  const html = out[0] ?? ''
  assert.ok(html.startsWith('<div class="table-wrap"><table>'), html)
  assert.ok(html.includes('<th>a</th>') && html.includes('<td>2</td>'), html)
})

test('mdToHtml: quote blocks with and without spaces merge like CommonMark', async () => {
  const out = await mdToHtml('> one\n>two')
  assert.deepEqual(out, ['<blockquote><p>one two</p></blockquote>'])
})

test('mdToHtml: inline XSS vectors stay escaped', async () => {
  const out = await mdToHtml('hello <script>alert(1)</script> & "friends"')
  const html = out.join('')
  assert.ok(!html.includes('<script>'), html)
  assert.ok(html.includes('&lt;script&gt;'), html)
  assert.ok(html.includes('&amp;'), html)
})

test('mdToHtml: javascript: hrefs are neutralised', async () => {
  const out = await mdToHtml('[click](javascript:alert(1))')
  const html = out.join('')
  assert.ok(!html.includes('javascript:'), html)
  assert.ok(html.includes('href="#"'), html)
})

test('xmlEsc escapes XML control characters', () => {
  assert.equal(xmlEsc('a<b&c"d'), 'a&lt;b&amp;c&quot;d')
})
