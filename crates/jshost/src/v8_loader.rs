//! §4.2: a `ModuleLoader` for the V8 runtime that transpiles and resolves the
//! way the Node worker's `tsx-loader.mjs` does — but in Rust.
//!
//! `FsModuleLoader` (deno_core's) reads files verbatim, which fails on any
//! `.ts`/`.tsx`: V8 cannot parse type annotations or JSX. This loader reads the
//! file, runs `oxide_fw_compiler::lower_to_js` (Oxc type-strip + JSX lowering),
//! and returns the plain JS. It resolves a relative specifier against the
//! importer and a bare specifier against `node_modules` via `oxc_resolver` —
//! the same resolver the bundler uses, so a project that bundles also serves.
//!
//! This is what makes the V8 backend able to load `oxide.config.ts` and the
//! framework adapter it imports, not only a pre-bundled `dist/server` graph.

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleResolveResponse,
    ModuleSource, ModuleType, ResolutionKind,
};
use std::path::{Path, PathBuf};

pub struct TranspileLoader {
    /// Project root: bare specifiers resolve `node_modules` from here.
    root: PathBuf,
    /// The JSX runtime to lower `.tsx`/`.jsx` against (`react` → the Oxc
    /// default; `@solidjs/web` for Solid).
    jsx_import_source: Option<String>,
    /// PERF-10: the resolver, built **once** and reused. `oxc_resolver`'s
    /// `Resolver` holds caches (realpath, package.json, tsconfig); building it
    /// per resolve threw those away and re-read the filesystem every import.
    /// `OnceCell` because the loader is constructed before the process cwd is
    /// guaranteed stable.
    resolver: std::cell::OnceCell<oxc_resolver::Resolver>,
}

impl TranspileLoader {
    pub fn new(root: impl Into<PathBuf>, jsx_import_source: Option<String>) -> Self {
        Self {
            root: root.into(),
            jsx_import_source,
            resolver: std::cell::OnceCell::new(),
        }
    }

    /// PERF-10: the resolver, built on first use and reused thereafter.
    fn resolver(&self) -> &oxc_resolver::Resolver {
        self.resolver.get_or_init(|| {
            let options = oxc_resolver::ResolveOptions {
                extensions: vec![
                    ".ts".into(),
                    ".tsx".into(),
                    ".js".into(),
                    ".jsx".into(),
                    ".mjs".into(),
                    ".cjs".into(),
                    ".json".into(),
                ],
                // §4.6: the V8 runtime is browser-like. `worker` FIRST (see
                // the long note that used to live here): react-dom/server →
                // server.browser.js, @solidjs/web → dist/server.js.
                condition_names: vec![
                    "worker".into(),
                    "browser".into(),
                    "import".into(),
                    "module".into(),
                    "default".into(),
                ],
                main_fields: vec!["module".into(), "main".into()],
                ..Default::default()
            };
            oxc_resolver::Resolver::new(options)
        })
    }

    fn is_relative(spec: &str) -> bool {
        spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
    }

    /// Resolve a specifier seen from `referrer` (a `file://` URL) to a file
    /// path. Returns `None` if it cannot be resolved.
    fn resolve_to_path(&self, specifier: &str, referrer: &str) -> Option<PathBuf> {
        // A `file://` specifier (the framework ops import the config that way)
        // is already an absolute path — use it directly. Falling through to
        // oxc_resolver here is a race: it resolves inconsistently against a
        // base directory. An absolute path is the same case.
        // A `file://` specifier is already absolute — return it without an
        // `is_file` gate (a concurrent build write or a pool worker racing the
        // first render made the stat flaky). `load` reports a real read error.
        //
        // Parse as a URL then `to_file_path()`, NOT `strip_prefix("file://")`:
        // on Windows `file:///C:/Users/x/mod.mjs` strips to `/C:/Users/x/mod.mjs`,
        // whose leading `/` before the drive makes `Path::new(..)` a non-file
        // path — the subsequent `ModuleSpecifier::from_file_path` then fails
        // with "path is not a file URL". `to_file_path()` knows the
        // `file:///C:/...` -> `C:\...` mapping on every OS.
        if specifier.starts_with("file://")
            && let Ok(url) = deno_core::ModuleSpecifier::parse(specifier)
            && let Ok(path) = url.to_file_path()
        {
            return Some(path);
        }
        // Malformed `file://` URL — fall through to the resolver rather
        // than guessing a path from the raw string.
        // A bare absolute path likewise.
        let as_path = Path::new(specifier);
        if as_path.is_absolute() {
            return Some(as_path.to_path_buf());
        }

        // The referrer is a `file://` URL; its directory is the resolution base.
        // Canonicalize it: a referrer like `<manifest>/../../project/.oxide/x.js`
        // leaves `..` segments that break a raw `join` probe on macOS, where
        // `/var` is a symlink to `/private/var` and a `..`-laden path can point
        // at a different file than its canonical form.
        //
        // Parse as a URL, not `strip_prefix("file://")`: on Windows the
        // referrer is `file:///C:/x/mod.mjs`; a strip leaves `/C:/x/mod.mjs`,
        // whose `Path::parent()` is `/C:/x` — a directory that does not
        // exist, so every relative import resolves from the wrong base and
        // then fails. `to_file_path()` maps the URL to the native
        // `C:\x\mod.mjs` on every OS.
        let base_raw = referrer
            .strip_prefix("file://")
            .map(|s| {
                // Prefer the URL-aware conversion; fall back to the raw
                // suffix only if the URL is malformed (a hand-built
                // referrer in a test) — then let canonicalize surface it.
                deno_core::ModuleSpecifier::parse(referrer)
                    .ok()
                    .and_then(|u| u.to_file_path().ok())
                    .unwrap_or_else(|| PathBuf::from(s))
            })
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());
        let base_dir = crate::strip_verbatim(std::fs::canonicalize(&base_raw).unwrap_or(base_raw));

