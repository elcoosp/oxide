// The V2-E05 plugin-compat assert evaluator (spec v0.2 §V2-E05).
//
// For each case the runner builds the fixture with `oxide-cli build`, then
// this module evaluates every `assert` against the emitted `dist/`: the
// `file` glob selects the built files, `contains` is the substring the
// transformed output must carry (e.g. SVGR's `createElement`, Vue's
// `_createVNode`). A fail is recorded, never hidden — the spec's acceptance.
//
// Zero runtime dependencies: a hand-rolled glob (`*` / `**`) + a recursive
// directory walk over `node:fs`. The dry-run mode evaluates the SAME
// evaluator against a recorded dist tree, so the assert path is unit-testable
// V8-free without a single install or build.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

/**
 * Convert a case `assert.file` glob into a RegExp. A single `*` matches one
 * path segment's worth of non-slash characters; a double-star matches across
 * segments (any chars including the separator); every other character is a
 * literal (regex metacharacters are escaped).
 *
 * @param {string} pattern
 * @returns {RegExp}
 */
export function globToRegex(pattern) {
  let src = '';
  let i = 0;
  while (i < pattern.length) {
    const ch = pattern[i];
    if (ch === '*') {
      if (pattern[i + 1] === '*') {
        src += '.*';
        i += 2;
        // swallow an optional following slash so `**/` matches zero dirs too
        if (pattern[i] === '/') i += 1;
      } else {
        src += '[^/]*';
        i += 1;
      }
    } else if (/[.+^${}()|[\]\\]/.test(ch)) {
      src += '\\' + ch;
      i += 1;
    } else if (ch === '/') {
      // accept either separator on disk (Windows builds emit `\`)
      src += '[/\\\\]';
      i += 1;
    } else {
      src += ch;
      i += 1;
    }
  }
  return new RegExp(`^${src}$`);
}

/**
 * Recursively walk `root`, returning the file paths (relative to `root`) that
 * match the glob `pattern`. A non-existent root yields `[]` (the caller
 * records a fail with the missing-glob reason, never a throw).
 *
 * @param {string} root   - absolute path to the build output root.
 * @param {string} pattern - the case's `assert.file` glob.
 * @returns {string[]}
 */
export function globFiles(root, pattern) {
  let entries;
  try {
    entries = readdirSync(root);
  } catch {
    return [];
  }
  const re = globToRegex(pattern);
  /** @type {string[]} */ const out = [];
  const walk = (dir, prefix) => {
    let list;
    try {
      list = readdirSync(dir);
    } catch {
      return;
    }
    for (const name of list) {
      const full = join(dir, name);
      let st;
      try {
        st = statSync(full);
      } catch {
        continue;
      }
      const rel = prefix ? `${prefix}/${name}` : name;
      if (st.isDirectory()) {
        walk(full, rel);
      } else if (re.test(rel)) {
        out.push(rel);
      }
    }
  };
  walk(root, '');
  out.sort();
  return out;
}

/**
 * Evaluate one `{ file, contains }` assert against a build root.
 *
 * @param {string} root
 * @param {{ file: string; contains: string }} assertion
 * @returns {{ ok: true; matched: string[] } | { ok: false; reason: string; matched: string[] }}
 *   `matched` lists the files the glob selected (empty when the glob matched
 *   nothing — a recorded fail, the spec's "not hidden" rule).
 */
export function evaluateAssert(root, assertion) {
  const matched = globFiles(root, assertion.file);
  if (matched.length === 0) {
    return {
      ok: false,
      matched,
      reason: `glob \`${assertion.file}\` matched no files in the build output`,
    };
  }
  for (const rel of matched) {
    let body;
    try {
      body = readFileSync(join(root, rel), 'utf8');
    } catch {
      continue;
    }
    if (body.includes(assertion.contains)) {
      return { ok: true, matched };
    }
  }
  return {
    ok: false,
    matched,
    reason: `\`${assertion.contains}\` not found in any of ${matched.length} file(s) matching \`${assertion.file}\``,
  };
}

/**
 * Evaluate every assert of a case against a build root.
 *
 * @param {import('./schema.mjs').Case} caseObj
 * @param {string} root - the fixture's build output root (the dir holding `dist/`).
 * @returns {{ ok: boolean; results: { file: string; contains: string; ok: boolean; reason?: string; matched: string[] }[] }}
 */
export function evaluateCase(caseObj, root) {
  /** @type {Array<{ file: string; contains: string; ok: boolean; reason?: string; matched: string[] }>} */
  const results = [];
  let ok = true;
  for (const a of caseObj.assert) {
    const r = evaluateAssert(root, a);
    if (r.ok) {
      results.push({ file: a.file, contains: a.contains, ok: true, matched: r.matched });
    } else {
      ok = false;
      results.push({
        file: a.file,
        contains: a.contains,
        ok: false,
        reason: r.reason,
        matched: r.matched,
      });
    }
  }
  return { ok, results };
}
