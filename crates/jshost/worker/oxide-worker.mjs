// oxide-worker.mjs — Oxide Node worker (spec §4.2, §4.6 tier 3, §4.7, §7).
// Speaks the length-prefixed JSON op protocol. Handles:
//   render                   → {html}
//   tooling:compile          → uppercase echo (test-only)
//   tooling:module           → require(call.module)[call.export](...call.args)
//   framework:load           → dynamic import of a config path → descriptor
//   framework:render         → dynamic import → adapter.render(input) → JSON chunked
//   loader:run               → dynamic import → load(event) → {ok, status, location, data}
//   framework:invalidate     → bump moduleGeneration → {generation: N}
//   generate-params:run      → import module, call generateParams → {params: [...]}
//   boom                     → CallErr
//   die                      → exit(1)
//   sleep                    → delayed reply
//   payload.chunked          → CallChunk* + CallEnd
// Pure Node ESM; no external deps.
import process from "node:process";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { readFileSync } from "node:fs";
import * as fs from "node:fs";
import * as pathMod from "node:path";


// Compiled modules are written to <projectRoot>/.oxide/transform/. Reasons
// for that specific location rather than os.tmpdir() or next-to-source:
//
// 1. Bare-specifier resolution. A compiled page imports "react", the
//    framework runtime, etc. Node resolves those by walking up from the
//    module's URL. From <projectRoot>/.oxide/transform/<hash>.ts the walk
//    finds <projectRoot>/node_modules (or the monorepo hoist above it) —
//    the same resolution the raw source got. From os.tmpdir() the walk
//    finds nothing and every import fails with `Cannot find package`.
// 2. The dev watcher's exclusion filter matches a `.oxide` path
//    *component*, so writes under <root>/.oxide/ don't self-trigger the
//    stale-generation guard. The old sibling location tripped it.
// 3. `src/` stays clean — the pollution complaint that motivated the
//    os.tmpdir() attempt.
//
// Content hash still keys the URL: an edit produces a new hash, hence a
// new URL, hence a fresh Node import. Same B-13 contract as before; only
// the medium changed.
const _transformDirs = new Map();

