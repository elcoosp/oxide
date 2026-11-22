// The V2-E05 case-schema node --test gate (zero deps, V8-free).
//
// Mirrors the conformance suite's rule: every case file validates against
// the §V2-E05 shape, and the validator itself refuses the malformed shapes
// a hand-authored case could drift into (a typoed hook, a missing version,
// a bad adapter). Red-first: the validator failed before the cases existed.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { ADAPTERS, CASE_HOOKS, STATUSES, loadCase, validateCase } from '../schema.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const casesDir = join(here, '..', 'cases');

test('ADAPTERS lists the four framework adapters', () => {
  assert.deepEqual([...ADAPTERS].sort(), ['react', 'solid', 'svelte', 'vue']);
});

test('CASE_HOOKS carries the camelCase wire names the bridge knows', () => {
  assert.ok(CASE_HOOKS.includes('transform'));
  assert.ok(CASE_HOOKS.includes('resolveId'));
  assert.ok(CASE_HOOKS.includes('configureServer'));
  assert.ok(CASE_HOOKS.includes('cacheHandler'));
});

test('STATUSES lists the three runner statuses', () => {
  assert.deepEqual([...STATUSES].sort(), ['fail', 'pass', 'unsupported-hook']);
});

test('validateCase accepts the spec-verbatim shape', () => {
  const res = validateCase({
    plugin: 'vite-plugin-svgr@4.3.0',
    fixture: 'svg-component',
    adapter: 'react',
    assert: [{ file: 'dist/client/*.js', contains: 'createElement' }],
    hooks: ['transform'],
  });
  assert.equal(res.ok, true);
  assert.equal(res.case.plugin, 'vite-plugin-svgr@4.3.0');
  assert.equal(res.case.adapter, 'react');
});

test('validateCase refuses a plugin without a version', () => {
  const res = validateCase({
    plugin: 'vite-plugin-svgr',
    fixture: 'x',
    adapter: 'react',
    assert: [{ file: 'dist/client/*.js', contains: 'x' }],
    hooks: ['transform'],
  });
  assert.equal(res.ok, false);
  assert.match(res.errors.join('; '), /version/);
});

test('validateCase refuses an unknown adapter', () => {
  const res = validateCase({
    plugin: 'p@1.0.0',
    fixture: 'x',
    adapter: 'preact',
    assert: [{ file: 'dist/client/*.js', contains: 'x' }],
    hooks: ['transform'],
  });
  assert.equal(res.ok, false);
  assert.match(res.errors.join(';'), /adapter/);
});

test('validateCase refuses an unknown hook (a typo, never a silent skip)', () => {
  const res = validateCase({
    plugin: 'p@1.0.0',
    fixture: 'x',
    adapter: 'react',
    assert: [{ file: 'dist/client/*.js', contains: 'x' }],
    hooks: ['transformm'],
  });
  assert.equal(res.ok, false);
  assert.match(res.errors.join(';'), /transformm/);
});

test('validateCase refuses an empty assert array', () => {
  const res = validateCase({
    plugin: 'p@1.0.0',
    fixture: 'x',
    adapter: 'react',
    assert: [],
    hooks: ['transform'],
  });
  assert.equal(res.ok, false);
  assert.match(res.errors.join(';'), /assert/);
});

test('every committed case file validates', () => {
  const files = readdirSync(casesDir).filter((f) => f.endsWith('.json'));
  assert.ok(files.length >= 10, `expected ≥10 cases, found ${files.length}`);
  for (const f of files) {
    const c = loadCase(join(casesDir, f));
    assert.ok(c.plugin.includes('@'), `${f}: plugin missing version`);
    assert.ok(ADAPTERS.includes(c.adapter), `${f}: bad adapter ${c.adapter}`);
    assert.ok(c.hooks.length > 0, `${f}: no hooks`);
  }
});
