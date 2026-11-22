// The V2-E05 plugin-compat report parser + verified-plugins matrix renderer
// (spec v0.2 §V2-E05).
//
// Two consumers, one module:
//
// 1. `parseReport(buildLog)` — extracts the bridge's load-time
//    unsupported-hook report from a build log. Two shapes are read:
//    - the structured `oxide-plugin-load\t<json>` line the Rust bridge emits
//      (`oxide_fw_plugin::PluginRegistry::emit_load_reports_jsonl` +
//      `WasmPlugin::load_time_report`) — the WASM host's declared-but-
//      unexported / no-host-path gaps;
//    - the existing `PLG-HOOK` messages the JS bridge raises at load for
//      `configureServer`/`cacheHandler` (the fail-at-load arm — the runner
//      reads both, so reusing the EXISTING emission needs no new V8 op).
// 2. `renderMatrix(results)` — renders the verified-plugins table the
//    generated `docs/plugin-compat.md` embeds. One source of truth: the
//    runner writes `conformance/plugins/results.json`, the CLI's
//    `PluginCompatPage` generator embeds it, `docs-check` fails on drift.
//
// Zero runtime dependencies — `node --test` runs the parser + renderer
// suite V8-free against fixture logs.

/**
 * @typedef {Object} PluginReport
 * @property {string} plugin
 * @property {string} kind
 * @property {string[]} unsupportedHooks - camelCase wire names.
 * @property {string[]} wiredHooks      - camelCase wire names.
 * @property {boolean} loaded           - a structured report line was seen.
 */

/**
 * Parse the bridge's load-time report from a build log.
 *
 * @param {string} log - the captured `oxide-cli build` stderr/stdout.
 * @returns {Record<string, PluginReport>} keyed by plugin name; a plugin with
 *   only `PLG-HOOK` messages (no structured line) lands here too with
 *   `loaded: false` and the refused hooks in `unsupportedHooks`.
 */
export function parseReport(log) {
  /** @type {Record<string, PluginReport>} */ const reports = {};
  for (const line of log.split(/\r?\n/)) {
    const structured = line.startsWith('oxide-plugin-load\t')
      ? line.slice('oxide-plugin-load\t'.length)
      : null;
    if (structured) {
      let parsed;
      try {
        parsed = JSON.parse(structured);
      } catch {
        continue;
      }
      if (!parsed || typeof parsed.plugin !== 'string') continue;
      reports[parsed.plugin] = {
        plugin: parsed.plugin,
        kind: typeof parsed.kind === 'string' ? parsed.kind : 'unknown',
        unsupportedHooks: hookList(parsed.unsupportedHooks),
        wiredHooks: hookList(parsed.wiredHooks),
        loaded: true,
      };
      continue;
    }
    // The JS bridge's fail-at-load arm: `plugin \`X\` declares hook \`Y\`,
    // which the JS bridge does not support`. Reused, not re-emitted.
    const m = line.match(
      /plugin `([^`]+)` declares hook `([^`]+)`, which the JS bridge does not support/,
    );
    if (m) {
      const [, name, hook] = /** @type {RegExpMatchArray} */ (m);
      const r = reports[name] ?? {
        plugin: name,
        kind: 'js',
        unsupportedHooks: [],
        wiredHooks: [],
        loaded: false,
      };
      if (!r.unsupportedHooks.includes(hook)) r.unsupportedHooks.push(hook);
      reports[name] = r;
    }
  }
  return reports;
}

/**
 * @param {unknown} v
 * @returns {string[]}
 */
function hookList(v) {
  return Array.isArray(v) ? v.filter((h) => typeof h === 'string') : [];
}

/**
 * @typedef {Object} CaseResult
 * @property {string} plugin   - the package name (no version).
 * @property {string} version  - the installed version.
 * @property {string} adapter  - react / vue / svelte / solid.
 * @property {'pass'|'fail'|'unsupported-hook'} status
 * @property {string[]} unsupportedHooks
 * @property {{ passed: number; failed: number }} asserts
 * @property {string} [note]   - a documented failure reason (never hidden).
 */

/**
 * Render the verified-plugins markdown table from the runner's results.
 *
 * The table is the V2-E05 acceptance: at least ten third-party plugins
 * verified, failures documented not hidden. The generated
 * `docs/plugin-compat.md` embeds this block byte-identical (the
 * `PluginCompatPage` generator + the docs-check gate).
 *
 * @param {CaseResult[]} results
 * @returns {string}
 */
export function renderMatrix(results) {
  const sorted = [...results].sort((a, b) =>
    a.plugin < b.plugin ? -1 : a.plugin > b.plugin ? 1 : 0,
  );
  const rows = sorted.map((r) => {
    const hooks = r.unsupportedHooks.length
      ? r.unsupportedHooks.join(', ')
      : '—';
    const asserts = `${r.asserts.passed}/${r.asserts.passed + r.asserts.failed}`;
    const note = r.note ? r.note.replace(/\|/g, '\\|') : '—';
    return `| \`${r.plugin}\` | \`${r.version}\` | ${r.adapter} | ${r.status} | ${hooks} | ${asserts} | ${note} |`;
  });
  return [
    '| Plugin | Version | Adapter | Status | Unsupported hooks | Asserts | Note |',
    '| --- | --- | --- | --- | --- | --- | --- |',
    ...rows,
  ].join('\n');
}

/**
 * Tally the per-assert results into the `{ passed, failed }` summary the
 * matrix renders.
 *
 * @param {{ ok: boolean }[]} assertResults
 * @returns {{ passed: number; failed: number }}
 */
export function tallyAsserts(assertResults) {
  let passed = 0;
  let failed = 0;
  for (const a of assertResults) {
    if (a.ok) passed += 1;
    else failed += 1;
  }
  return { passed, failed };
}