function findProjectRoot(sourcePath) {
  let dir = pathMod.dirname(sourcePath);
  for (;;) {
    if (
      fs.existsSync(pathMod.join(dir, ".oxide")) ||
      fs.existsSync(pathMod.join(dir, "oxide.config.ts")) ||
      fs.existsSync(pathMod.join(dir, "oxide.config.mjs")) ||
      fs.existsSync(pathMod.join(dir, "oxide.config.js")) ||
      fs.existsSync(pathMod.join(dir, "package.json"))
    ) {
      return dir;
    }
    const parent = pathMod.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

async function transformDir(sourcePath) {
  const root = findProjectRoot(sourcePath) ?? process.cwd();
  const cached = _transformDirs.get(root);
  if (cached) return cached;
  const dir = pathMod.join(root, ".oxide", "transform");
  fs.mkdirSync(dir, { recursive: true });
  _transformDirs.set(root, dir);
  return dir;
}

const require = createRequire(import.meta.url);
const sucrase = (() => { try { return require('sucrase') } catch { return null } })();
const solidJsxOxc = (() => {
  try {
    const m = require('solid-jsx-oxc')
    return m.default ?? m
  } catch {
    return null
  }
})();
const b64 = (buf) => buf.toString("base64");

// ---------------------------------------------------------------------------
// HMR cache invalidation (spec §13.3)
// ---------------------------------------------------------------------------
//
// Node caches ES modules by URL. To force re-evaluation after a file change,
// we tag every dynamic import with `?v=<generation>`. The generation is bumped
// by the `framework:invalidate` op, which the dev server's file watcher calls
// when a source file it cares about changes. Outside of dev, the generation
// stays at 0 and no invalidation happens — imports and caching behave
// normally.
let moduleGeneration = 0;

/**
 * Append the current generation to a URL. `file:///x.mjs` becomes
 * `file:///x.mjs?v=0` and every subsequent bump produces a fresh URL, so
 * Node treats each generation as a distinct module.
 */
// SPEC-14: the generation is written to `.oxide/hmr-generation` by the
// Rust watcher on every invalidate. Reading it fresh on EVERY versioned()
// call is what makes HMR work across the pool: NodePool dispatches renders
// to any of N workers by least-loaded, so an in-memory counter bumped on
// ONE worker by `framework:invalidate` leaves the other N-1 on the old
// generation. Reading from disk means every worker sees every edit.
const HMR_GEN_PATH = process.env.OXIDE_ROOT
  ? `${process.env.OXIDE_ROOT}/.oxide/hmr-generation`
  : null;
function currentGeneration() {
  if (!HMR_GEN_PATH) return moduleGeneration;
  try {
    const v = readFileSync(HMR_GEN_PATH, "utf8").trim();
    const n = parseInt(v, 10);
    return Number.isFinite(n) && n > 0 ? n : moduleGeneration;
  } catch {
    return moduleGeneration;
  }
}
function versioned(url) {
  const sep = url.includes("?") ? "&" : "?";
  return `${url}${sep}v=${currentGeneration()}`;
}
// PERF-12: frame = [4-byte len LE][1-byte kind][payload]. len covers
// kind+payload. kind 0 = JSON, kind 1 = binary chunk ([8-byte id LE][raw]).
const out = (obj) => {
  const body = Buffer.from(JSON.stringify(obj), "utf8");
  const head = Buffer.alloc(5);
  head.writeUInt32LE(body.length + 1, 0);
  head.writeUInt8(0, 4); // kind 0 = JSON
  process.stdout.write(Buffer.concat([head, body]), (e) => e && process.exit(0));
};

/// PERF-12: write a raw chunk as a binary frame (no base64, no JSON wrapper).
const outBinChunk = (id, buf) => {
  const payloadLen = 8 + buf.length;
  const head = Buffer.alloc(5);
  head.writeUInt32LE(payloadLen + 1, 0);
  head.writeUInt8(1, 4); // kind 1 = binary chunk
  const idBuf = Buffer.alloc(8);
  idBuf.writeBigUInt64LE(BigInt(id), 0);
  process.stdout.write(Buffer.concat([head, idBuf, buf]), (e) => e && process.exit(0));
};
const diagnostic = (code, message) => ({
  code, severity: "error", message, location: null, code_frame: null, causes: [], fix: null,
});
const callErr = (id, code, message) => out({ t: "call_err", id, diagnostic: diagnostic(code, message) });
const callOk = (id, value) =>
  out({ t: "call_ok", id, chunks: [b64(Buffer.from(JSON.stringify(value), "utf8"))], complete: true });

// ---------------------------------------------------------------------------
// §4.5 / §9.3: worker-originated ops.
//
// The runtime (user code) reaches the host through `__oxide_op_request`. It
// sends an `op_request` frame and awaits the matching `op_response` on
// stdin. Only the three cache ops are handled by the host today; any other
// op answers an error the caller can fall back on.
//
// The map is keyed by a monotonic id. A response arriving for an id with no
// pending entry is a late reply to a call whose timeout already fired —
// dropped, not an error.
const pendingOps = new Map();
let nextOpId = 1;

function oxideOpRequest(op, args) {
  return new Promise((resolve, reject) => {
    const id = nextOpId++;
    pendingOps.set(id, { resolve, reject });
    // `Op` is a serde externally-tagged enum: the wire is the variant
    // name, not its `#[repr(u16)]` value.
    out({ t: "op_request", id, op: opName(op), args: args ?? {} });
  });
}

// The wire is the `Op` variant name — serde's externally-tagged enum
// serializes a unit variant as its name string, not its numeric repr.
function opName(name) {
  switch (name) {
    case "stream_write": return "StreamWrite";
    case "response_head": return "ResponseHead";
    case "request_body_read": return "RequestBodyRead";
    case "serialize": return "Serialize";
    case "cache_get": return "CacheGet";
    case "cache_set": return "CacheSet";
    case "cache_tag": return "CacheTag";
    case "fetch": return "Fetch";
    case "ws_send": return "WsSend";
    case "log": return "Log";
    case "trace": return "Trace";
    case "compile_module": return "CompileModule";
    default: return name;
  }
}

globalThis.__oxide_op_request = oxideOpRequest;

// §20: the inbound request's W3C `traceparent`, stashed for the duration of
// one op so the `fetch` shim can propagate it to outgoing requests. The
// worker runs one op at a time, so a module-level variable is per-request
// correct. Cleared after each dispatch.
let currentTraceparent = null;

const TRACEPARENT_RE = /^[0-9a-f]{2}-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/;

/**
 * Pull `traceparent` out of a payload. S-14: prefer the host's *validated*
 * value (`payload.trace.traceparent`), and only accept a raw header when it
 * matches the W3C shape — garbage/oversized values are dropped rather than
 * forwarded verbatim to upstream APM pipelines.
 */
function traceparentFromPayload(payload) {
  const validated = payload && payload.trace && payload.trace.traceparent;
  if (typeof validated === "string" && TRACEPARENT_RE.test(validated)) {
    return validated;
  }
  const hs = payload && payload.headers;
  if (!Array.isArray(hs)) return null;
  for (const pair of hs) {
    if (Array.isArray(pair) && pair.length === 2 &&
        String(pair[0]).toLowerCase() === "traceparent") {
      const raw = String(pair[1]);
      return TRACEPARENT_RE.test(raw) ? raw : null;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// PERF-05: a `fetch` shim. User code's server-side `fetch()` routes through
// the `fetch` op so it goes through the host's SSRF-hardened path (the same
// one the image endpoint uses). A host that has no fetch handler answers
// `JSH-OP`; we then fall back to Node's global `fetch` **with a warning**, so
// a project is never broken by a missing handler — it just loses the guard.
//
// Same-origin + allow-listed URLs are the common case; we route *every*
// request through the op (the host decides what is allowed), which is the
// safe default. Set OXIDE_FETCH_NATIVE=1 to bypass the op entirely (tests).
const __nativeFetch = globalThis.fetch ? globalThis.fetch.bind(globalThis) : undefined;
let __fetchFallbackWarned = false;

function __headersToObject(h) {
  const o = {};
  try {
    if (h && typeof h.forEach === "function") h.forEach((v, k) => { o[k] = v; });
    else if (Array.isArray(h)) for (const [k, v] of h) o[k] = v;
    else if (h && typeof h === "object") for (const k of Object.keys(h)) o[k] = String(h[k]);
  } catch { /* ignore malformed headers */ }
  return o;
}

globalThis.fetch = async function oxideFetch(input, init = {}) {
  // §17.2: `--offline-plugins` — deny every outbound request from a worker
  // spawned under the flag. A plugin/tooling module cannot phone home.
  if (process.env.OXIDE_OFFLINE_PLUGINS === "1") {
    const e = new Error("outbound network disabled by --offline-plugins");
    e.code = "JSH-OFFLINE";
    throw e;
  }
  const url = typeof input === "string" ? input : (input && input.url) || String(input);
  const method = (init.method || (input && input.method) || "GET").toUpperCase();
  const headers = __headersToObject(init.headers || (input && input.headers));
  let bodyB64 = "";
  const body = init.body;
  if (body != null) {
    const bytes =
      typeof body === "string"
        ? Buffer.from(body, "utf8")
        : body instanceof Uint8Array
        ? Buffer.from(body)
        : Buffer.from(String(body), "utf8");
    bodyB64 = bytes.toString("base64");
  }
  if (process.env.OXIDE_FETCH_NATIVE === "1" && __nativeFetch) {
    return __nativeFetch(input, init);
  }
  // §20: propagate the inbound trace to the outbound request. A caller that
  // set its own `traceparent` wins.
  if (currentTraceparent && !Object.keys(headers).some((k) => k.toLowerCase() === "traceparent")) {
    headers["traceparent"] = currentTraceparent;
  }
  try {
    const res = await oxideOpRequest("fetch", { url, method, headers, body_b64: bodyB64 });
    const bytes = Buffer.from(res.body_b64 || "", "base64");
    const h = new Headers();
    // Rust serializes `Vec<(String,String)>` as `[["k","v"], ...]`.
    const raw = res.headers || [];
    if (Array.isArray(raw)) {
      for (const pair of raw) {
        if (Array.isArray(pair) && pair.length === 2) h.append(pair[0], pair[1]);
      }
    } else if (typeof raw === "object") {
      for (const k of Object.keys(raw)) h.append(k, String(raw[k]));
    }
    return new Response(bytes, { status: res.status, headers: h });
  } catch (e) {
    // S-01: only an *unregistered* op (a legacy host with no fetch handler)
    // may fall back to native fetch. A `JSH-FETCH` error IS the SSRF guard
    // (or a real fetch failure) and must propagate, never be retried via the
    // unguarded native path.
    const code = e && typeof e.code === "string" ? e.code : "";
    if (code === "JSH-OP" || code === "JSH-NOKIND") {
      if (!__fetchFallbackWarned) {
        __fetchFallbackWarned = true;
        console.error("[oxide] fetch op unavailable (" + (e && e.message) + "); falling back to native fetch (no SSRF guard)");
      }
      if (__nativeFetch) return __nativeFetch(input, init);
    }
    throw e;
  }
};

// Cache required modules across calls (CJS path).
const modCache = new Map();
// Server-fn graph imports, keyed by (entry path, module generation).
const serverFnGraphCache = new Map();
function loadModule(name) {
  if (!modCache.has(name)) {
    try {
      modCache.set(name, { ok: require(name) });
    } catch (e) {
      modCache.set(name, { err: e });
    }
  }
  return modCache.get(name);
}

// ---------------------------------------------------------------------------
// §6.3 SFC compilation ops.
//
// These are custom ops rather than `tooling:module` calls because the SFC
// compiler APIs are stateful multi-step (parse → compileScript /
// compileTemplate) and their intermediate results are not cleanly
// JSON-serializable. Running the whole pipeline in-process avoids the
// round-trip and keeps the tooling-pool protocol generic.
// ---------------------------------------------------------------------------

async function dispatchSolidCompile(id, input) {
  const { source, filename, ssr } = input.payload ?? {};
  if (typeof source !== "string" || typeof filename !== "string") {
    callErr(id, "JSH-SOLID", "solid-compile requires {source, filename}");
    return;
  }
  if (!solidJsxOxc || typeof solidJsxOxc.transform !== "function") {
    callErr(
      id,
      "JSH-SOLID",
      "solid-compile requires `solid-jsx-oxc` in the workspace (Rust/OXC Solid JSX compiler)",
    );
    return;
  }
  // `solid-jsx-oxc` compiles `<main>…</main>` JSX into `ssr`-template
  // literals — same shape as `babel-preset-solid`, but the transform is
  // Rust/OXC-native (no babel, no per-file JS glue). Its output preserves
  // TypeScript type annotations in function signatures (it parses `.tsx`
  // but does not strip types), so a type-strip pass runs on the result.
  // By this point the JSX is already gone, so plain sucrase `typescript`
  // is enough and cannot trip on JSX syntax.
  // Solid 1.x's runtime lived at `solid-js/web`; Solid 2.0 moved it to
  // the `@solidjs/web` package. `solid-jsx-oxc`'s default `moduleName`
  // points at the 1.x path, which no longer exists in a 2.0 workspace.
  // Passing the 2.0 name is what makes the emitted `import { ssr } from
  // "@solidjs/web"` resolvable by Rolldown.
  let out;
  try {
    out = solidJsxOxc.transform(source, {
      moduleName: "@solidjs/web",
      generate: ssr ? "ssr" : "dom",
      hydratable: true,
      filename,
    });
  } catch (e) {
    callErr(id, "JSH-SOLID", `solid transform: ${e.message ?? e}`);
    return;
  }
  let js = typeof out === "string" ? out : out?.code;
  if (typeof js !== "string") {
    callErr(id, "JSH-SOLID", "solid transform produced no code");
    return;
  }
  // `solid-jsx-oxc@0.1.0-alpha.1` ignores `moduleName` and always emits
  // `from "solid-js/web"` — the Solid 1.x runtime location. Solid 2.0
  // moved the runtime to `@solidjs/web`. Rewriting the import specifier
  // here keeps the compiler choice (Rust/OXC, no babel) and matches what
  // the project actually has installed. When upstream honors `moduleName`,
  // this rewrite becomes a no-op (the same string).
  js = js.replaceAll('from "solid-js/web"', 'from "@solidjs/web"')
         .replaceAll("from 'solid-js/web'", "from '@solidjs/web'");
  if (sucrase) {
    try {
      js = sucrase.transform(js, {
        transforms: ["typescript"],
        production: true,
        filePath: filename,
      }).code;
    } catch (e) {
      callErr(id, "JSH-SOLID", `solid type-strip: ${e.message ?? e}`);
      return;
    }
  }
  callOk(id, { js });
}

async function dispatchVueCompile(id, input) {
  const { source, filename, generate } = input.payload ?? {};
  if (typeof source !== "string" || typeof filename !== "string") {
    callErr(id, "JSH-VUE", "vue-compile requires {source, filename}");
    return;
  }
  let vue;
  try { vue = require("@vue/compiler-sfc"); }
  catch (e) { callErr(id, "JSH-VUE", `require(@vue/compiler-sfc): ${e.message}`); return; }
  const parsed = vue.parse(source, { filename });
  if (parsed.errors && parsed.errors.length > 0) {
    callErr(id, "JSH-VUE", `vue parse: ${parsed.errors[0].message ?? parsed.errors[0]}`);
    return;
  }
  const shortId = Buffer.from(filename).toString("base64").replace(/[^A-Za-z0-9]/g, "").slice(0, 8);
  let script;
  try {
    // `inlineTemplate: true` produces one render-function-bearing JS blob
    // we can hand straight to the bundler. The template is compiled by
    // `@vue/compiler-dom` internally.
    script = vue.compileScript(parsed.descriptor, {
      id: shortId,
      inlineTemplate: true,
      // The SSR `generate` mode also uses the browser compiler under the
      // hood; the difference is the render-function shape. Pass through
      // the caller's mode so the server bundle uses SSR render fns.
      templateOptions: { ssr: generate === "ssr" },
    });
  } catch (e) {
    callErr(id, "JSH-VUE", `vue compileScript: ${e.message ?? e}`);
    return;
  }
  // `<script setup lang="ts">` preserves TypeScript annotations in the
  // output. The bundler's Rolldown pipeline expects JS here; strip types
  // with sucrase (the same transform the dev loader uses).
  let js = script.content;
  if (sucrase) {
    try {
      js = sucrase.transform(js, {
        transforms: ["typescript"],
        production: true,
        filePath: filename,
      }).code;
    } catch (e) {
      callErr(id, "JSH-VUE", `vue type-strip: ${e.message ?? e}`);
      return;
    }
  }
  callOk(id, { js });
}

async function dispatchSvelteCompile(id, input) {
  const { source, filename, generate } = input.payload ?? {};
  if (typeof source !== "string" || typeof filename !== "string") {
    callErr(id, "JSH-SVELTE", "svelte-compile requires {source, filename}");
    return;
  }
  let svelte;
  try { svelte = require("svelte/compiler"); }
  catch (e) { callErr(id, "JSH-SVELTE", `require(svelte/compiler): ${e.message}`); return; }
  let compiled;
  try {
    compiled = svelte.compile(source, {
      filename,
      generate: generate === "ssr" ? "ssr" : "dom",
      css: "injected",
    });
  } catch (e) {
    callErr(id, "JSH-SVELTE", `svelte compile: ${e.message ?? e}`);
    return;
  }
  let js = compiled?.js?.code ?? "";
  // `<script lang="ts">` passes type annotations through to the emitted
  // JS. Strip them the same way the Vue op does.
  if (sucrase) {
    try {
      js = sucrase.transform(js, {
        transforms: ["typescript"],
        production: true,
        filePath: filename,
      }).code;
    } catch {
      // Svelte's output is already valid JS in the common case; if
      // sucrase cannot parse it, keep the original rather than fail.
    }
  }
  callOk(id, { js });
}

async function dispatchToolingModule(id, input) {
  const { module: name, export: fn, args } = input.payload ?? {};
  const entry = loadModule(name);
  if (entry.err) {
    callErr(id, "JSH-TOOLING", `require(${name}) failed: ${entry.err.message}`);
    return;
  }
  const target = entry.ok?.[fn];
  if (typeof target !== "function") {
    callErr(id, "JSH-TOOLING", `${name}.${fn} is not a function`);
    return;
  }
  try {
    let result = await target(...(args ?? []));
    // Strip non-JSON-encodable values (functions, circular refs); SFC compilers
    // return compiler output that is usually JSON-safe, but not always.
    const json = JSON.parse(JSON.stringify(result, (_k, v) =>
      typeof v === "function" ? undefined : v
    ));
    callOk(id, json);
  } catch (e) {
    callErr(id, "JSH-TOOLING", `call ${name}.${fn} threw: ${e?.message ?? e}`);
  }
}

// --- Framework adapter ops (spec §7) -------------------------------------
//
// Load a user's oxide.config.ts (or .mjs) and route render() calls through
// the FrameworkAdapter it exports. This is the JS side of the contract
// documented in packages/oxide/src/server/README.md — the CLI consumes it
// via `FrameworkHost` (crates/jshost/src/framework_host.rs).
//
// Ops:
//   framework:load     { path }                      → { name, extensions, capabilities }
//   framework:render   { path, input, components }   → { html } (streamed)
//   components:load    { entry }                     → { generation, ids }
//
// §7 / §6.4: the config supplies the framework *descriptor* only. Route
// components come from a build-emitted entry (`dist/server/ssr/index.js`) in
// production, or from a CLI-generated source map in dev. The config is
// imported for the adapter; it is never the component graph root.
//
// The config module is cached per `moduleGeneration`; `framework:invalidate`
// bumps the generation (the watcher does this on a config edit), so dev edits
// take effect without paying a config import on every render.

// ---------------------------------------------------------------------------
// SPEC-17 / §15.1: the JS plugin bridge. A plugin module exports hook
// functions (by camelCase name). `plugin:load` imports it and records which
// hooks exist; `plugin:call` invokes one with the given args and returns its
// result (or an error envelope).
const pluginCache = new Map(); // modulePath -> { mod, hooks: Set<string> }

async function dispatchPluginLoad(id, input) {
  const path = String(input?.payload?.path ?? "");
  if (!path) {
    callErr(id, "PLG-LOAD", "plugin:load requires a `path`");
    return;
  }
  try {
    const url = pathToFileURL(path).href;
    const mod = await import(versioned(url));
    const plugin = mod.default ?? mod;
    const hooks = Object.keys(plugin).filter((k) => typeof plugin[k] === "function");
    pluginCache.set(path, { mod: plugin, hooks: new Set(hooks) });
    callOk(id, { hooks, name: plugin.name ?? null });
  } catch (e) {
    callErr(id, "PLG-LOAD", `plugin import failed for ${path}: ${e?.message ?? e}`);
  }
}

/**
 * Documented positional argument order per hook (SPEC-15.2). The Rust side
 * sends a named object; this maps it to the argument list a plugin author
 * writes. A hook not listed here falls back to `Object.values(args)`, which
 * is correct for a single-argument hook whose object has one key.
 */
const PLUGIN_HOOK_ARGS = {
  transform: ["code", "id"],
  resolveId: ["specifier", "from"],
  load: ["id"],
  renderChunk: ["file", "code"],
  generateBundle: ["files"],
  transformHtml: ["route", "html"],
  config: ["config"],
  configResolved: ["config"],
  buildStart: [],
  buildEnd: ["result"],
  handleHotUpdate: ["ctx"],
};

async function dispatchPluginCall(id, input) {
  const path = String(input?.payload?.path ?? "");
  const hook = String(input?.payload?.hook ?? "");
  const args = input?.payload?.args ?? {};
  const entry = pluginCache.get(path);
  if (!entry) {
    callErr(id, "PLG-LOAD", `plugin not loaded: ${path} (call plugin:load first)`);
    return;
  }
  const fn = entry.mod[hook];
  if (typeof fn !== "function") {
    callErr(id, "PLG-HOOK", `plugin ${path} has no \`${hook}\` hook`);
    return;
  }
  try {
    // Vite-style hook context: a minimal object the common hooks read.
    const ctx = {
      warn: (m) => console.error("[plugin]", m),
      error: (m) => { throw new Error(String(m)); },
      resolve: async (spec) => spec,
    };
    // Hooks are POSITIONAL on the plugin side — `transform(code, id)`,
    // `resolveId(specifier, from)`, `load(id)`. The Rust side sends a named
    // object (`{code, id}`); spreading `Object.values(args)` is not safe
    // because serde_json sorts keys alphabetically workspace-wide, so
    // `{specifier, from}` would arrive as `(from, specifier)`. This table
    // names the documented order per hook.
    const order = PLUGIN_HOOK_ARGS[hook];
    const callArgs = order
      ? order.map((k) => args?.[k])
      : Object.values(args ?? {});
    const out = await fn.call(ctx, ...callArgs);
    callOk(id, out === undefined ? null : out);
  } catch (e) {
    callErr(id, "PLG-CALL", `plugin hook ${hook} threw: ${e?.message ?? e}`);
  }
}

// §6.4 / WP-B3: the config supplies only the adapter descriptor. Import it
// once per module generation (the watcher bumps `moduleGeneration` on a
// config edit), never once per request — sequential renders reuse the cached
// module instead of re-evaluating the config graph each time.
const frameworkConfigCache = new Map(); // `${path}:gen${generation}` -> Promise<object>
function loadFrameworkConfig(path) {
  // §6.4 / WP-r9-5: key on the DISK generation, not this worker's in-memory
  // `moduleGeneration`. `framework:invalidate` bumps the counter on exactly
  // one worker; the pool dispatches renders to any of N, so keying on the
  // per-worker counter left the other N-1 serving the stale config forever.
  // `currentGeneration()` reads `.oxide/hmr-generation` fresh (the same
  // reason `versioned()` uses it).
  const key = `${path}:gen${currentGeneration()}`;
  if (!frameworkConfigCache.has(key)) {
    for (const k of frameworkConfigCache.keys()) {
      if (k.startsWith(`${path}:`)) frameworkConfigCache.delete(k);
    }
    frameworkConfigCache.set(key, import(versioned(pathToFileURL(path).href)));
  }
  return frameworkConfigCache.get(key);
}

async function dispatchFrameworkLoad(id, input) {
  const { path } = input.payload ?? {};
  if (typeof path !== "string" || path.length === 0) {
    callErr(id, "JSH-FRAMEWORK", "framework:load requires a `path` string");
    return;
  }
  try {
    const mod = await loadFrameworkConfig(path);
    const cfg = mod.default ?? mod;
    const framework = cfg?.framework;
    if (!framework || typeof framework !== "object") {
      callErr(id, "JSH-FRAMEWORK", `config at ${path} has no framework export`);
      return;
    }
    const descriptor = {
      name: String(framework.name ?? ""),
      extensions: Array.isArray(framework.extensions) ? framework.extensions.map(String) : [],
      capabilities: framework.capabilities ?? {},
    };
    callOk(id, descriptor);
  } catch (e) {
    callErr(id, "JSH-FRAMEWORK", `framework:load failed: ${e?.message ?? e}`);
  }
}

// §6.4 / WP-B1: the component resolver. The generated entry exports
// `routes = { "<route id>": moduleNamespace }`; `components:load` imports it
// once per module generation and `framework:render` hands the adapter a
// synchronous resolver over that map.
const componentEntryCache = new Map(); // `${entry}:gen${generation}` -> Promise

function componentExport(ns) {
  if (!ns || typeof ns !== "object") return ns;
  return ns.default ?? ns.Page ?? ns.page ?? ns.Component ?? ns.component ?? ns;
}

function resolverFor(routes) {
  return (id) => (routes && Object.prototype.hasOwnProperty.call(routes, id)
    ? componentExport(routes[id])
    : undefined);
}

// §6.4 / Wave M: the SSR entry may be eager (modules) or lazy (loader fns).
// We normalize to loader fns so the render path only imports what it needs.
// A route module in the entry can be:
//   eager:  export const routes = { "id": ns }         (legacy)
//   lazy:   export const routes = { "id": () => import("./x.js") }   (current)
// This helper imports the manifest (cheap — no route modules load), then
// normalizes so callers can await only the ones they need.
async function loadComponentManifest(entryPath) {
  const key = `manifest:${entryPath}:gen${moduleGeneration}`;
  if (!componentEntryCache.has(key)) {
    componentEntryCache.set(key, (async () => {
      const mod = await import(versioned(pathToFileURL(entryPath).href));
      const routes = mod?.routes ?? {};
      const normalized = {};
      for (const [id, v] of Object.entries(routes)) {
        const loader = (typeof v === 'function') ? v : () => Promise.resolve(v);
        normalized[id] = loader;
        // §6.4 / WP-A2: rolldown names entries after their file basename,
        // which sanitizes `[slug]` -> `_slug_`. The pipeline sends the
        // route id in its source shape (`routes/blog/[slug]`), from
        // `canonical_route_id` (which only strips `/page`). Alias both
        // shapes here so neither a bundled build nor a dev build can
        // miss the lookup — the miss otherwise surfaces as a 500 on
        // every dynamic route.
        const bracketForm = id.replace(/_([^_/]+)_/g, '[$1]');
        if (bracketForm !== id && !(bracketForm in normalized)) {
          normalized[bracketForm] = loader;
        }
        const underscoreForm = id.replace(/\[([^\]/]+)\]/g, '_$1_');
        if (underscoreForm !== id && !(underscoreForm in normalized)) {
          normalized[underscoreForm] = loader;
        }
      }
      return normalized;
    })());
  }
  return componentEntryCache.get(key);
}

// Import only the given ids, in parallel. Missing ids are skipped (a
// route pointing at a layout the entry does not declare is a benign miss).
async function loadNeededComponents(manifest, neededIds) {
  const out = {};
  await Promise.all([...neededIds].map(async (id) => {
    if (typeof id !== 'string' || id.length === 0) return;
    const loader = manifest[id];
    if (typeof loader !== 'function') return;
    try {
      out[id] = await loader();
    } catch (e) {
      // A missing route module is a resolver miss the adapter will report;
      // a malformed one (syntax, missing dep) is a build bug worth a line.
      console.error(`[oxide] component import failed for ${id}:`, e?.message ?? e);
    }
  }));
  return out;
}

async function dispatchComponentsLoad(id, input) {
  const entry = String(input?.payload?.entry ?? "");
  if (!entry) {
    callErr(id, "JSH-COMPONENTS", "components:load requires an `entry` string");
    return;
  }
  try {
    const routes = await loadComponentEntry(entry);
    callOk(id, { generation: moduleGeneration, ids: Object.keys(routes) });
  } catch (e) {
    callErr(id, "JSH-COMPONENTS", `components:load failed: ${e?.message ?? e}`);
  }
}

async function dispatchFrameworkRender(id, input) {
  const { path, input: renderInput } = input.payload ?? {};
  // §6.4 / WP-B3: accept `components` at the payload level (the current
  // wire shape) OR nested inside `input` (backward compatibility for any
  // older client); the Rust side hoists it to the payload level.
  const components = input.payload?.components ?? renderInput?.components;
  if (typeof path !== "string" || path.length === 0) {
    callErr(id, "JSH-FRAMEWORK", "framework:render requires a `path` string");
    return;
  }
  try {
    const mod = await loadFrameworkConfig(path);
    const cfg = mod.default ?? mod;
    const framework = cfg?.framework;
    if (!framework || typeof framework.render !== "function") {
      callErr(id, "JSH-FRAMEWORK", `config at ${path} has no framework.render`);
      return;
    }
    // §6.4: build the resolver BEFORE render. Production imports the build
    // entry once per generation; dev imports the CLI's source map (the
    // tsx/sfc loader stack compiles each module), also versioned by the
    // generation the watcher bumps.
    let resolver = null;
    if (components && typeof components.entry === "string") {
      // §6.4 / Wave M: manifest import is cheap; only the route's actual
      // module (and its layouts + error boundary) is imported. This is
      // the TTFB fix — one render used to import the whole 12-module entry.
      const manifest = await loadComponentManifest(components.entry);
      const needed = new Set(
        [renderInput?.route,
         ...(Array.isArray(renderInput?.layouts) ? renderInput.layouts : []),
         renderInput?.errorRoute]
          .filter((x) => typeof x === "string" && x.length > 0)
      );
      const loaded = await loadNeededComponents(manifest, needed);
      resolver = resolverFor(loaded);
    } else if (components && components.sources && typeof components.sources === "object") {
      const routes = {};
      const wanted = new Set([
        renderInput?.route,
        ...(Array.isArray(renderInput?.layouts) ? renderInput.layouts : []),
        renderInput?.errorRoute,
      ].filter((x) => typeof x === "string" && x.length > 0));
      for (const rid of wanted) {
        const src = components.sources[rid];
        if (typeof src !== "string" || src.length === 0) continue;
        try {
          routes[rid] = await import(versioned(pathToFileURL(src).href));
        } catch (e) {
          callErr(
            id,
            "JSH-COMPONENTS",
            `dev component import failed for ${rid} (${src}): ${e?.message ?? e}`,
          );
          return;
        }
      }
      resolver = resolverFor(routes);
    }
    const rinput = renderInput ?? {};
    if (resolver) rinput.resolve = resolver;
    const stream = framework.render(rinput, resolver ?? undefined);
    // Emit the adapter's ReadableStream<Uint8Array> as chunked frames of
    // RAW HTML bytes — no JSON envelope. Rust consumers (render for the
    // collected shape and the per-settle streaming shape (WP-B1)) both
    // get the same HTML; the difference is only whether they buffer.
    //
    // Chunk size: 8 KB. Small enough for reasonable TTFB once we wire a
    // truly streaming adapter (React 19, Solid resources), large enough
    // that per-frame base64 + JSON framing overhead stays negligible.
    // Send each adapter chunk as its own `call_chunk` frame. The adapter's
    // `ReadableStream<Uint8Array>` is already progressive — a React
    // `renderToPipeableStream` yields the shell before boundaries, our IR
    // adapter yields head, body, then one frame per deferred resolution.
    // Preserving those boundaries on the wire means a caller that consumes
    // `render_streaming` sees the head before the boundary resolutions —
    // the TTFB property streaming exists for. A caller that collects
    // (FrameworkHost::render) still gets identical bytes after concatenation.
    for await (const chunk of stream) {
      const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      // PERF-12: raw bytes ride a binary frame (no base64 overhead).
      outBinChunk(id, Buffer.from(buf));
    }
    out({ t: "call_end", id });
  } catch (e) {
    callErr(id, "JSH-FRAMEWORK", `framework:render failed: ${e?.message ?? e}`);
  }
}

// --- Loader ops (spec §9.1) ----------------------------------------------
//
//   loader:run   { module_path, route, params, url, method, headers }
//              → { ok, status, location, data }
//
// Imports the user's `page.server.ts` (or any `.server.ts` with a `load`
// export), calls its `load(event)`, and returns the (JSON) result. `error()`
// and `redirect()` sentinels thrown by the load become {status, location}
// in the response rather than worker errors — the pipeline maps those to
// HTTP responses.
// ---------------------------------------------------------------------------
// §9.3 / §6.2: server-module compilation for the loader path.
//
// `dispatchLoaderRun` imports a `.server.ts` file directly, bypassing the
// bundler — so the compiler's `'use cache'` and server-fn passes never run
// on it. The fix walks the module's whole import graph: every relative
// `.ts`/`.tsx` import reachable from the entry is compiled through the
// `compile_module` op, written as a hidden `.oxide.mjs` temp file beside
// its source, and every relative import specifier inside a compiled file is
// rewritten to point at the compiled temp file of its target.
const serverModuleCache = new Map();

// B-13: per-module compiled result, keyed by the content+path hash. An
// unchanged module reuses both the compiled code and its temp path, so a
// repeat loader/action/endpoint/socket request skips the `compile_module`
// IPC round-trip (and the write, when the temp file is still on disk).
const compiledModuleCache = new Map();
const COMPILED_MAX = 512;
function rememberCompiled(hash, info) {
  if (compiledModuleCache.has(hash)) compiledModuleCache.delete(hash);
  compiledModuleCache.set(hash, info);
  if (compiledModuleCache.size > COMPILED_MAX) {
    const [oldest] = compiledModuleCache.keys();
    compiledModuleCache.delete(oldest);
  }
}

function scanRelativeSpecifiers(source) {
  const out = [];
  const patterns = [
    /from\s*["'](\.\.?\/[^"']+)["']/g,
    /import\s*\(\s*["'](\.\.?\/[^"']+)["']\s*\)/g,
    /import\s+["'](\.\.?\/[^"']+)["']/g,
  ];
  for (const re of patterns) {
    let m;
    while ((m = re.exec(source)) !== null) {
      out.push({ spec: m[1] });
    }
  }
  return out;
}

async function resolveRelative(fromPath, spec, pathMod) {
  const fs = await import("node:fs/promises");
  const base = pathMod.resolve(pathMod.dirname(fromPath), spec);
  const candidates = [
    base,
    base + ".ts",
    base + ".tsx",
    base + ".js",
    base + ".mjs",
    base + ".css",
    pathMod.join(base, "index.ts"),
    pathMod.join(base, "index.tsx"),
  ];
  for (const cand of candidates) {
    try {
      const st = await fs.stat(cand);
      if (st.isFile()) return cand;
    } catch { /* next */ }
  }
  return null;
}

async function compileServerModule(entryPath) {
  const fs = await import("node:fs/promises");
  const crypto = await import("node:crypto");

  if (typeof globalThis.__oxide_op_request !== "function") {
    return { url: pathToFileURL(entryPath).href, isTemp: false };
  }

  // B-13 fast path: reuse the previous graph's temp files when every
  // module still has the same mtime+size. No read, no compile, no write.
  const prev = serverModuleCache.get(entryPath);
  if (prev?.graph) {
    let fresh = true;
    for (const g of prev.graph) {
      try {
        const st = await fs.stat(g.path);
        if (st.mtimeMs !== g.mtimeMs || st.size !== g.size) {
          fresh = false;
          break;
        }
      } catch {
        fresh = false;
        break;
      }
    }
    if (fresh) return { url: prev.tmpUrl, isTemp: true };
  }

  const modules = new Map();
  const order = [];
  const graph = [];
  const queue = [entryPath];
  while (queue.length > 0) {
    const path = queue.shift();
    if (modules.has(path)) continue;
    const source = await fs.readFile(path, "utf8");
    try {
      const st = await fs.stat(path);
      graph.push({ path, mtimeMs: st.mtimeMs, size: st.size });
    } catch { /* unreadable now; forces a recompile next time */ }
    const deps = [];
    for (const { spec } of scanRelativeSpecifiers(source)) {
      if (spec.includes("?")) continue;
      const dep = await resolveRelative(path, spec, pathMod);
      if (dep === null) continue;
      if (dep.includes(`${pathMod.sep}node_modules${pathMod.sep}`)) continue;
      deps.push({ spec, path: dep });
      queue.push(dep);
    }
    modules.set(path, { source, deps });
    order.push(path);
  }

  const tempOf = new Map();
  for (const path of order) {
    const { source } = modules.get(path);
    // BUG-08: CSS is a server side-effect import — empty module, no compile.
    // NOTE: tmpPath must be a NEW temp file, never the original .css path
    // (the loop below writes every tmpPath to disk).
    if (path.endsWith(".css")) {
      // WP-C: the CSS stub is a scratch module; write it into the per-worker
      // transform dir, not next to the source. Hash by path so distinct
      // stylesheets land in distinct files in the shared scratch dir.
      const cssHash = crypto
        .createHash("sha256")
        .update("css:\u0000")
        .update(path)
        .digest("hex")
        .slice(0, 16);
      const tDir = await transformDir(path);
      const cssTmp = pathMod.join(tDir, `css-${cssHash}.mjs`);
      tempOf.set(path, { tmpPath: cssTmp, tmpUrl: pathToFileURL(cssTmp).href, content: 'export default "";', hash: "css" });
      continue;
    }
    const hash = crypto
      .createHash("sha256")
      .update(source)
      .update("\u0000")
      .update(path)
      .digest("hex")
      .slice(0, 16);
    const known = compiledModuleCache.get(hash);
    if (known) {
      // Refresh LRU position and reuse the previous compile.
      rememberCompiled(hash, known);
      tempOf.set(path, { ...known, fromCache: true });
      continue;
    }
    let compiled;
    try {
      const result = await globalThis.__oxide_op_request("compile_module", {
        code: source,
        path,
        lang: path.endsWith(".tsx") ? "tsx" : "ts",
      });
      compiled = result?.code;
    } catch (e) {
      throw new Error(`compile_module failed for ${path}: ${e?.message ?? e}`);
    }
    if (typeof compiled !== "string") {
      throw new Error(`compile_module returned no code for ${path}`);
    }
    // WP-C: emit into the per-worker scratch dir, never next to the source.
    // `.ts` (not `.mjs`): `transform_passes_only` runs the AST passes but
    // not the TypeScript strip, so the emitted module still carries type
    // annotations. Node's native type-stripper handles a `.ts` file on
    // import; a `.mjs` file would be parsed as-is and fail.
    //
    // The hash already mixes in `path` (see the `crypto.createHash("sha256")`
    // call above), so a flat hash-named file cannot collide across modules.
    const tDir = await transformDir(path);
    const tmpPath = pathMod.join(tDir, `${hash}.ts`);
    const info = { tmpPath, tmpUrl: pathToFileURL(tmpPath).href, content: compiled, hash };
    // Cache the *raw* compiled output; the import-specifier rewrite below
    // mutates only the per-request copy in `tempOf`, so a later hit never
    // inherits stale dependency temp paths.
    rememberCompiled(hash, { ...info });
    tempOf.set(path, info);
  }

  for (const path of order) {
    const info = tempOf.get(path);
    const { deps } = modules.get(path);
    let content = info.content;
    for (const { spec, path: depPath } of deps) {
      const dep = tempOf.get(depPath);
      if (!dep) continue;
      const rel = pathMod
        .relative(pathMod.dirname(info.tmpPath), dep.tmpPath)
        .split(pathMod.sep)
        .join("/");
      const relSpec = rel.startsWith(".") ? rel : `./${rel}`;
      content = content.split(`"${spec}"`).join(`"${relSpec}"`);
      content = content.split(`'${spec}'`).join(`'${relSpec}'`);
    }
    info.content = content;
  }

  const entryInfo = tempOf.get(entryPath);
  for (const path of order) {
    const info = tempOf.get(path);
    if (info.fromCache) {
      // Already on disk from the previous request — no write needed.
      try {
        await fs.access(info.tmpPath);
        continue;
      } catch { /* temp file was cleaned up; rewrite below */ }
    }
    await fs.writeFile(info.tmpPath, info.content, "utf8");
  }
  if (prev?.tmpPath && prev.tmpPath !== entryInfo.tmpPath) {
    await fs.rm(prev.tmpPath, { force: true }).catch(() => {});
  }
  serverModuleCache.set(entryPath, {
    hash: entryInfo.hash,
    tmpPath: entryInfo.tmpPath,
    tmpUrl: entryInfo.tmpUrl,
    graph,
  });
  return { url: entryInfo.tmpUrl, isTemp: true };
}

/**
 * §8.3 / §9.1: parse a URL's query string into a `Record<string, string |
 * string[]>`. A repeated key (`?tag=a&tag=b`) becomes an array, matching
 * `URLSearchParams.getAll` semantics. The `LoaderSearchParams` type emitted
 * by `oxide-router` typegen declares exactly this shape.
 */
function parseSearchParams(url) {
  const out = Object.create(null);
  if (typeof url !== "string") return out;
  const q = url.indexOf("?");
  if (q < 0) return out;
  const hashAt = url.indexOf("#", q);
  const query = hashAt >= 0 ? url.slice(q + 1, hashAt) : url.slice(q + 1);
  if (!query) return out;
  for (const part of query.split("&")) {
    if (!part) continue;
    const eq = part.indexOf("=");
    const rawK = eq < 0 ? part : part.slice(0, eq);
    const rawV = eq < 0 ? "" : part.slice(eq + 1);
    const k = decodeURIComponentSafe(rawK.replace(/\+/g, " "));
    const v = decodeURIComponentSafe(rawV.replace(/\+/g, " "));
    if (!k) continue;
    if (k in out) {
      const prev = out[k];
      if (Array.isArray(prev)) prev.push(v);
      else out[k] = [prev, v];
    } else {
      out[k] = v;
    }
  }
  return out;
}

function decodeURIComponentSafe(s) {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

async function dispatchLoaderRun(id, input) {
  // §20: stash this op's traceparent for the fetch shim.
  currentTraceparent = traceparentFromPayload(input && input.payload);

  const { module_path, route, params, url, method, headers, locals: inputLocals, locale: payloadLocale } =
    input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-LOADER", "loader:run requires a `module_path` string");
    return;
  }
  try {
    // §9.3: run the compiler passes before the module is evaluated, so a
    // `'use cache'` function is wrapped and reaches the Rust tier.
    const compiled = await compileServerModule(module_path);
    const mod = await import(versioned(compiled.url));
    const load = mod.load;
    if (typeof load !== "function") {
      // §9.3: a `page.server.ts` that exports only `actions` (or only
      // `config`) is valid — the route has no loader, so its data is empty.
      // Erroring here made an actions-only module unusable: the form POST
      // could not re-render the page it belonged to. Only a module that
      // exports neither is a genuine mistake worth surfacing.
      const hasActions = mod.actions && typeof mod.actions === "object";
      const hasConfig = mod.config && typeof mod.config === "object";
      if (hasActions || hasConfig) {
        callOk(id, {
          ok: true,
          status: 200,
          location: null,
          data: {},
          session: null,
          setCookies: [],
          headers: [],
          deferred: {},
          deferredPending: [],
        });
        return;
      }
      callErr(id, "JSH-LOADER", `module at ${module_path} has no \`load\` export`);
      return;
    }
    // Build a minimal LoadEvent. `defineLoad`'s wrapper uses
    // `event.defer` and `event.__oxideDeferredRegistry` to assign deferred
    // ids; provide both so the wrapper's defer conversion works.
    const registry = new Map();
    let nextId = 1;
    const defer = (promise) => {
      const nid = nextId++;
      registry.set(nid, promise);
      return { __deferred: nid };
    };

    // §16.6: a real CookieStore over the request's `cookie` header, with
    // writes collected as `Set-Cookie` values. `get` parses the header once;
    // `set`/`delete` append to `setCookies`. A same-name overwrite replaces
    // the pending value so a load that sets then re-sets a cookie sends one
    // header, not two.
    const inboundCookieHeader = (Array.isArray(headers) ? headers : []).find(
      (h) => Array.isArray(h) && String(h[0]).toLowerCase() === "cookie",
    );
    const jar = new Map();
    if (inboundCookieHeader) {
      for (const part of String(inboundCookieHeader[1]).split(";")) {
        const eq = part.indexOf("=");
        if (eq > 0) {
          const k = part.slice(0, eq).trim();
          const v = part.slice(eq + 1).trim();
          if (k) jar.set(k, decodeURIComponentSafe(v));
        }
      }
    }
    const pendingCookies = new Map(); // name → Set-Cookie value
    // S-13: validate the user-controlled name/path/domain before interpolation.
    // A `;` in any of them would inject a cookie attribute; a bad name is an
    // app bug and surfaces as a 500 at the loader boundary.
    const COOKIE_NAME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    const serializeCookie = (name, value, opts = {}) => {
      if (!COOKIE_NAME_RE.test(name)) {
        throw new Error(`invalid cookie name: ${JSON.stringify(String(name).slice(0, 32))}`);
      }
      let s = `${name}=${encodeURIComponent(value)}`;
      if (opts.path != null) {
        if (/[;,\s]/.test(String(opts.path))) throw new Error("invalid cookie path");
        s += `; Path=${opts.path}`;
      }
      if (opts.domain != null) {
        if (/[;,\s]/.test(String(opts.domain)) || !/^[A-Za-z0-9.-]+$/.test(String(opts.domain))) {
          throw new Error("invalid cookie domain");
        }
        s += `; Domain=${opts.domain}`;
      }
      if (typeof opts.maxAge === "number") s += `; Max-Age=${opts.maxAge}`;
      if (opts.httpOnly) s += "; HttpOnly";
      if (opts.secure) s += "; Secure";
      if (opts.sameSite) {
        const ss = String(opts.sameSite);
        s += `; SameSite=${ss[0].toUpperCase()}${ss.slice(1).toLowerCase()}`;
      }
      return s;
    };
    const cookies = {
      get: (name) => jar.get(name),
      set: (name, value, opts) => {
        jar.set(name, value);
        pendingCookies.set(name, serializeCookie(name, value, opts));
      },
      delete: (name) => {
        jar.delete(name);
        pendingCookies.set(name, serializeCookie(name, "", { maxAge: 0, path: "/" }));
      },
    };

    // §9.2: headers the load adds via `setHeaders`. Collected in insertion
    // order and applied to the response by the pipeline.
    const extraHeaders = [];
    const setHeaders = (h) => {
      if (!h || typeof h !== "object") return;
      for (const [k, v] of Object.entries(h)) extraHeaders.push([String(k), String(v)]);
    };

    // §9.2: `parent()` awaits the parent layout's data. The CLI pre-resolves
    // the layout chain and passes it in as `parentData`; a load with no parent
    // resolves to an empty object.
    const parentData = (input.payload && input.payload.parent_data) || {};
    const parent = async () => parentData;

    const event = {
      route: route ?? "",
      params: params ?? {},
      // §8.3 / §9.1: parsed search params — the `LoaderSearchParams`
      // type declared in `routes.d.ts` (repeated key → `string[]`).
      searchParams: parseSearchParams(url ?? "/"),
      url: url ?? "/",
      request: {
        method: method ?? "GET",
        url: url ?? "/",
        headers: Array.isArray(headers) ? headers : [],
      },
      // §8.1 / §9.1: request-scoped locals injected by middleware. Absent
      // (empty object) when the project has no middleware; present when a
      // middleware returned `{ locals: {...} }`.
      locals: inputLocals && typeof inputLocals === "object" ? inputLocals : {},
      // D1: the decoded session, mutable. A loader that changes it has the
      // change persisted by the pipeline (which re-seals and sets the
      // cookie), so `event.session.user = ...` is the write API.
      session:
        inputLocals && typeof inputLocals === "object" && inputLocals.session
          ? inputLocals.session
          : {},
      // SPEC-10 / §16.4: the pipeline's resolved locale, forwarded by the
      // Rust host whitelist. Empty string when the project has no i18n —
      // mirrors the shim's `LoaderRunInput::locale` default.
      locale: typeof payloadLocale === "string" ? payloadLocale : "",
      cookies,
      setHeaders,
      parent,
      defer,
      __oxideDeferredRegistry: registry,
    };
    let result;
    try {
      result = await load(event);
    } catch (err) {
      // OxideHttpError: has `status` (number). OxideRedirect: has
      // `status` AND `location`. Both become {status, location?} responses.
      if (err && typeof err.status === "number") {
        callOk(id, {
          ok: true,
          status: err.status,
          location: err.location ?? null,
          data: { message: err.message ?? "" },
          // §16.6 / WP-r9-3: a loader that mutates \`event.session\` and then
          // throws a redirect/error must still have that mutation persisted.
          // Without this line, the four SSR short-circuit arms in the Rust
          // dispatch had \`out.session == None\` by construction — the
          // encode calls were dead code. Now they fire when the loader set
          // a session on the way out.
          session: event.session,
          setCookies: [...pendingCookies.values()],
          headers: extraHeaders,
        });
        return;
      }
      throw err;
    }
    // §7.1 / §9.2: the loader's `defer(promise)` markers point into a
    // registry populated by the wrapper. Await every promise and serialize
    // `{ id: value }` so the Rust side can inject resolution scripts and
    // the client's DeferredClient slots resolve to real data. A rejected
    // promise surfaces as a serialized error object, not a throw — a
    // loader whose data is partially ready should still render.
    // 9/10 (WP-B1): stream deferred settles instead of awaiting them all.
    //
    // The load result ships immediately with `deferredPending` naming the
    // ids; each promise, as it settles, emits a `call_chunk` carrying
    // `{deferredSettle: {id, value|error}}`; a final `call_end` closes the
    // stream. The Rust host collects the same way it does today (no
    // protocol change), so this is behaviour-preserving for a consumer that
    // awaits `collect()` - and the seam a streaming consumer (WP-B2) uses to
    // flush a resolution script per settle rather than after the slowest.
    const pendingIds = [];
    const settlePromises = [];
    for (const [did, promise] of registry) {
      pendingIds.push(String(did));
      settlePromises.push(
        promise.then(
          (v) => ({ deferredSettle: { id: String(did), value: v } }),
          (e) => ({ deferredSettle: { id: String(did), error: String(e?.message ?? e) } }),
        ),
      );
    }
    // §16-G: a `page.server.ts` may export
    // `meta = defineMeta(({ data }) => ({ title, description, ... }))`.
    // `defineMeta` tags the function; calling it with the load result gives
    // the neutral head model. Evaluated AFTER the load (its `data` argument
    // is what the load returned) and kept separate from `data` so a route's
    // loader result and its head contribution do not collide.
    let meta = null;
    if (typeof mod.meta === "function") {
      try {
        const m = await mod.meta({ data: result ?? {}, params: params ?? {} });
        if (m && typeof m === "object") meta = m;
      } catch (e) {
        // A throwing meta must not fail the route's render — the page body
        // and its data are what the user came for; the head is decoration.
        process.stderr.write(
          '[oxide] meta() threw for ' + module_path + ': ' + (e?.message ?? e) + '\n',
        );
      }
    }

    const initial = {
      ok: true,
      status: 200,
      location: null,
      deferred: {},
      deferredPending: pendingIds,
      data: result ?? {},
      // D1: the loader may have mutated `event.session`; the pipeline
      // compares it against the inbound value and re-seals when it changed.
      session: event.session,
      meta,
      setCookies: [...pendingCookies.values()],
      headers: extraHeaders,
    };
    if (settlePromises.length === 0) {
      // Nothing deferred: the plain complete response is byte-identical to
      // what this op sent before, so a route with no defer() is untouched.
      callOk(id, initial);
    } else {
      // Initial chunk, not complete.
      // NDJSON: each streamed payload is one JSON line. The host splits on
      // `\n` and folds the `{deferredSettle}` lines into the initial
      // payload's `deferred` map.
      out({
        t: "call_ok",
        id,
        chunks: [b64(Buffer.from(JSON.stringify(initial) + "\n", "utf8"))],
        complete: false,
      });
      for (const p of settlePromises) {
        const payload = await p;
        out({
          t: "call_chunk",
          id,
          data_b64: b64(Buffer.from(JSON.stringify(payload) + "\n", "utf8")),
        });
      }
      out({ t: "call_end", id });
    }
  } catch (e) {
    callErr(id, "JSH-LOADER", `loader:run failed: ${e?.message ?? e}`);
  }
}

// --- Action op (spec §9.3) ----------------------------------------------
//
//   action:run   { module_path, action, form_json, params, url, method, headers }
//              → { ok, status, location, data, setCookies, headers, revalidate }
//
// Imports the route's `page.server.ts`, calls `actions[action](ctx)`, and
// returns the result. `error()`/`redirect()` sentinels become status +
// location (same as a load). `revalidate` calls made through the action's
// `ctx.revalidate` are collected and returned so the pipeline can invalidate.
async function dispatchActionRun(id, input) {
  // §20: stash this op's traceparent for the fetch shim.
  currentTraceparent = traceparentFromPayload(input && input.payload);

  const { module_path, action, form_json, params, url, method, headers, locals: inputLocals } =
    input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-ACTION", "action:run requires a `module_path` string");
    return;
  }
  if (typeof action !== "string" || action.length === 0) {
    callErr(id, "JSH-ACTION", "action:run requires an `action` name");
    return;
  }
  try {
    const compiled = await compileServerModule(module_path);
    const mod = await import(versioned(compiled.url));
    const handlers = mod.actions;
    if (!handlers || typeof handlers !== "object") {
      callErr(id, "JSH-ACTION", `module at ${module_path} has no \`actions\` export`);
      return;
    }
    const def = handlers[action];
    if (!def) {
      // Distinct from "ran and threw": the pipeline maps this to a 404.
      callErr(id, "JSH-ACTION-NOT-FOUND", `no action named \`${action}\``);
      return;
    }
    // `defineAction` returns `{ $$oxide: 'action', input, run }`; a plain
    // async function is also accepted.
    const run = typeof def === "function" ? def : def.run;
    if (typeof run !== "function") {
      callErr(id, "JSH-ACTION", `action \`${action}\` has no run function`);
      return;
    }

    // Rebuild a FormData from the JSON the CLI sent (the pipeline parses the
    // request body before calling in).
    let form;
    try {
      const entries = form_json ? JSON.parse(form_json) : [];
      form = new FormData();
      for (const [k, v] of entries) form.append(k, v);
    } catch (e) {
      callErr(id, "JSH-ACTION", `form body is not valid: ${e?.message ?? e}`);
      return;
    }

    // `revalidate` collectors (§11.3): recorded in order, returned to Rust.
    const revalidateLog = [];
    const revalidate = {
      tag: (t) => revalidateLog.push({ type: "tag", tag: String(t) }),
      path: (p) => revalidateLog.push({ type: "path", path: String(p) }),
    };

    // Cookie store over the request's cookie header + collected Set-Cookie.
    const inbound = (Array.isArray(headers) ? headers : []).find(
      (h) => Array.isArray(h) && String(h[0]).toLowerCase() === "cookie",
    );
    const jar = new Map();
    if (inbound) {
      for (const part of String(inbound[1]).split(";")) {
        const eq = part.indexOf("=");
        if (eq > 0) {
          const k = part.slice(0, eq).trim();
          if (k) jar.set(k, decodeURIComponentSafe(part.slice(eq + 1).trim()));
        }
      }
    }
    const pendingCookies = new Map();
    const serializeCookie = (name, value, opts = {}) => {
      let s = `${name}=${encodeURIComponent(value)}`;
      if (opts.path) s += `; Path=${opts.path}`;
      if (opts.domain) s += `; Domain=${opts.domain}`;
      if (typeof opts.maxAge === "number") s += `; Max-Age=${opts.maxAge}`;
      if (opts.httpOnly) s += "; HttpOnly";
      if (opts.secure) s += "; Secure";
      if (opts.sameSite) {
        const ss = String(opts.sameSite);
        s += `; SameSite=${ss[0].toUpperCase()}${ss.slice(1).toLowerCase()}`;
      }
      return s;
    };
    const cookies = {
      get: (name) => jar.get(name),
      set: (name, value, opts) => {
        jar.set(name, value);
        pendingCookies.set(name, serializeCookie(name, value, opts));
      },
      delete: (name) => {
        jar.delete(name);
        pendingCookies.set(name, serializeCookie(name, "", { maxAge: 0, path: "/" }));
      },
    };
    const extraHeaders = [];
    const setHeaders = (h) => {
      if (!h || typeof h !== "object") return;
      for (const [k, v] of Object.entries(h)) extraHeaders.push([String(k), String(v)]);
    };

    // Input validation via a Standard Schema validator, when attached.
    let validated;
    if (def && def.input && typeof def.input["~standard"]?.validate === "function") {
      const raw = Object.fromEntries(form.entries());
      const res = await def.input["~standard"].validate(raw);
      if (res.issues && res.issues.length > 0) {
        callOk(id, {
          ok: true,
          status: 422,
          location: null,
          data: { message: res.issues.map((i) => i.message).join("; ") },
          setCookies: [...pendingCookies.values()],
          headers: extraHeaders,
          revalidate: revalidateLog,
        });
        return;
      }
      validated = res.value;
    } else {
      validated = Object.fromEntries(form.entries());
    }

    const event = {
      params: params ?? {},
      // §8.3 / §9.1: parsed search params (parity with loader:run).
      searchParams: parseSearchParams(url ?? "/"),
      request: {
        method: method ?? "POST",
        url: url ?? "/",
        headers: Array.isArray(headers) ? headers : [],
      },
      // §8.1 middleware.ts / §9.1: request-scoped locals injected by
      // middleware.
      locals: inputLocals && typeof inputLocals === "object" ? inputLocals : {},
      cookies,
      setHeaders,
      input: validated,
      form,
      revalidate,
    };

    let result;
    try {
      result = await run(event);
    } catch (err) {
      if (err && typeof err.status === "number") {
        callOk(id, {
          ok: true,
          status: err.status,
          location: err.location ?? null,
          data: { message: err.message ?? "" },
          setCookies: [...pendingCookies.values()],
          headers: extraHeaders,
          revalidate: revalidateLog,
        });
        return;
      }
      throw err;
    }
    callOk(id, {
      ok: true,
      status: 200,
      location: null,
      data: result ?? {},
      setCookies: [...pendingCookies.values()],
      headers: extraHeaders,
      revalidate: revalidateLog,
    });
  } catch (e) {
    callErr(id, "JSH-ACTION", `action:run failed: ${e?.message ?? e}`);
  }
}

// --- Endpoint op (spec §8.1 route.ts, §10.2 step 7) ---------------------
//
//   endpoint:run   { module_path, method, url, params, headers, body_b64 }
//                 → { status, headers: [[k,v],...], body_b64 }
//
// Loads the route's `route.ts`, dispatches by HTTP method. Two shapes are
// supported:
//   * Named per-method exports (`export function GET(request) {}`,
//     `export const POST = ...`) — the Next.js-style convention.
//   * A default export `(request) => Response`.
// The return value must be a `Response` (Web standard). Any non-Response is
// a diagnostic 500 — a route.ts that forgets to return is a bug we surface
// loudly rather than converting to a silent 204.
async function dispatchEndpointRun(id, input) {
  // §20: stash this op's traceparent for the fetch shim.
  currentTraceparent = traceparentFromPayload(input && input.payload);

  const { module_path, method, url, params, headers, body_b64, locals: inputLocals } = input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-ENDPOINT", "endpoint:run requires a `module_path` string");
    return;
  }
  const methodUpper = String(method ?? "GET").toUpperCase();
  try {
    const compiled = await compileServerModule(module_path);
    const mod = await import(versioned(compiled.url));

    // Method handler resolution: named GET/POST/... first, then default.
    const handler = mod[methodUpper] ?? mod.default;
    if (typeof handler !== "function") {
      callErr(
        id,
        "JSH-ENDPOINT",
        `module at ${module_path} has no \`${methodUpper}\` export and no default export`,
      );
      return;
    }

    const headerList = Array.isArray(headers) ? headers : [];
    const bodyBytes = body_b64 ? Buffer.from(body_b64, "base64") : new Uint8Array();
    const canHaveBody = methodUpper !== "GET" && methodUpper !== "HEAD";
    const reqInit = {
      method: methodUpper,
      headers: headerList,
      ...(canHaveBody && bodyBytes.length > 0 ? { body: bodyBytes } : {}),
    };
    const absoluteUrl = url && url.startsWith("http")
      ? url
      : `http://placeholder${url || "/"}`;

    const ctx = {
      request: new Request(absoluteUrl, reqInit),
      params: params ?? {},
      // §8.1 middleware.ts / §9.1: request-scoped locals injected by middleware.
      locals: inputLocals && typeof inputLocals === "object" ? inputLocals : {},
    };

    let response;
    try {
      // Next-style: `(request, { params })`.
      response = await handler(ctx.request, { params: ctx.params, locals: ctx.locals });
    } catch (err) {
      if (err && typeof err.status === "number") {
        callOk(id, {
          status: err.status,
          headers: err.location ? [["location", String(err.location)]] : [],
          body_b64: "",
        });
        return;
      }
      throw err;
    }

    if (!(response instanceof Response)) {
      callErr(
        id,
        "JSH-ENDPOINT",
        `route.ts handler for ${methodUpper} returned ${typeof response}, not a Response`,
      );
      return;
    }

    const respHeaders = [];
    response.headers.forEach((v, k) => respHeaders.push([k, v]));

    // §9.6: a `route.ts` that returns a streaming body (an SSE response, a
    // download) must not be buffered — `arrayBuffer()` would wait for the
    // whole stream and defeat server-sent events. When the body is a
    // `ReadableStream`, emit a header frame then each chunk as a
    // `call_chunk_bin` frame; the Rust side assembles an `OxideResponse`
    // whose body is that stream.
    if (response.body && typeof response.body.getReader === "function") {
      out({
        t: "call_ok",
        id,
        chunks: [b64(Buffer.from(JSON.stringify({ status: response.status, headers: respHeaders, streaming: true }), "utf8"))],
        complete: false,
      });
      const reader = response.body.getReader();
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value && value.length > 0) outBinChunk(id, Buffer.from(value));
        }
      } catch (e) {
        // A mid-stream throw: end the stream cleanly; the client sees the
        // partial body (an SSE `retry` on the next reconnect).
      }
      out({ t: "call_end", id });
      return;
    }

    const respBody = new Uint8Array(await response.arrayBuffer());
    const bodyOut = respBody.length > 0 ? Buffer.from(respBody).toString("base64") : "";
    callOk(id, {
      status: response.status,
      headers: respHeaders,
      body_b64: bodyOut,
    });
  } catch (e) {
    callErr(id, "JSH-ENDPOINT", `endpoint:run failed: ${e?.message ?? e}`);
  }
}

