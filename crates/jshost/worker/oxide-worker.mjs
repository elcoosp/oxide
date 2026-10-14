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
function versioned(url) {
  const sep = url.includes("?") ? "&" : "?";
  return `${url}${sep}v=${moduleGeneration}`;
}
const out = (obj) => {
  const body = Buffer.from(JSON.stringify(obj), "utf8");
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length, 0);
  process.stdout.write(Buffer.concat([head, body]), (e) => e && process.exit(0));
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
// Two ops:
//   framework:load   { path }            → { name, extensions, capabilities }
//   framework:render { path, input }     → { html }
//
// The config module is dynamic-imported fresh on every call so dev edits
// take effect immediately (HMR-lite). Caching by mtime is a follow-up.

async function dispatchFrameworkLoad(id, input) {
  const { path } = input.payload ?? {};
  if (typeof path !== "string" || path.length === 0) {
    callErr(id, "JSH-FRAMEWORK", "framework:load requires a `path` string");
    return;
  }
  try {
    const mod = await import(versioned(pathToFileURL(path).href));
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

async function dispatchFrameworkRender(id, input) {
  const { path, input: renderInput } = input.payload ?? {};
  if (typeof path !== "string" || path.length === 0) {
    callErr(id, "JSH-FRAMEWORK", "framework:render requires a `path` string");
    return;
  }
  try {
    const mod = await import(versioned(pathToFileURL(path).href));
    const cfg = mod.default ?? mod;
    const framework = cfg?.framework;
    if (!framework || typeof framework.render !== "function") {
      callErr(id, "JSH-FRAMEWORK", `config at ${path} has no framework.render`);
      return;
    }
    const stream = framework.render(renderInput ?? {});
    // Emit the adapter's ReadableStream<Uint8Array> as chunked frames of
    // RAW HTML bytes — no JSON envelope. Rust consumers (render for the
    // collected shape, render_streaming for the incremental shape) both
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
      out({ t: "call_chunk", id, data_b64: b64(buf) });
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
  const pathMod = await import("node:path");

  if (typeof globalThis.__oxide_op_request !== "function") {
    return { url: pathToFileURL(entryPath).href, isTemp: false };
  }

  const modules = new Map();
  const order = [];
  const queue = [entryPath];
  while (queue.length > 0) {
    const path = queue.shift();
    if (modules.has(path)) continue;
    const source = await fs.readFile(path, "utf8");
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
    const hash = crypto
      .createHash("sha256")
      .update(source)
      .update("\u0000")
      .update(path)
      .digest("hex")
      .slice(0, 16);
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
    const dir = pathMod.dirname(path);
    const base = pathMod.basename(path);
    // `.ts` (not `.mjs`): `transform_passes_only` runs the AST passes but
    // not the TypeScript strip, so the emitted module still carries type
    // annotations. Node's native type-stripper handles a `.ts` file on
    // import; a `.mjs` file would be parsed as-is and fail.
    const tmpPath = pathMod.join(dir, `.${base}.${hash}.oxide.ts`);
    tempOf.set(path, { tmpPath, tmpUrl: pathToFileURL(tmpPath).href, content: compiled, hash });
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
  const cached = serverModuleCache.get(entryPath);
  for (const path of order) {
    await fs.writeFile(tempOf.get(path).tmpPath, tempOf.get(path).content, "utf8");
  }
  if (cached?.tmpPath && cached.tmpPath !== entryInfo.tmpPath) {
    await fs.rm(cached.tmpPath, { force: true }).catch(() => {});
  }
  serverModuleCache.set(entryPath, {
    hash: entryInfo.hash,
    tmpPath: entryInfo.tmpPath,
    tmpUrl: entryInfo.tmpUrl,
  });
  return { url: entryInfo.tmpUrl, isTemp: true };
}

function decodeURIComponentSafe(s) {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

async function dispatchLoaderRun(id, input) {
  const { module_path, route, params, url, method, headers } = input.payload ?? {};
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
      url: url ?? "/",
      request: {
        method: method ?? "GET",
        url: url ?? "/",
        headers: Array.isArray(headers) ? headers : [],
      },
      locals: {},
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
          setCookies: [...pendingCookies.values()],
          headers: extraHeaders,
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
    });
  } catch (e) {
    callErr(id, "JSH-LOADER", `loader:run failed: ${e?.message ?? e}`);
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
  const { id, entry, input } = msg;
  if (entry.kind === "tooling:module") { await dispatchToolingModule(id, input); return; }
  if (entry.kind === "tooling:vue-compile") { await dispatchVueCompile(id, input); return; }
  if (entry.kind === "tooling:svelte-compile") { await dispatchSvelteCompile(id, input); return; }
  if (entry.kind === "tooling:solid-compile") { await dispatchSolidCompile(id, input); return; }
  if (entry.kind === "framework:load") { await dispatchFrameworkLoad(id, input); return; }
  if (entry.kind === "framework:render") { await dispatchFrameworkRender(id, input); return; }
  if (entry.kind === "loader:run") { await dispatchLoaderRun(id, input); return; }
  if (entry.kind === "framework:invalidate") { await dispatchFrameworkInvalidate(id, input); return; }
  if (entry.kind === "generate-params:run") { await dispatchGenerateParamsRun(id, input); return; }
  if (entry.kind === "server-fn:run") { await dispatchServerFn(id, input); return; }
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
    if (buf.length < 4 + len) return;
    const msg = JSON.parse(buf.subarray(4, 4 + len).toString("utf8"));
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
