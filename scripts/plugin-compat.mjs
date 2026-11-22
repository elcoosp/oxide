#!/usr/bin/env node
// The V2-E05 plugin-compat runner (spec v0.2 §V2-E05).
//
// For each case under `conformance/plugins/cases/<name>.json`:
//   - install the plugin into a TEMP COPY of its fixture,
//   - build with `oxide build` (oxide-cli),
//   - evaluate every `assert` against the emitted `dist/`,
//   - record `{ status, unsupportedHooks }` from the bridge's load-time
//     report (parsed from the build log).
//
// `--dry-run` evaluates RECORDED build logs + a recorded dist tree
// (`conformance/plugins/logs/<name>/`) so the assert evaluator + the report
// parser + the matrix render run V8-free with zero installs — the same
// node-pool-free rule the §22 conformance registry follows. The ten-plugin
// install+build itself is a WRITTEN, REGISTERED CI execution (the `node` job):
// in CI the runner runs in real mode, refreshes `results.json`, and the
// `docs-check` job regenerates `docs/plugin-compat.md` + fails on `git diff`.
//
// Usage:
//   node scripts/plugin-compat.mjs              # real mode (CI; installs + builds)
//   node scripts/plugin-compat.mjs --dry-run    # V8-free, recorded logs
//   node scripts/plugin-compat.mjs --dry-run --out -   # print the matrix

import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { loadCase } from '../conformance/plugins/schema.mjs';
import { evaluateCase } from '../conformance/plugins/evaluator.mjs';
import { parseReport, renderMatrix, tallyAsserts } from '../conformance/plugins/report.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..');
const casesDir = join(repo, 'conformance/plugins/cases');
const fixturesDir = join(repo, 'conformance/plugins/fixtures');
const logsDir = join(repo, 'conformance/plugins/logs');
const defaultOut = join(repo, 'conformance/plugins/results.json');

const args = process.argv.slice(2);
const dryRun = args.includes('--dry-run');
const help = args.includes('--help') || args.includes('-h');
const outIdx = args.indexOf('--out');
const outArg = outIdx >= 0 ? args[outIdx + 1] : defaultOut;

if (help) {
  process.stdout.write(
    [
      'oxide plugin-compat runner (V2-E05)',
      '',
      '  node scripts/plugin-compat.mjs              real mode (CI: install + build each case)',
      '  node scripts/plugin-compat.mjs --dry-run    V8-free: evaluate recorded logs + dists',
      '  node scripts/plugin-compat.mjs --dry-run --out -   print the matrix to stdout',
      '',
    ].join('\n'),
  );
  process.exit(0);
}

/**
 * List the case files (`<name>.json`) under `casesDir`, sorted.
 * @returns {string[]}
 */
function listCases() {
  return readdirSync(casesDir)
    .filter((f) => f.endsWith('.json'))
    .sort();
}

/**
 * Run one case in real mode: copy the fixture, install the plugin, build,
 * evaluate, record. Failures are documented (the spec's "not hidden" rule).
 *
 * @param {import('../conformance/plugins/schema.mjs').Case} caseObj
 * @param {string} caseName
 * @returns {import('../conformance/plugins/report.mjs').CaseResult}
 */