// --- Middleware op (spec §8.1 middleware.ts, §8.5 rewrites) -------------
//
//   middleware:run   { module_path, method, url, params, headers, body_b64,
//                      locals }
//                  → { short: bool, status, headers, body_b64, redirect?, rewrite? }
//
// Loads the middleware module at `module_path`, calls the user's
// `middleware(request, ctx)` (or the default export), and produces a
// directive the pipeline applies:
//
//   undefined / null          → `{ short: false }` (pass through)
//   { headers: [[k,v]] }      → `{ short: false, headers }` (add response headers)
//   { locals: {...} }         → `{ short: false, locals }` (propagate to loader)
//   { rewrite: '/path' }      → `{ short: false, rewrite }` (re-route request)
//   { redirect: '/login', status? } → `{ short: true, status, redirect }`
//   Response (Web)            → `{ short: true, status, headers, body_b64 }`
//
// Returning anything else is a diagnostic (`JSH-MIDDLEWARE`) — a middleware
// that forgot to return is a bug we surface rather than silently letting a
// request proceed.
async function dispatchMiddlewareRun(id, input) {
  // §20: stash this op's traceparent for the fetch shim.
  currentTraceparent = traceparentFromPayload(input && input.payload);

  const { module_path, method, url, params, headers, body_b64, locals } = input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-MIDDLEWARE", "middleware:run requires a `module_path` string");
    return;
  }
  const methodUpper = String(method ?? "GET").toUpperCase();
  try {
    const compiled = await compileServerModule(module_path);
    const mod = await import(versioned(compiled.url));
    const handler = mod.middleware ?? mod.default;
    if (typeof handler !== "function") {
      callErr(
        id,
        "JSH-MIDDLEWARE",
        `module at ${module_path} has no \`middleware\` export and no default export`,
      );
      return;
    }

    const headerList = Array.isArray(headers) ? headers : [];
    const bodyBytes = body_b64 ? Buffer.from(body_b64, "base64") : new Uint8Array();
    const canHaveBody = methodUpper !== "GET" && methodUpper !== "HEAD";
    const reqInit = {
      method: methodUpper,
      headers: headerList,
      ...(canHaveBody && bodyBytes.length > 0 ? { body: bodyBytes } : {}),
    };
    const absoluteUrl = url && url.startsWith("http")
      ? url
      : `http://placeholder${url || "/"}`;
    const request = new Request(absoluteUrl, reqInit);
    const ctx = {
      params: params ?? {},
      locals: locals ?? {},
    };

    let result;
    try {
      result = await handler(request, ctx);
    } catch (err) {
      if (err && typeof err.status === "number") {
        callOk(id, {
          short: true,
          status: err.status,
          headers: err.location ? [["location", String(err.location)]] : [],
          body_b64: "",
          redirect: err.location ?? null,
        });
        return;
      }
      throw err;
    }

    if (result === undefined || result === null) {
      callOk(id, { short: false, status: 200, headers: [], body_b64: "" });
      return;
    }

    // A Web Response short-circuits entirely.
    if (result instanceof Response) {
      const respHeaders = [];
      result.headers.forEach((v, k) => respHeaders.push([k, v]));
      const respBody = new Uint8Array(await result.arrayBuffer());
      const bodyOut = respBody.length > 0 ? Buffer.from(respBody).toString("base64") : "";
      callOk(id, {
        short: true,
        status: result.status,
        headers: respHeaders,
        body_b64: bodyOut,
      });
      return;
    }

    // A plain object is a directive.
    if (typeof result === "object") {
      if (typeof result.redirect === "string") {
        callOk(id, {
          short: true,
          status: typeof result.status === "number" ? result.status : 307,
          headers: [["location", result.redirect]],
          body_b64: "",
          redirect: result.redirect,
        });
        return;
      }
      callOk(id, {
        short: false,
        status: 200,
        headers: Array.isArray(result.headers) ? result.headers : [],
        body_b64: "",
        rewrite: typeof result.rewrite === "string" ? result.rewrite : null,
        locals: result.locals ?? null,
      });
      return;
    }

    callErr(
      id,
      "JSH-MIDDLEWARE",
      `middleware returned ${typeof result}; expected undefined, an object directive, or a Response`,
    );
  } catch (e) {
    callErr(id, "JSH-MIDDLEWARE", `middleware:run failed: ${e?.message ?? e}`);
  }
}