        // A relative import with an explicit extension we can probe directly.
        if Self::is_relative(specifier) {
            let joined = base_dir.join(specifier);
            if let Some(found) = probe_extensions(&joined) {
                return Some(crate::strip_verbatim(
                    std::fs::canonicalize(&found).unwrap_or(found),
                ));
            }
        }

        // Everything else goes through the reused resolver (tsconfig paths,
        // package `exports`, conditions, node_modules) — PERF-10.
        self.resolver()
            .resolve(&base_dir, specifier)
            .ok()
            .map(|r| crate::strip_verbatim(r.full_path().to_path_buf()))
    }
}

/// Split `?query` off a specifier/URL. BUG-04 generation tags.
fn split_query(s: &str) -> (&str, Option<&str>) {
    match s.split_once('?') {
        Some((base, q)) => (base, Some(q)),
        None => (s, None),
    }
}

/// Try `p` as-is, then with each module extension appended.
fn probe_extensions(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let cand = p.with_extension(ext);
        if cand.is_file() {
            return Some(cand);
        }
    }
    // Directory index (`./dir` → `./dir/index.ts`).
    if p.is_dir() {
        for ext in ["ts", "tsx", "js", "jsx", "mjs"] {
            let cand = p.join(format!("index.{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// The source of a `node:*` shim (§4.6 tier 2). Covers the built-ins a
/// framework runtime references without depending on Node behavior: a no-op
/// `createRequire`, and minimal `path`/`fs`/`url` surfaces. Anything else
/// returns an empty module so an unused import does not fail the load.
fn node_shim(specifier: &str) -> String {
    // `specifier` is the URL path, e.g. `/shim/module`.
    let name = specifier.rsplit('/').next().unwrap_or("");
    match name {
        "module" => "export function createRequire() {\n\
             return function () { throw new Error('require() is not available in the V8 runtime'); };\n\
           }\n\
           export default { createRequire };\n"
            .to_string(),
        "path" => "export function join(...p) { return p.join('/').replace(/\\/+/g, '/'); }\n\
           export function resolve(...p) { return join(...p); }\n\
           export function dirname(p) { return p.replace(/\\/[^/]*$/, '') || '/'; }\n\
           export function basename(p) { return p.replace(/.*\\//, ''); }\n\
           export const sep = '/';\n\
           export default { join, resolve, dirname, basename, sep };\n"
            .to_string(),
        // `pathToFileURL` must be Windows-aware: a bare `D:\foo\bar.js`
        // has to become `file:///D:/foo/bar.js`. The naive POSIX form
        // produces `file:///D:\foo\bar.js`, where `D:` is parsed as the
        // URL scheme (ERR_UNSUPPORTED_ESM_URL_SCHEME). `fileURLToPath`
        // mirrors it: strip the scheme+slashes, and re-add the leading `/`
        // a Windows path does not carry (`file:///D:/x` -> `D:/x`).
        "url" => r#"export function fileURLToPath(u) {
  let s = String(u).replace(/^file:\/\//, '');
  if (/^\/[A-Za-z]:/.test(s)) s = s.slice(1);
  return s;
}
export function pathToFileURL(p) {
  const s = String(p);
  if (s.startsWith('file://')) return new URL(s);
  const norm = s.replace(/\\/g, '/');
  if (/^[A-Za-z]:/.test(norm)) return new URL('file:///' + norm);
  if (norm.startsWith('//')) return new URL('file:' + norm);
  return new URL('file://' + (norm.startsWith('/') ? '' : '/') + norm);
}
export default { fileURLToPath, pathToFileURL };"#.to_string(),
        // node:crypto — synchronous sha256 only. `@oxide-fw/core` keys its
        // content cache on `createHash('sha256').update(raw).digest('hex')`;
        // the isolate has no async digest op, so this is a real pure-JS
        // SHA-256 (FIPS 180-4), not a stub. Any other algorithm throws
        // naming the limitation (§4.6.3 loudness).
        "crypto" => concat!(
            "const __K = new Uint32Array([0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,",
            "0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,",
            "0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,",
            "0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,",
            "0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,",
            "0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,",
            "0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,",
            "0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,",
            "0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,",
            "0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,",
            "0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2]);\n",
            "function __u8(s) {\n",
            "  if (typeof TextEncoder !== 'undefined') return new TextEncoder().encode(s);\n",
            "  const out = [];\n",
            "  for (let i = 0; i < s.length; i++) {\n",
            "    let c = s.charCodeAt(i);\n",
            "    if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {\n",
            "      const d = s.charCodeAt(i + 1);\n",
            "      if (d >= 0xdc00 && d <= 0xdfff) { c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00); i++; }\n",
            "    }\n",
            "    if (c < 0x80) out.push(c);\n",
            "    else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));\n",
            "    else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));\n",
            "    else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));\n",
            "  }\n",
            "  return new Uint8Array(out);\n",
            "}\n",
            "export function createHash(algo) {\n",
            "  if (String(algo).toLowerCase() !== 'sha256') {\n",
            "    throw new Error('JSH-NODESHIM: node:crypto createHash supports only sha256 in the V8 runtime (got: ' + algo + ')');\n",
            "  }\n",
            "  const h = new Uint32Array([0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]);\n",
            "  const w = new Uint32Array(64);\n",
            "  const pend = new Uint8Array(64);\n",
            "  let plen = 0;\n",
            "  let total = 0;\n",
            "  const rotr = (x, n) => (x >>> n) | (x << (32 - n));\n",
            "  const block = (p, off) => {\n",
            "    for (let i = 0; i < 16; i++) {\n",
            "      const j = off + i * 4;\n",
            "      w[i] = ((p[j] << 24) | (p[j + 1] << 16) | (p[j + 2] << 8) | p[j + 3]) >>> 0;\n",
            "    }\n",
            "    for (let i = 16; i < 64; i++) {\n",
            "      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);\n",
            "      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);\n",
            "      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;\n",
            "    }\n",
            "    let a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];\n",
            "    for (let i = 0; i < 64; i++) {\n",
            "      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);\n",
            "      const ch = (e & f) ^ (~e & g);\n",
            "      const t1 = (hh + S1 + ch + __K[i] + w[i]) >>> 0;\n",
            "      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);\n",
            "      const maj = (a & b) ^ (a & c) ^ (b & c);\n",
            "      const t2 = (S0 + maj) >>> 0;\n",
            "      hh = g; g = f; f = e; e = (d + t1) >>> 0;\n",
            "      d = c; c = b; b = a; a = (t1 + t2) >>> 0;\n",
            "    }\n",
            "    h[0] = (h[0] + a) >>> 0; h[1] = (h[1] + b) >>> 0; h[2] = (h[2] + c) >>> 0; h[3] = (h[3] + d) >>> 0;\n",
            "    h[4] = (h[4] + e) >>> 0; h[5] = (h[5] + f) >>> 0; h[6] = (h[6] + g) >>> 0; h[7] = (h[7] + hh) >>> 0;\n",
            "  };\n",
            "  const absorb = (bytes) => {\n",
            "    let i = 0;\n",
            "    if (plen > 0) {\n",
            "      const take = Math.min(64 - plen, bytes.length);\n",
            "      pend.set(bytes.subarray(0, take), plen);\n",
            "      plen += take; i = take;\n",
            "      if (plen === 64) { block(pend, 0); plen = 0; }\n",
            "    }\n",
            "    while (bytes.length - i >= 64) { block(bytes, i); i += 64; }\n",
            "    if (bytes.length > i) { pend.set(bytes.subarray(i), plen); plen += bytes.length - i; }\n",
            "  };\n",
            "  const out = {\n",
            "    update(data) {\n",
            "      let bytes;\n",
            "      if (typeof data === 'string') bytes = __u8(data);\n",
            "      else if (data instanceof Uint8Array) bytes = data;\n",
            "      else if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);\n",
            "      else throw new TypeError('createHash().update: string, Uint8Array or ArrayBuffer required');\n",
            "      absorb(bytes);\n",
            "      total += bytes.length;\n",
            "      return out;\n",
            "    },\n",
            "    digest(encoding) {\n",
            "      const bits = total * 8;\n",
            "      const hi = Math.floor(bits / 0x100000000);\n",
            "      const lo = bits % 0x100000000;\n",
            "      const rem = plen;\n",
            "      const padLen = (rem < 56 ? 56 - rem : 120 - rem) + 8;\n",
            "      const pad = new Uint8Array(padLen);\n",
            "      pad[0] = 0x80;\n",
            "      pad[padLen - 8] = (hi >>> 24) & 255;\n",
            "      pad[padLen - 7] = (hi >>> 16) & 255;\n",
            "      pad[padLen - 6] = (hi >>> 8) & 255;\n",
            "      pad[padLen - 5] = hi & 255;\n",
            "      pad[padLen - 4] = (lo >>> 24) & 255;\n",
            "      pad[padLen - 3] = (lo >>> 16) & 255;\n",
            "      pad[padLen - 2] = (lo >>> 8) & 255;\n",
            "      pad[padLen - 1] = lo & 255;\n",
            "      absorb(pad);\n",
            "      const hex = '0123456789abcdef';\n",
            "      let s = '';\n",
            "      for (let i = 0; i < 8; i++) {\n",
            "        s += hex[(h[i] >>> 28) & 15] + hex[(h[i] >>> 24) & 15] + hex[(h[i] >>> 20) & 15] + hex[(h[i] >>> 16) & 15] + hex[(h[i] >>> 12) & 15] + hex[(h[i] >>> 8) & 15] + hex[(h[i] >>> 4) & 15] + hex[h[i] & 15];\n",
            "      }\n",
            "      const bytes = new Uint8Array(32);\n",
            "      for (let i = 0; i < 8; i++) {\n",
            "        bytes[i * 4] = (h[i] >>> 24) & 255; bytes[i * 4 + 1] = (h[i] >>> 16) & 255;\n",
            "        bytes[i * 4 + 2] = (h[i] >>> 8) & 255; bytes[i * 4 + 3] = h[i] & 255;\n",
            "      }\n",
            "      if (encoding === 'hex') return s;\n",
            "      if (encoding === 'base64' && typeof btoa === 'function') return btoa(String.fromCharCode.apply(null, bytes));\n",
            "      return bytes;\n",
            "    },\n",
            "  };\n",
            "  return out;\n",
            "}\n",
            "export default { createHash };\n"
        )
        .to_string(),
        // node:fs / node:fs/promises / node:util — import-safe, call-loud.
        // Importing must LINK (a transitive import inside a dependency
        // cannot be allowed to fail the whole module graph before any code
        // runs), but calling throws a message naming the builtin and the
        // remedy — §4.6.3 loudness is preserved at the call site. Export
        // exactly the names the runtime graph imports (scope-checked across
        // packages/*/dist); any other named import still fails the link
        // loudly, which is the desired default.
        "fs" | "promises" => {
            let label = if name == "fs" { "node:fs" } else { "node:fs/promises" };
            format!(
                "const __loud = (n) => () => {{ throw new Error('JSH-NODESHIM: {label}.' + n + ' is not available in the V8 runtime; set server.backend to node (§4.6)'); }};\n\
                 export const readFile = __loud('readFile');\n\
                 export const readFileSync = __loud('readFileSync');\n\
                 export const mkdir = __loud('mkdir');\n\
                 export const mkdirSync = __loud('mkdirSync');\n\
                 export const readdir = __loud('readdir');\n\
                 export const writeFile = __loud('writeFile');\n\
                 export default {{ readFile, readFileSync, mkdir, mkdirSync, readdir, writeFile }};\n"
            )
        }
        "util" => "export function parseArgs() { throw new Error('JSH-NODESHIM: node:util.parseArgs is not available in the V8 runtime; set server.backend to node (§4.6)'); }\n\
           export default { parseArgs };\n"
            .to_string(),
        // buffer/events/stream — import-safe arms. requires_node_backend
        // already treats these as shimmed; without arms here, a graph that
        // imports them passes the check and then dies at link with a
        // confusing "does not provide an export named X". Export the names
        // the runtime reaches for; calls throw loudly (§4.6.3).
        "buffer" => "export class Buffer extends Uint8Array {\n\
             static from() { throw new Error('JSH-NODESHIM: node:buffer.Buffer.from is not available in the V8 runtime; set server.backend to node (§4.6)'); }\n\
           }\n\
           export const constants = {};\n\
           export default { Buffer, constants };\n"
            .to_string(),
        "events" => "export class EventEmitter {\n\
             on() { throw new Error('JSH-NODESHIM: node:events.EventEmitter.on is not available in the V8 runtime; set server.backend to node (§4.6)'); }\n\
           }\n\
           export default { EventEmitter };\n"
            .to_string(),
        "stream" => "export class Readable {\n\
             constructor() { throw new Error('JSH-NODESHIM: node:stream.Readable is not available in the V8 runtime; set server.backend to node (§4.6)'); }\n\
           }\n\
           export class Writable {\n\
             constructor() { throw new Error('JSH-NODESHIM: node:stream.Writable is not available in the V8 runtime; set server.backend to node (§4.6)'); }\n\
           }\n\
           export default { Readable, Writable };\n"
            .to_string(),
        // SPEC-02 / §4.6.3: an unsupported Node builtin is LOUD. A silent
        // `export default {}` makes `import fs from 'node:fs'; fs.readFile(…)`
        // fail at the call site with a confusing "not a function" far from
        // the cause. Throwing on import names the builtin and the remedy.
        other => format!(
            "throw new Error('JSH-NODESHIM: node:{other} is not available in the V8 runtime; set server.backend to node or avoid this import (§4.6)');\n"
        ),
    }
}

/// SPEC-02 / §4.6.3: does this module specifier require the Node backend?
///
/// - a `.node` native addon — V8 cannot load it at all;
/// - a `node:` builtin outside the shim set (`fs`, `crypto`, `child_process`,
///   `net`, `worker_threads`, …).
///
/// When true and `server.backend` is `auto`/`v8`, the build switches to Node
/// and prints why.
pub fn requires_node_backend(specifier: &str) -> Option<String> {
    if specifier.ends_with(".node") {
        return Some(format!("native addon `{specifier}`"));
    }
    if let Some(name) = specifier.strip_prefix("node:") {
        let shimmed = matches!(
            name,
            "module"
                | "path"
                | "url"
                | "buffer"
                | "events"
                | "util"
                | "stream"
                | "crypto"
                | "fs"
                | "fs/promises"
        );
        if !shimmed {
            return Some(format!("node:{name} builtin"));
        }
    }
    None
}

/// Pick the `lang` tag `lower_to_js` expects from a path's extension.
fn lang_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") => "ts",
        Some("tsx") => "tsx",
        Some("jsx") => "jsx",
        Some("js") => "js",
        Some("mjs") => "mjs",
        _ => "js",
    }
}

fn is_esm_statement(t: &str) -> bool {
    for kw in ["import", "export"] {
        if t.starts_with(kw)
            && let Some(c) = t[kw.len()..].chars().next()
            && !c.is_alphanumeric()
            && c != '_'
        {
            return true;
        }
    }
    false
}
fn looks_like_cjs(source: &str) -> bool {
    let has_marker = source.contains("module.exports")
        || source.contains("exports.")
        || source.contains("exports[");
    if !has_marker {
        return false;
    }
    for line in source.lines() {
        if is_esm_statement(line.trim_start()) {
            return false;
        }
    }
    true
}
fn extract_requires(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = source.as_bytes();
    let needle = b"require(";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let quote = bytes[j];
                let start = j + 1;
                let mut k = start;
                while k < bytes.len() && bytes[k] != quote && bytes[k] != b'\n' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == quote {
                    let spec = std::str::from_utf8(&bytes[start..k])
                        .unwrap_or("")
                        .to_string();
                    if !spec.is_empty() && !out.contains(&spec) && !spec.starts_with("node:") {
                        out.push(spec);
                    }
                    i = k + 1;
                    continue;
                }
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}
fn extract_named_exports(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    let prefixes: [&[u8]; 2] = [b"exports.", b"module.exports."];
    while i < bytes.len() {
        let mut matched = false;
        for pfx in &prefixes {
            if i + pfx.len() <= bytes.len() && &bytes[i..i + pfx.len()] == *pfx {
                if i > 0 {
                    let prev = bytes[i - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' {
                        continue;
                    }
                }
                let mut j = i + pfx.len();
                let ns = j;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'$')
                {
                    j += 1;
                }
                if j == ns {
                    continue;
                }
                let mut k = j;
                while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                    k += 1;
                }
                if k < bytes.len()
                    && bytes[k] == b'='
                    && (k + 1 >= bytes.len() || bytes[k + 1] != b'=')
                {
                    let name = std::str::from_utf8(&bytes[ns..j]).unwrap_or("").to_string();
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                    i = j;
                    matched = true;
                }
            }
        }
        if !matched {
            i += 1;
        }
    }
    if let Some(start) = source.find("module.exports") {
        let after = &source[start + "module.exports".len()..];
        let after = after.trim_start();
        if after.starts_with('=') {
            let after = after[1..].trim_start();
            if after.starts_with('{') {
                let end = after[1..].find('}').unwrap_or(0);
                let inner = &after[1..1 + end];
                for part in inner.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let key = part.split(':').next().unwrap_or("").trim();
                    if !key.is_empty()
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                        && !out.contains(&key.to_string())
                    {
                        out.push(key.to_string());
                    }
                }
            }
        }
    }
    out
}
fn extract_module_exports_requires(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let needle = b"module.exports";
    let bytes = source.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            if i > 0 {
                let prev = bytes[i - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' {
                    i += 1;
                    continue;
                }
            }
            let mut j = i + needle.len();
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n') {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'=' || (j + 1 < bytes.len() && bytes[j + 1] == b'=')
            {
                i += 1;
                continue;
            }
            j += 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n') {
                j += 1;
            }
            if j + 7 > bytes.len() || &bytes[j..j + 7] != b"require" {
                i += 1;
                continue;
            }
            j += 7;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'(' {
                i += 1;
                continue;
            }
            j += 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j >= bytes.len() || (bytes[j] != b'"' && bytes[j] != b'\'') {
                i += 1;
                continue;
            }
            let quote = bytes[j];
            let start = j + 1;
            let mut k = start;
            while k < bytes.len() && bytes[k] != quote && bytes[k] != b'\n' {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == quote {
                let spec = std::str::from_utf8(&bytes[start..k])
                    .unwrap_or("")
                    .to_string();
                if !spec.is_empty() && !out.contains(&spec) {
                    out.push(spec);
                }
                i = k + 1;
                continue;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}
fn resolve_relative(path: &std::path::Path, spec: &str) -> Option<std::path::PathBuf> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return None;
    }
    let parent = path.parent()?;
    let joined = parent.join(spec);
    if joined.is_file() {
        return Some(joined);
    }
    if joined.extension().is_none() {
        let with_js = joined.with_extension("js");
        if with_js.is_file() {
            return Some(with_js);
        }
    }
    None
}
fn extract_reexport_names(source: &str, path: &std::path::Path, depth: u32) -> Vec<String> {
    if depth > 8 {
        return vec![];
    }
    for spec in extract_module_exports_requires(source) {
        if let Some(tp) = resolve_relative(path, &spec)
            && let Ok(content) = std::fs::read_to_string(&tp)
        {
            let names = extract_named_exports(&content);
            if !names.is_empty() {
                return names;
            }
            let nested = extract_reexport_names(&content, &tp, depth + 1);
            if !nested.is_empty() {
                return nested;
            }
        }
    }
    vec![]
}
fn transform_cjs_to_esm(
    source: &str,
    path: &std::path::Path,
) -> Result<String, deno_error::JsErrorBox> {
    let requires = extract_requires(source);
    let mut named = extract_named_exports(source);
    if named.is_empty() {
        named = extract_reexport_names(source, path, 0);
    }
    let mut import_lines = String::new();
    let mut require_shim = String::new();
    for (i, spec) in requires.iter().enumerate() {
        let local = format!("__cjs_import_{}", i);
        import_lines.push_str(&format!("import {} from {:?};\n", local, spec));
        require_shim.push_str(&format!("  if (spec === {:?}) return {};\n", spec, local));
    }
    let path_display = path.display();
    let mut out = String::new();
    out.push_str(&format!(
        "// @oxide-cjs-interop: transformed ({})\n",
        path_display
    ));
    out.push_str(&import_lines);
    out.push_str("const __cjs_module = { exports: {} };\n");
    out.push_str("const __cjs_require = (spec) => {\n");
    out.push_str(&require_shim);
    out.push_str("  throw new Error(`oxide: CJS require(${spec}) not statically resolvable`);\n");
    out.push_str("};\n");
    out.push_str("(function(module, exports, require) {\n");
    out.push_str(source);
    out.push_str("\n})(__cjs_module, __cjs_module.exports, __cjs_require);\n");
    for name in &named {
        out.push_str(&format!(
            "export const {} = __cjs_module.exports.{};\n",
            name, name
        ));
    }
    out.push_str("export default __cjs_module.exports;\n");
    Ok(out)
}
impl ModuleLoader for TranspileLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        // BUG-04: generation query (?v=N from __oxideFrameworkInvalidate).
        // Strip for path resolution, then re-attach to the canonical URL so
        // deno_core caches generations separately. A nested relative import
        // with no query inherits the referrer's — the whole graph moves to
        // the new generation together (mirrors the Node worker's versioned()
        // tagger).
        let (spec_base, spec_query) = split_query(specifier);
        let (_, ref_query) = split_query(referrer);
        let query = spec_query.or(ref_query);
        // §4.6 tier 2: a `node:*` specifier resolves to an in-memory shim, not
        // a file. V8 has no Node built-ins; the shim covers the handful a
        // framework runtime touches (a no-op `createRequire`, an empty
        // `path`/`fs`). Without this, bundling React's ESM build — which
        // imports `node:module` for a dev-only warning — fails at load.
        if let Some(builtin) = spec_base.strip_prefix("node:") {
            // Drop the colon: `oxide:shim/module`, so the shim's path is a
            // clean `/shim/<name>` with no second `:` to confuse URL parsing.
            let url = deno_core::ModuleSpecifier::parse(&format!("oxide:shim/{builtin}"))
                .map_err(|_| deno_error::JsErrorBox::generic("bad node: specifier"))?;
            return Ok(url);
        }
        let Some(path) = self.resolve_to_path(spec_base, referrer) else {
            return Err(deno_error::JsErrorBox::generic(format!(
                "cannot resolve `{specifier}` from `{referrer}`"
            )));
        };
        let mut url = deno_core::ModuleSpecifier::from_file_path(&path)
            .map_err(|_| deno_error::JsErrorBox::generic("path is not a file URL"))?;
        if let Some(q) = query {
            url.set_query(Some(q));
        }
        Ok(url)
    }

    fn load(
        &self,
        module_specifier: &deno_core::ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        // A `node:*` shim: return the in-memory source.
        if module_specifier.scheme() == "oxide" {
            let spec = module_specifier.clone();
            let src = node_shim(module_specifier.path());
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                deno_core::ModuleSourceCode::String(src.into()),
                &spec,
                None,
            )));
        }
        let (spec_no_query, _) = split_query(module_specifier.as_str());
        let path = match deno_core::ModuleSpecifier::parse(spec_no_query)
            .map_err(|_| deno_error::JsErrorBox::generic("module specifier is not a file path"))
            .and_then(|u| {
                u.to_file_path().map_err(|_| {
                    deno_error::JsErrorBox::generic("module specifier is not a file path")
                })
            }) {
            Ok(p) => p,
            Err(e) => {
                return ModuleLoadResponse::Sync(Err(e));
            }
        };
        // The ModuleSource keeps the ORIGINAL specifier (query included) so
        // deno_core caches `?v=N` generations as distinct modules (BUG-04).
        let spec = module_specifier.clone();
        // G1: retained as a field read for test introspection; the
        // compile path now routes through compile_module_sync.
        let _jsx = self.jsx_import_source.clone();
        let fut = async move {
            let src = std::fs::read_to_string(&path).map_err(|e| {
                deno_error::JsErrorBox::generic(format!("read {}: {e}", path.display()))
            })?;
            // BUG-08: CSS is a server side-effect import — resolve to an
            // empty module (SSR never applies styles; the client bundle's
            // CssPlugin handles the real CSS).
            if path.extension().and_then(|e| e.to_str()) == Some("css") {
                return Ok(ModuleSource::new(
                    ModuleType::JavaScript,
                    deno_core::ModuleSourceCode::String("export default \"\";".to_string().into()),
                    &spec,
                    None,
                ));
            }
            // JSON modules are passed through; V8 handles the `with { type }`.
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                return Ok(ModuleSource::new(
                    ModuleType::Json,
                    deno_core::ModuleSourceCode::String(src.into()),
                    &spec,
                    None,
                ));
            }
            let code = if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts" | "tsx" | "jsx")
            ) {
                let lang = lang_of(&path);
                let path_str = path.to_string_lossy().into_owned();
                let after_plugins = crate::v8_ops::compile_module_sync(
                    path_str.clone(),
                    src.clone(),
                    lang.to_string(),
                )
                .map_err(|e| {
                    deno_error::JsErrorBox::generic(format!(
                        "compile {}: {} {}",
                        path.display(),
                        e.0.code,
                        e.0.message
                    ))
                })?;
                oxide_fw_compiler::lower_to_js(&after_plugins, lang, &path_str, _jsx.as_deref())
                    .map(|(c, _map)| c)
                    .map_err(|e| {
                        deno_error::JsErrorBox::generic(format!(
                            "lower {}: {} {}",
                            path.display(),
                            e.0.code,
                            e.0.message
                        ))
                    })?
            } else if path.extension().and_then(|e| e.to_str()) == Some("js")
                && looks_like_cjs(&src)
            {
                transform_cjs_to_esm(&src, &path)?
            } else {
                src
            };
            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                deno_core::ModuleSourceCode::String(code.into()),
                &spec,
                None,
            ))
        };
        ModuleLoadResponse::Async(Box::pin(fut))
    }
}