function runReal(caseObj, caseName) {
  const fixtureSrc = join(fixturesDir, caseObj.fixture);
  const tmp = join(repo, `.oxide-plugin-compat-tmp/${caseName}-${process.pid}`);
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });
  let note;
  try {
    if (!existsSync(fixtureSrc)) {
      return result(caseObj, 'fail', [], { passed: 0, failed: caseObj.assert.length },
        `fixture \`${caseObj.fixture}\` not found under conformance/plugins/fixtures/`);
    }
    cpSync(fixtureSrc, tmp, { recursive: true });
    // install the exact plugin version (the case's `plugin` is "<name>@<version>")
    const pkgMgr = detectPackageManager(tmp);
    const install = spawnSync(pkgMgr, ['add', caseObj.plugin, '--no-frozen-lockfile'], {
      cwd: tmp,
      encoding: 'utf8',
    });
    if (install.status !== 0) {
      return result(caseObj, 'fail', [], { passed: 0, failed: caseObj.assert.length },
        `install failed: ${(install.stderr || install.stdout || '').slice(0, 200)}`);
    }
    // build with the oxide CLI (the `node` job puts `oxide` on PATH)
    const build = spawnSync('oxide', ['build'], { cwd: tmp, encoding: 'utf8' });
    const log = `${build.stdout || ''}\n${build.stderr || ''}`;
    const reports = parseReport(log);
    const report = reports[pluginName(caseObj.plugin)] ?? reports[caseName];
    const unsupported = report?.unsupportedHooks ?? [];
    const evalRes = evaluateCase(caseObj, tmp);
    const status = unsupported.length
      ? 'unsupported-hook'
      : evalRes.ok
        ? 'pass'
        : 'fail';
    return result(caseObj, status, unsupported, tallyAsserts(evalRes.results),
      noteFor(status, unsupported, evalRes));
  } catch (e) {
    note = `runner error: ${/** @type {Error} */ (e).message}`;
    return result(caseObj, 'fail', [], { passed: 0, failed: caseObj.assert.length }, note);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

/**
 * Run one case in dry-run mode: read the recorded build log + the recorded
 * dist tree under `logs/<caseName>/`, evaluate the asserts, parse the report.
 * V8-free — no install, no build.
 *
 * @param {import('../conformance/plugins/schema.mjs').Case} caseObj
 * @param {string} caseName
 * @returns {import('../conformance/plugins/report.mjs').CaseResult}
 */
function runDry(caseObj, caseName) {
  const caseLogDir = join(logsDir, caseName);
  const buildLogPath = join(caseLogDir, 'build.log');
  let log = '';
  try {
    log = readFileSync(buildLogPath, 'utf8');
  } catch {
    return result(caseObj, 'fail', [], { passed: 0, failed: caseObj.assert.length },
      `no recorded build log at conformance/plugins/logs/${caseName}/build.log`);
  }
  const reports = parseReport(log);
  const report = reports[pluginName(caseObj.plugin)] ?? reports[caseName];
  const unsupported = report?.unsupportedHooks ?? [];
  const evalRes = evaluateCase(caseObj, caseLogDir);
  const status = unsupported.length
    ? 'unsupported-hook'
    : evalRes.ok
      ? 'pass'
      : 'fail';
  return result(caseObj, status, unsupported, tallyAsserts(evalRes.results),
    noteFor(status, unsupported, evalRes));
}

/** The documented reason for a non-pass result (the spec's "not hidden" rule). */
function noteFor(status, unsupportedHooks, evalRes) {
  if (status === 'unsupported-hook') {
    return `declares ${unsupportedHooks.join(', ')} (bridge-unsupported; the plugin fails at load so its output never lands)`;
  }
  if (!evalRes.ok) {
    return evalRes.results.find((r) => !r.ok)?.reason;
  }
  return undefined;
}

/**
 * @param {import('../conformance/plugins/schema.mjs').Case} caseObj
 * @param {string} status
 * @param {string[]} unsupportedHooks
 * @param {{ passed: number; failed: number }} asserts
 * @param {string} [note]
 * @returns {import('../conformance/plugins/report.mjs').CaseResult}
 */
function result(caseObj, status, unsupportedHooks, asserts, note) {
  const [name, version] = splitPlugin(caseObj.plugin);
  const out = {
    plugin: name,
    version,
    adapter: caseObj.adapter,
    status,
    unsupportedHooks,
    asserts,
  };
  if (note) out.note = note;
  return out;
}

/** Split "<name>@<version>" on the LAST `@` (scoped names carry one). */
function splitPlugin(p) {
  const i = p.lastIndexOf('@');
  return i > 0 ? [p.slice(0, i), p.slice(i + 1)] : [p, ''];
}
/** The package name half of a `plugin` string (for report lookup). */
function pluginName(p) {
  return splitPlugin(p)[0];
}

/** pnpm (the repo's manager) → npm fallback; corepack vendors pnpm in CI. */
function detectPackageManager(cwd) {
  if (existsSync(join(cwd, 'pnpm-lock.yaml'))) return 'pnpm';
  return 'npm';
}

function main() {
  /** @type {import('../conformance/plugins/report.mjs').CaseResult[]} */
  const results = [];
  for (const file of listCases()) {
    const caseName = basename(file, '.json');
    const caseObj = loadCase(join(casesDir, file));
    results.push(dryRun ? runDry(caseObj, caseName) : runReal(caseObj, caseName));
  }
  const payload = {
    generated_by: 'scripts/plugin-compat.mjs' + (dryRun ? ' --dry-run' : ''),
    count: results.length,
    results,
    matrix: renderMatrix(results),
  };
  const json = JSON.stringify(payload, null, 2) + '\n';
  if (outArg === '-') {
    process.stdout.write(payload.matrix + '\n');
  } else {
    writeFileSync(outArg, json);
    process.stdout.write(
      `wrote ${outArg} (${results.length} cases; ` +
        `${results.filter((r) => r.status === 'pass').length} pass, ` +
        `${results.filter((r) => r.status === 'fail').length} fail, ` +
        `${results.filter((r) => r.status === 'unsupported-hook').length} unsupported-hook)\n`,
    );
  }
}

main();