// --- Socket op (spec §9.6) -----------------------------------------------
//
//   socket:run   { module_path, phase, payload, socket_id }
//              → { send?: string, close?: {code,reason} }
//
// Loads the route's `socket.ts` (`export const socket = defineSocket({...})`
// or a default `{ open, message, close }` object), calls the named phase,
// and returns whatever the handler `send()`s plus an optional close. The
// Rust side owns the upgrade, framing and ping/pong; this op is one phase.
async function dispatchSocketRun(id, input) {
  // §20: stash this op's traceparent for the fetch shim.
  currentTraceparent = traceparentFromPayload(input && input.payload);

  const { module_path, phase, payload, socket_id } = input.payload ?? {}
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-SOCKET", "socket:run requires a `module_path` string")
    return
  }
  const phaseName = phase === "open" || phase === "message" || phase === "close" ? phase : "message"
  try {
    const compiled = await compileServerModule(module_path)
    const mod = await import(versioned(compiled.url))
    const handlers = mod.socket ?? mod.default
    if (!handlers || typeof handlers !== "object") {
      callErr(id, "JSH-SOCKET", `module at ${module_path} exports no socket handlers`)
      return
    }
    const fn = handlers[phaseName]
    if (typeof fn !== "function") {
      // No handler for this phase is not an error — a socket may only
      // implement `message`.
      callOk(id, { send: null, close: null })
      return
    }
    // Per-socket state: a Map on the worker so `open` can stash a user id
    // that `message` reads. Keyed by the Rust-side socket_id.
    if (!globalThis.__oxide_socket_state) globalThis.__oxide_socket_state = new Map()
    const state = globalThis.__oxide_socket_state
    let record = state.get(socket_id)
    if (!record) { record = { locals: {} }; state.set(socket_id, record) }

    const sent = []
    const broadcasts = []
    const targets = []
    const encode = (data) =>
      typeof data === "string" ? data : Buffer.from(data).toString("base64")
    const send = (data) => { sent.push(encode(data)) }
    const broadcast = (data) => { broadcasts.push(encode(data)) }
    const sendTo = (id, data) => { targets.push([String(id), encode(data)]) }
    const clients = Array.isArray(payload?.clients) ? payload.clients.map(String) : []
    let close = null
    const ctx = {
        id: socket_id,
      data: phaseName === "message" ? payload?.data : undefined,
      code: phaseName === "close" ? (payload?.code ?? 1000) : undefined,
      reason: phaseName === "close" ? (payload?.reason ?? "") : undefined,
      headers: payload?.headers ?? {},
      locals: record.locals,
      clients,
      send,
      broadcast,
      sendTo,
    }
    try {
      await fn(ctx)
    } catch (e) {
      if (e && typeof e.code === "number") {
        close = { code: e.code, reason: String(e.reason ?? e.message ?? "") }
      } else {
        throw e
      }
    }
    if (phaseName === "close") state.delete(socket_id)
    callOk(id, {
      send: sent.length === 0 ? null : (sent.length === 1 ? sent[0] : sent),
      broadcasts,
      targets,
      close,
    })
  } catch (e) {
    callErr(id, "JSH-SOCKET", `socket:run failed: ${e?.message ?? e}`)
  }
}