#[cfg(test)]
mod node_shim_tests {
    use super::*;

    // ---- crypto ----

    #[test]
    fn crypto_shim_has_create_hash_and_fips_iv() {
        let src = node_shim("shim/crypto");
        assert!(
            src.contains("export function createHash"),
            "node:crypto arm missing createHash"
        );
        assert!(
            src.contains("0x6a09e667"),
            "node:crypto arm missing FIPS-180-4 IV (0x6a09e667)"
        );
        assert!(
            src.contains("export default"),
            "node:crypto arm missing default export"
        );
    }

    #[test]
    fn crypto_shim_handles_opaque_path_shape() {
        // ModuleSpecifier::path() on a non-special scheme yields the opaque
        // path without a leading slash. node_shim extracts the arm name via
        // rsplit('/').next(), so both "shim/crypto" and "/shim/crypto" must
        // produce the arm.
        let with_slash = node_shim("/shim/crypto");
        let without_slash = node_shim("shim/crypto");
        assert!(
            with_slash.contains("export function createHash"),
            "/shim/crypto must produce crypto arm (opaque path trap)"
        );
        assert!(
            without_slash.contains("export function createHash"),
            "shim/crypto must produce crypto arm"
        );
    }

    #[test]
    fn crypto_shim_rejects_other_algorithms() {
        let src = node_shim("shim/crypto");
        // The loud-throw path is present — other algorithms throw with a
        // message naming the limitation rather than silently returning
        // garbage.
        assert!(
            src.contains("JSH-NODESHIM"),
            "crypto shim missing loud-throw for unsupported algorithms"
        );
        assert!(
            src.contains("sha256"),
            "crypto shim message must name sha256 explicitly"
        );
    }

