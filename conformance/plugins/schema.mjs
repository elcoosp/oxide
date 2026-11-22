// The V2-E05 plugin-compat case schema + loader (spec v0.2 §V2-E05).
//
// One case file per third-party plugin: `conformance/plugins/cases/<name>.json`
// holds `{ plugin, fixture, adapter, assert, hooks }`. This module validates
// that shape with ZERO runtime dependencies — `node --test` runs the schema
// suite V8-free, the same rule the §22 conformance registry follows.
//
// The shape is the spec's verbatim:
//   { "plugin": "vite-plugin-svgr@x", "fixture": "svg-component",
//     "adapter": "react",
//     "assert": [{ "file": "dist/client/*.js", "contains": "createElement" }],
//     "hooks": ["transform"] }
//
// The `hooks` field lists the camelCase wire names the plugin declares — the
// same set `oxide_fw_plugin::Hook` serializes to (§15.2 table), so the
// bridge's load-time report and the case agree on vocabulary. A hook outside
// the known set is a schema error (a typo, never a silent skip).

import { readFileSync } from 'node:fs';

export const ADAPTERS = ['react', 'vue', 'svelte', 'solid'];

// The camelCase hook names the bridge knows (§15.2 table — mirrors
// `oxide_fw_plugin::hook_label` byte-for-byte; the Rust pins hold the model,
// this is the consumer-side vocabulary the cases draw from).
export const CASE_HOOKS = [
  'config',
  'configResolved',
  'buildStart',
  'buildEnd',
  'resolveId',
  'load',
  'transform',
  'renderChunk',
  'generateBundle',
  'transformHtml',
  'handleHotUpdate',
  'configureServer',
  'routes',
  'adapter',
  'serverMiddleware',
  'renderHead',
  'cacheHandler',
  'typegen',
  'diagnostics',
];

// The `status` values the runner records (spec §V2-E05): a case passes, fails
// an assert, or the bridge reported an unsupported hook at load.
export const STATUSES = ['pass', 'fail', 'unsupported-hook'];

/**
 * Validate one case object against the §V2-E05 shape.
 *
 * @param {unknown} raw - the parsed JSON of a case file.
 * @returns {{ ok: true, case: Case } | { ok: false, errors: string[] }}
 *   `ok` plus the normalized case on success; `errors` lists every offence
 *   (the file is authored, so strictness costs nothing — the same rule the
 *   errors registry parser follows).
 */
export function validateCase(raw) {
  const errors = [];
  if (!isPlainObject(raw)) {
    return { ok: false, errors: ['a case must be a JSON object'] };
  }
  const obj = /** @type {Record<string, unknown>} */ (raw);

  // plugin: "<name>@<version>" — the spec pins the @version separator so the
  // runner can install an exact package (verify each name + version when
  // writing a case; the CI node job re-resolves them).
  const plugin = obj.plugin;
  if (typeof plugin !== 'string' || plugin.length === 0) {
    errors.push('`plugin` must be a non-empty string ("<name>@<version>")');
  } else if (!plugin.includes('@')) {
    errors.push('`plugin` must carry a version ("<name>@<version>")');
  }

  if (typeof obj.fixture !== 'string' || obj.fixture.length === 0) {
    errors.push('`fixture` must be a non-empty string (a fixture dir name)');
  }

  if (typeof obj.adapter !== 'string' || !ADAPTERS.includes(obj.adapter)) {
    errors.push(`\`adapter\` must be one of ${ADAPTERS.join(', ')}`);
  }

  // assert: an array of { file, contains } — the file glob runs over the
  // emitted dist; `contains` is the substring the built output must carry.
  const asserts = obj.assert;
  if (!Array.isArray(asserts) || asserts.length === 0) {
    errors.push('`assert` must be a non-empty array of { file, contains }');
  } else {
    asserts.forEach((a, i) => {
      if (!isPlainObject(a) || typeof a.file !== 'string' || a.file.length === 0) {
        errors.push(`assert[${i}].file must be a non-empty glob string`);
      }
      if (!isPlainObject(a) || typeof a.contains !== 'string' || a.contains.length === 0) {
        errors.push(`assert[${i}].contains must be a non-empty substring`);
      }
    });
  }

  // hooks: the camelCase wire names the plugin declares. At least one — the
  // runner records `unsupportedHooks` against this set.
  const hooks = obj.hooks;
  if (!Array.isArray(hooks) || hooks.length === 0) {
    errors.push('`hooks` must be a non-empty array of camelCase hook names');
  } else {
    hooks.forEach((h, i) => {
      if (typeof h !== 'string' || !CASE_HOOKS.includes(h)) {
        errors.push(
          `hooks[${i}] is "${h}" — not a known hook (${CASE_HOOKS.join(', ')})`,
        );
      }
    });
  }

  if (errors.length > 0) {
    return { ok: false, errors };
  }
  return {
    ok: true,
    case: {
      plugin: /** @type {string} */ (plugin),
      fixture: /** @type {string} */ (obj.fixture),
      adapter: /** @type {string} */ (obj.adapter),
      assert: /** @type {{ file: string; contains: string }[]} */ (asserts),
      hooks: /** @type {string[]} */ (hooks),
    },
  };
}

/**
 * Load + validate a case file from disk. Returns the normalized case or
 * throws with the schema errors joined (the runner treats a bad case file as
 * a hard failure — a typoed case is never silently skipped).
 *
 * @param {string} path - absolute path to a `cases/<name>.json` file.
 * @returns {Case}
 */
export function loadCase(path) {
  const text = readFileSync(path, 'utf8');
  let raw;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new Error(`${path}: invalid JSON — ${/** @type {Error} */ (e).message}`);
  }
  const res = validateCase(raw);
  if (!res.ok) {
    throw new Error(`${path}: schema errors —\n  ${res.errors.join('\n  ')}`);
  }
  return res.case;
}

/**
 * @typedef {Object} Case
 * @property {string} plugin   - "<name>@<version>".
 * @property {string} fixture  - a fixture dir name under `fixtures/`.
 * @property {string} adapter  - one of {@link ADAPTERS}.
 * @property {{ file: string; contains: string }[]} assert
 * @property {string[]} hooks  - camelCase wire names the plugin declares.
 */

function isPlainObject(v) {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