// --- HMR op (spec §13.3) --------------------------------------------------
//
//   framework:invalidate   {}   → { generation: N }
//
// Bumps the module-generation counter. Every subsequent `import()` inside
// this worker uses `?v=<N>`, so Node re-evaluates cached modules on next
// access. Idempotent; cheap. Called by the dev server's file watcher.
async function dispatchFrameworkInvalidate(id, _input) {
  moduleGeneration += 1;
  callOk(id, { generation: moduleGeneration });
}

// --- Generate-params ops (spec §10.5) -----------------------------------
//
//   generate-params:run   { module_path }
//                       → { params: [ {...}, ... ] }
//
// Imports a `page.server.ts` module, invokes its `generateParams` export
// (either a function or a value), and returns the array of param objects
// as strings. Used by `oxide build` to enumerate static paths for a route
// whose generateParams cannot be evaluated statically (e.g. it reads
// `process.env` or builds an array programmatically).
// ---------------------------------------------------------------------------
// §9.4 server functions.
//
// A `serverFn(...)` export in a `.server.ts` module is registered at
// import time under a build-computed id (`__oxide_register_fn("<id>", fn)`,
// which the runtime installs on `globalThis`). A client call POSTs to
// `/_oxide/fn/<id>`; the CLI's dispatcher forwards (id, args) here, and
// this op imports the modules that own the registry, looks the id up, runs
// it, and returns the encoded result.
//
// Registry ownership: the ids are compiled into the SSR chunks. Rather
// than track which module registered what, the op imports the SSR adapter
// entry once (`ssr/index.js`), which imports every route module, which
// imports its `.server.ts` sidecar — so every `serverFn` in the project
// has run its registration by the time the entry finishes importing. The
// import is cached; subsequent calls reuse it.
//
// This mirrors how the render path is set up: the whole graph is one
// import graph rooted at `ssr/index.js`.

