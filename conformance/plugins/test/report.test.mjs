// The V2-E05 report-parser + matrix-renderer node --test gate (zero deps,
// V8-free).
//
// `parseReport` reads the bridge's load-time emission from a build log —
// both the structured `oxide-plugin-load` line (the WASM host + the
// registered-registry report) and the existing `PLG-HOOK` fail-at-load
// messages the JS bridge raises for `configureServer`/`cacheHandler`.
// `renderMatrix` renders the verified-plugins table the generated
// `docs/plugin-compat.md` embeds.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { parseReport, renderMatrix, tallyAsserts } from '../report.mjs';

test('parseReport reads the structured oxide-plugin-load line', () => {
  const log = [
    'oxide build: starting',
    'oxide-plugin-load\t{"plugin":"vite-plugin-svgr","kind":"js","unsupportedHooks":[],"wiredHooks":["transform","load","resolveId"]}',
    'oxide build: done',
  ].join('\n');
  const r = parseReport(log);
  assert.deepEqual(Object.keys(r), ['vite-plugin-svgr']);
  assert.equal(r['vite-plugin-svgr'].kind, 'js');
  assert.equal(r['vite-plugin-svgr'].loaded, true);
  assert.deepEqual(r['vite-plugin-svgr'].unsupportedHooks, []);
  assert.deepEqual(r['vite-plugin-svgr'].wiredHooks, ['transform', 'load', 'resolveId']);
});

test('parseReport reads the JS bridge PLG-HOOK fail-at-load arm', () => {
  // The existing emission — reused, not re-emitted: a plugin declaring
  // `configureServer` fails at load with a message naming the hook.
  const log =
    'plugin `vite-plugin-pwa` declares hook `configureServer`, which the JS bridge does not support — see the compatibility table (§15.2/§15.3)';
  const r = parseReport(log);
  assert.equal(r['vite-plugin-pwa'].loaded, false);
  assert.deepEqual(r['vite-plugin-pwa'].unsupportedHooks, ['configureServer']);
});

test('parseReport merges multiple PLG-HOOK messages for one plugin', () => {
  const log = [
    'plugin `p` declares hook `configureServer`, which the JS bridge does not support',
    'plugin `p` declares hook `cacheHandler`, which the JS bridge does not support',
  ].join('\n');
  const r = parseReport(log);
  assert.deepEqual(r.p.unsupportedHooks.sort(), ['cacheHandler', 'configureServer']);
});

test('parseReport returns {} for a log with no report', () => {
  assert.deepEqual(parseReport('oxide build: nothing to see'), {});
});

test('renderMatrix renders the verified-plugins table deterministically', () => {
  const a = renderMatrix([
    {
      plugin: 'vite-plugin-svgr',
      version: '4.3.0',
      adapter: 'react',
      status: 'pass',
      unsupportedHooks: [],
      asserts: { passed: 1, failed: 0 },
    },
    {
      plugin: 'vite-plugin-pwa',
      version: '2.0.0',
      adapter: 'react',
      status: 'unsupported-hook',
      unsupportedHooks: ['configureServer'],
      asserts: { passed: 0, failed: 1 },
      note: 'configureServer is bridge-unsupported (PLG-HOOK); the SW registration never lands',
    },
  ]);
  // sorted by plugin name, header + two rows.
  const lines = a.split('\n');
  assert.equal(lines[0], '| Plugin | Version | Adapter | Status | Unsupported hooks | Asserts | Note |');
  assert.equal(lines[1], '| --- | --- | --- | --- | --- | --- | --- |');
  assert.ok(lines[2].includes('`vite-plugin-pwa`'), 'pwa sorts before svgr');
  assert.ok(lines[2].includes('unsupported-hook'));
  assert.ok(lines[2].includes('configureServer'));
  assert.ok(lines[3].includes('`vite-plugin-svgr`'));
  assert.ok(lines[3].includes('pass'));
  // determinism: same input → same bytes.
  assert.equal(a, renderMatrix([
    {
      plugin: 'vite-plugin-svgr',
      version: '4.3.0',
      adapter: 'react',
      status: 'pass',
      unsupportedHooks: [],
      asserts: { passed: 1, failed: 0 },
    },
    {
      plugin: 'vite-plugin-pwa',
      version: '2.0.0',
      adapter: 'react',
      status: 'unsupported-hook',
      unsupportedHooks: ['configureServer'],
      asserts: { passed: 0, failed: 1 },
      note: 'configureServer is bridge-unsupported (PLG-HOOK); the SW registration never lands',
    },
  ]));
});

test('renderMatrix escapes a pipe in a note', () => {
  const a = renderMatrix([
    {
      plugin: 'p',
      version: '1.0.0',
      adapter: 'react',
      status: 'fail',
      unsupportedHooks: [],
      asserts: { passed: 0, failed: 1 },
      note: 'a | b',
    },
  ]);
  assert.ok(a.includes('a \\| b'), 'pipes in notes are escaped');
});

test('tallyAsserts counts passed + failed', () => {
  assert.deepEqual(tallyAsserts([{ ok: true }, { ok: false }, { ok: true }]), {
    passed: 2,
    failed: 1,
  });
});
