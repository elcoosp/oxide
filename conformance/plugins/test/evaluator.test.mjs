// The V2-E05 assert-evaluator node --test gate (zero deps, V8-free).
//
// The evaluator is the half the dry-run exercises against a recorded dist
// tree; these pins hold the glob + contains contract without a single
// install or build.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { evaluateAssert, evaluateCase, globFiles, globToRegex } from '../evaluator.mjs';

test('globToRegex matches a single-segment star', () => {
  const re = globToRegex('dist/client/*.js');
  assert.ok(re.test('dist/client/index.js'));
  assert.ok(re.test('dist/client/app.js'));
  assert.ok(!re.test('dist/client/nested/index.js'), '* stops at a slash');
  assert.ok(!re.test('dist/client/index.css'));
});

test('globToRegex matches a double-star across segments', () => {
  const re = globToRegex('**/*.css');
  assert.ok(re.test('dist/client/app.css'));
  assert.ok(re.test('dist/client/nested/a.css'));
});

test('globFiles walks a recorded dist tree and selects the glob', () => {
  const root = mkdtempSync(join(tmpdir(), 'oxide-eval-'));
  try {
    mkdirSync(join(root, 'dist', 'client'), { recursive: true });
    writeFileSync(join(root, 'dist', 'client', 'index.js'), 'export const x = 1');
    writeFileSync(join(root, 'dist', 'client', 'app.js'), 'createElement("svg")');
    writeFileSync(join(root, 'dist', 'client', 'style.css'), 'body{}');
    const js = globFiles(root, 'dist/client/*.js');
    assert.deepEqual(js, ['dist/client/app.js', 'dist/client/index.js']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('globFiles returns [] for a missing root (a recorded fail, never a throw)', () => {
  assert.deepEqual(globFiles(join(tmpdir(), 'oxide-nope-' + process.pid), '*'), []);
});

test('evaluateAssert passes when the marker is present', () => {
  const root = mkdtempSync(join(tmpdir(), 'oxide-eval-'));
  try {
    mkdirSync(join(root, 'dist', 'client'), { recursive: true });
    writeFileSync(join(root, 'dist', 'client', 'index.js'), 'const v = createElement("x");');
    const r = evaluateAssert(root, { file: 'dist/client/*.js', contains: 'createElement' });
    assert.equal(r.ok, true);
    assert.deepEqual(r.matched, ['dist/client/index.js']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('evaluateAssert fails when the glob matches but the marker is absent', () => {
  const root = mkdtempSync(join(tmpdir(), 'oxide-eval-'));
  try {
    mkdirSync(join(root, 'dist', 'client'), { recursive: true });
    writeFileSync(join(root, 'dist', 'client', 'index.js'), 'export const x = 1');
    const r = evaluateAssert(root, { file: 'dist/client/*.js', contains: 'createElement' });
    assert.equal(r.ok, false);
    assert.match(r.reason, /not found/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('evaluateAssert fails when the glob matches no files', () => {
  const root = mkdtempSync(join(tmpdir(), 'oxide-eval-'));
  try {
    const r = evaluateAssert(root, { file: 'dist/client/*.js', contains: 'x' });
    assert.equal(r.ok, false);
    assert.match(r.reason, /matched no files/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('evaluateCase runs every assert and folds the verdict', () => {
  const root = mkdtempSync(join(tmpdir(), 'oxide-eval-'));
  try {
    mkdirSync(join(root, 'dist', 'client'), { recursive: true });
    writeFileSync(join(root, 'dist', 'client', 'index.js'), 'createElement("svg"); _createVNode();');
    const res = evaluateCase(
      {
        plugin: 'p@1.0.0',
        fixture: 'x',
        adapter: 'react',
        assert: [
          { file: 'dist/client/*.js', contains: 'createElement' },
          { file: 'dist/client/*.js', contains: 'nope' },
        ],
        hooks: ['transform'],
      },
      root,
    );
    assert.equal(res.ok, false);
    assert.equal(res.results.length, 2);
    assert.equal(res.results[0].ok, true);
    assert.equal(res.results[1].ok, false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