// import_once is a Map<abs-path, Promise<module>>; the versioned() tag
// bumps on HMR invalidation, matching the rest of the worker.
async function loadServerFnGraph(entryPath) {
  const key = `fn-graph:${entryPath}:gen${moduleGeneration}`;
  if (!serverFnGraphCache.has(key)) {
    const p = (async () => {
      await import(versioned(pathToFileURL(entryPath).href));
      return true;
    })();
    serverFnGraphCache.set(key, p);
  }
  return serverFnGraphCache.get(key);
}

async function dispatchServerFn(id, input) {
  const { fn_id, args_json, entry_path } = input.payload ?? {};
  if (typeof fn_id !== "string" || fn_id.length === 0) {
    callErr(id, "JSH-FN", "server-fn requires a `fn_id` string");
    return;
  }
  if (typeof entry_path !== "string" || entry_path.length === 0) {
    callErr(id, "JSH-FN", "server-fn requires an `entry_path` string (the SSR adapter entry)");
    return;
  }

  try {
    await loadServerFnGraph(entry_path);
  } catch (e) {
    callErr(id, "JSH-FN", `failed to import the SSR entry: ${e?.message ?? e}`);
    return;
  }

  const registry = globalThis.__oxide_fn_registry;
  if (!registry || typeof registry !== "object") {
    callErr(
      id,
      "JSH-FN",
      "no server-fn registry: the SSR entry did not import `oxide/server`",
    );
    return;
  }
  const fn = registry[fn_id];
  if (typeof fn !== "function") {
    // Not an error at the op level — the HTTP layer turns this into a
    // 404 envelope. Signal "not found" distinctly from "ran and threw".
    callErr(id, "JSH-FN-NOT-FOUND", `no server function registered under id ${fn_id}`);
    return;
  }

  // Arguments arrive as a JSON array. The wire format is JSON for now;
  // ODF is a drop-in replacement for both directions once the browser
  // decoder ships.
  let args;
  try {
    args = args_json ? JSON.parse(args_json) : [];
  } catch (e) {
    callErr(id, "JSH-FN", `arguments are not valid JSON: ${e?.message ?? e}`);
    return;
  }
  if (!Array.isArray(args)) {
    callErr(id, "JSH-FN", "arguments must be a JSON array");
    return;
  }

  try {
    const value = await fn(...args);
    callOk(id, { value });
  } catch (e) {
    // A function that throws is not a transport failure — report the
    // message so the HTTP layer can build an error envelope.
    callErr(id, "JSH-FN-THREW", e?.message ?? String(e));
  }
}

