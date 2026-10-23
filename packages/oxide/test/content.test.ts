// §16.5 — getCollection / getEntry.
//
// The helper resolves `dist/content/<name>.entries.json` from
// OXIDE_ROOT (or process.cwd() when unset). Tests set OXIDE_ROOT to a
// temp project root and write the entries file there — the same path
// shape a real `oxide build` produces.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  getCollection,
  getEntry,
  __clearContentCache,
} from '../src/server/content.ts'

/** Create a temp project with `dist/content/<name>.entries.json` and
 * point OXIDE_ROOT at it. Returns the root for follow-up mutations. */
async function fixture(name: string, entries: unknown[]) {
  const root = await mkdtemp(join(tmpdir(), 'oxide-content-'))
  await mkdir(join(root, 'dist', 'content'), { recursive: true })
  await writeFile(
    join(root, 'dist', 'content', `${name}.entries.json`),
    JSON.stringify({ name, entries }),
  )
  process.env.OXIDE_ROOT = root
  __clearContentCache()
  return root
}

function unsetRoot() {
  delete process.env.OXIDE_ROOT
}

test('getCollection returns every entry with slug, frontmatter, body', async () => {
  await fixture('blog', [
    { slug: 'hello', frontmatter: { title: 'Hello' }, body: '# Hello\n\nworld', is_mdx: false },
    { slug: 'world', frontmatter: { title: 'World' }, body: '# World', is_mdx: false },
  ])
  const posts = await getCollection('blog')
  assert.equal(posts.length, 2)
  assert.equal(posts[0].slug, 'hello')
  assert.equal(posts[0].frontmatter.title, 'Hello')
  assert.equal(posts[0].body, '# Hello\n\nworld')
  assert.equal(posts[0].is_mdx, false)
  unsetRoot()
})

test('getEntry finds a slug; missing slug is undefined (not a throw)', async () => {
  await fixture('blog', [
    { slug: 'hello', frontmatter: { title: 'Hello' }, body: 'x' },
  ])
  const hit = await getEntry('blog', 'hello')
  assert.ok(hit)
  assert.equal(hit!.slug, 'hello')
  const miss = await getEntry('blog', 'nope')
  assert.equal(miss, undefined, 'missing slug is undefined, not an error')
  unsetRoot()
})

test('getCollection throws a helpful error on a missing collection', async () => {
  const root = await mkdtemp(join(tmpdir(), 'oxide-content-empty-'))
  process.env.OXIDE_ROOT = root
  __clearContentCache()
  await assert.rejects(
    () => getCollection('missing'),
    (e: Error) => {
      assert.match(e.message, /missing\.entries\.json/)
      assert.match(e.message, /content\.collections/)
      return true
    },
  )
  unsetRoot()
})

test('getCollection caches by content hash — a second read is free', async () => {
  await fixture('blog', [{ slug: 'a', frontmatter: {}, body: 'a' }])
  const first = await getCollection('blog')
  const second = await getCollection('blog')
  assert.equal(first, second, 'cache hit returns the same entries array')
  unsetRoot()
})

test('the cache is invalidated on a same-size rewrite', async () => {
  // WP-r9-1: the previous cache key was (mtimeMs, size). Two same-size
  // rewrites within one mtime quantum — or any mtime rounding on tmpfs —
  // served stale entries. This test's two payloads are byte-length-
  // identical by construction (`slug: 'a'` vs `slug: 'b'`, same JSON
  // envelope); the content hash is what must detect the change.
  const root = await fixture('blog', [{ slug: 'a', frontmatter: {}, body: 'a' }])
  const first = await getCollection('blog')
  assert.equal(first.length, 1)
  assert.equal(first[0].slug, 'a')

  const aBytes = JSON.stringify({
    name: 'blog',
    entries: [{ slug: 'a', frontmatter: {}, body: 'a' }],
  })
  const bBytes = JSON.stringify({
    name: 'blog',
    entries: [{ slug: 'b', frontmatter: {}, body: 'b' }],
  })
  assert.equal(
    aBytes.length,
    bBytes.length,
    'premise: the two payloads are byte-length-identical',
  )

  await writeFile(join(root, 'dist', 'content', 'blog.entries.json'), bBytes)
  const second = await getCollection('blog')
  assert.equal(second[0].slug, 'b', 'same-size rewrite is not served stale')
  unsetRoot()
})

test('OXIDE_ROOT resolution works with process.cwd() elsewhere', async () => {
  // A worker's cwd is inherited from the parent shell, not from the
  // --root flag; OXIDE_ROOT is the reliable answer. This test proves
  // the helper uses it.
  const root = await fixture('blog', [
    { slug: 'a', frontmatter: {}, body: 'a' },
  ])
  const previous = process.cwd()
  process.chdir(tmpdir()) // deliberately elsewhere
  try {
    const entries = await getCollection('blog')
    assert.equal(entries.length, 1, 'OXIDE_ROOT beats process.cwd()')
  } finally {
    process.chdir(previous)
    unsetRoot()
  }
})