    // ---- fs / fs/promises ----

    #[test]
    fn fs_promises_shim_exports_expected_names() {
        let src = node_shim("shim/fs/promises");
        for name in ["readFile", "mkdir", "readdir", "writeFile"] {
            assert!(
                src.contains(name),
                "node:fs/promises arm missing export: {}",
                name
            );
        }
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:fs/promises arm missing loud-throw"
        );
    }

    #[test]
    fn fs_shim_exports_sync_names() {
        let src = node_shim("shim/fs");
        assert!(
            src.contains("readFileSync"),
            "node:fs arm missing readFileSync"
        );
        assert!(src.contains("mkdirSync"), "node:fs arm missing mkdirSync");
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:fs arm missing loud-throw"
        );
    }

    // ---- util ----

    #[test]
    fn util_shim_has_parse_args() {
        let src = node_shim("shim/util");
        assert!(src.contains("parseArgs"), "node:util arm missing parseArgs");
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:util arm missing loud-throw"
        );
    }

    // ---- buffer / events / stream (import-safe, call-loud) ----

    #[test]
    fn buffer_shim_import_safe() {
        let src = node_shim("shim/buffer");
        assert!(src.contains("Buffer"), "node:buffer arm missing Buffer");
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:buffer arm missing loud-throw"
        );
    }

    #[test]
    fn events_shim_import_safe() {
        let src = node_shim("shim/events");
        assert!(
            src.contains("EventEmitter"),
            "node:events arm missing EventEmitter"
        );
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:events arm missing loud-throw"
        );
    }

    #[test]
    fn stream_shim_import_safe() {
        let src = node_shim("shim/stream");
        assert!(src.contains("Readable"), "node:stream arm missing Readable");
        assert!(src.contains("Writable"), "node:stream arm missing Writable");
        assert!(
            src.contains("JSH-NODESHIM"),
            "node:stream arm missing loud-throw"
        );
    }

    // ---- unsupported ----

    #[test]
    fn unsupported_builtin_throws_loudly() {
        let src = node_shim("shim/child_process");
        assert!(
            src.contains("JSH-NODESHIM"),
            "unsupported builtin must throw JSH-NODESHIM"
        );
        assert!(
            src.contains("throw"),
            "unsupported builtin must actually throw"
        );
    }

    // ---- requires_node_backend consistency ----

    #[test]
    fn requires_node_backend_lists_all_shimmed_builtins() {
        for name in [
            "module",
            "path",
            "url",
            "buffer",
            "events",
            "util",
            "stream",
            "crypto",
            "fs",
            "fs/promises",
        ] {
            assert!(
                requires_node_backend(&format!("node:{}", name)).is_none(),
                "node:{} must be shimmed (requires_node_backend should return None)",
                name
            );
        }
    }

    #[test]
    fn requires_node_backend_rejects_unshimmed_builtins() {
        for name in ["child_process", "worker_threads", "net", "http"] {
            assert!(
                requires_node_backend(&format!("node:{}", name)).is_some(),
                "node:{} must NOT be shimmed — it needs the Node backend",
                name
            );
        }
    }

    #[test]
    fn requires_node_backend_ignores_bare_specifiers() {
        // A bare (non-node:) specifier is not this function's concern.
        assert!(requires_node_backend("react").is_none());
        assert!(requires_node_backend("fs").is_none());
    }
}