async function dispatchGenerateParamsRun(id, input) {
  const { module_path } = input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-PARAMS", "generate-params:run requires a `module_path` string");
    return;
  }
  try {
    const mod = await import(versioned(pathToFileURL(module_path).href));
    const gp = mod.generateParams;
    if (gp === undefined) {
      // Absent export is not an error — the caller falls back to the
      // "no generateParams" path.
      callOk(id, { params: [] });
      return;
    }
    let value;
    if (typeof gp === "function") {
      value = await gp();
    } else {
      value = gp;
    }
    if (!Array.isArray(value)) {
      callErr(
        id,
        "JSH-PARAMS",
        `generateParams must return an array; got ${value === null ? "null" : typeof value}`,
      );
      return;
    }
    // Normalize each entry to a flat string map. Non-string scalars are
    // coerced via String(); nested objects/arrays are skipped (params must
    // be scalar — they get substituted into URL segments).
    const params = value.map((item) => {
      if (item === null || typeof item !== "object" || Array.isArray(item)) {
        return {};
      }
      const out = {};
      for (const [k, v] of Object.entries(item)) {
        if (v === null || v === undefined) continue;
        if (typeof v === "object") continue;
        out[k] = typeof v === "string" ? v : String(v);
      }
      return out;
    });
    callOk(id, { params });
  } catch (e) {
    callErr(id, "JSH-PARAMS", `generate-params:run failed: ${e?.message ?? e}`);
  }
}

const handlers = {
  render: (msg) => {
    const title = msg.input.payload?.title ?? `${msg.entry.module}:${msg.entry.export}`;
    return { html: `<html>${title}</html>` };
  },
  "tooling:compile": (msg) => ({ code: String(msg.input.payload?.code ?? "").toUpperCase() }),
  sleep: (msg) => ({ slept: true, ms: Number(msg.input.payload?.ms ?? 100) }),
};

async function handleCall(msg) {
  // §20: `currentTraceparent` is per-op state. Clear it in `finally` so a
  // later op on the same worker never inherits a stale trace id.
  try {
    return await handleCallInner(msg);
  } finally {
    currentTraceparent = null;
  }
}

async function handleCallInner(msg) {
  const { id, entry, input } = msg;
  if (entry.kind === "tooling:module") { await dispatchToolingModule(id, input); return; }
  if (entry.kind === "tooling:vue-compile") { await dispatchVueCompile(id, input); return; }
  if (entry.kind === "tooling:svelte-compile") { await dispatchSvelteCompile(id, input); return; }
  if (entry.kind === "tooling:solid-compile") { await dispatchSolidCompile(id, input); return; }
  if (entry.kind === "plugin:load") { await dispatchPluginLoad(id, input); return; }
  if (entry.kind === "plugin:call") { await dispatchPluginCall(id, input); return; }
  if (entry.kind === "framework:load") { await dispatchFrameworkLoad(id, input); return; }
  if (entry.kind === "framework:render") { await dispatchFrameworkRender(id, input); return; }
  if (entry.kind === "loader:run") { await dispatchLoaderRun(id, input); return; }
  if (entry.kind === "framework:invalidate") { await dispatchFrameworkInvalidate(id, input); return; }
  if (entry.kind === "components:load") { await dispatchComponentsLoad(id, input); return; }
  if (entry.kind === "generate-params:run") { await dispatchGenerateParamsRun(id, input); return; }
  if (entry.kind === "server-fn:run") { await dispatchServerFn(id, input); return; }
  if (entry.kind === "action:run") { await dispatchActionRun(id, input); return; }
  if (entry.kind === "endpoint:run") { await dispatchEndpointRun(id, input); return; }
  if (entry.kind === "socket:run") { await dispatchSocketRun(id, input); return; }
  if (entry.kind === "middleware:run") { await dispatchMiddlewareRun(id, input); return; }
  if (entry.kind === "stream-die") {
    // P0-2(b): emit one chunk so the host builds a `CallStream::Chunked`,
    // give the host a tick to observe it, then die mid-stream. The host
    // must turn the mid-stream EOF into JSH-CRASH, not a truncated
    // success.
    out({ t: "call_chunk", id, data_b64: b64(Buffer.from("partial", "utf8")) });
    await new Promise((r) => setTimeout(r, 20));
    process.exit(1);
  }
  if (entry.kind === "die") process.exit(1);
  if (entry.kind === "sleep") {
    await new Promise((r) => setTimeout(r, Number(input.payload?.ms ?? 100)));
  }
  if (entry.kind === "boom") {
    callErr(id, "JSH-BOOM", `worker simulated failure for kind \`${entry.kind}\``);
    return;
  }
  const fn = handlers[entry.kind];
  if (!fn) {
    callErr(id, "JSH-NOKIND", `worker has no handler for kind \`${entry.kind}\``);
    return;
  }
  if (input.payload?.chunked === true) {
    for (const part of ["<html>", entry.kind, "</html>"]) {
      out({ t: "call_chunk", id, data_b64: b64(Buffer.from(part, "utf8")) });
    }
    out({ t: "call_end", id });
    return;
  }
  out({ t: "call_ok", id, chunks: [b64(Buffer.from(JSON.stringify(fn(msg)), "utf8"))], complete: true });
}

out({ t: "ready", pid: process.pid });

let buf = Buffer.alloc(0);
process.stdin.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    if (buf.length < 4) return;
    const len = buf.readUInt32LE(0);
    // PERF-12: frame = [4-byte len][1-byte kind][payload]; len covers
    // kind+payload. Host→worker frames are always kind 0 (JSON).
    if (buf.length < 5 || len === 0) return;
    if (buf.length < 4 + len) return;
    const kind = buf.readUInt8(4);
    if (kind !== 0) {
      // The host never sends a binary frame to the worker; drop and resync.
      buf = buf.subarray(4 + len);
      return;
    }
    const msg = JSON.parse(buf.subarray(5, 4 + len).toString("utf8"));
    buf = buf.subarray(4 + len);
    if (msg.t === "op_response") {
      // §4.5: a reply to an op the worker sent. Resolve the pending
      // promise; a late reply (its id already removed) is dropped.
      const p = pendingOps.get(msg.id);
      if (p) {
        pendingOps.delete(msg.id);
        if (msg.ok) {
          p.resolve(msg.data);
        } else {
          const e = new Error(msg.data?.message ?? "host op failed");
          e.code = msg.data?.code ?? "JSH-OP";
          p.reject(e);
        }
      }
      continue;
    }
    if (msg.t !== "call") continue;
    Promise.resolve(handleCall(msg)).catch((e) =>
      callErr(msg.id, "JSH-WORKER-ERR", String(e?.message ?? e)),
    );
  }
});
process.stdin.on("end", () => process.exit(0));
process.stdout.on("error", () => process.exit(0));
process.stdin.resume();
