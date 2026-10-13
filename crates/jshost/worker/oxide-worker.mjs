// oxide-worker.mjs — Oxide Node worker (spec §4.2, §4.6 tier 3, §4.7, §7).
// Speaks the length-prefixed JSON op protocol. Handles:
//   render                   → {html}
//   tooling:compile          → uppercase echo (test-only)
//   tooling:module           → require(call.module)[call.export](...call.args)
//   framework:load           → dynamic import of a config path → descriptor
//   framework:render         → dynamic import → adapter.render(input) → JSON chunked
//   loader:run               → dynamic import → load(event) → {ok, status, location, data}
//   framework:invalidate     → bump moduleGeneration → {generation: N}
//   boom                     → CallErr
//   die                      → exit(1)
//   sleep                    → delayed reply
//   payload.chunked          → CallChunk* + CallEnd
// Pure Node ESM; no external deps.
import process from "node:process";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
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

// Cache required modules across calls (CJS path).
const modCache = new Map();
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
    // Collect the adapter's ReadableStream<Uint8Array> into one JSON blob
    // `{"html":"..."}` — but emit it as CHUNKED frames so the wire format is
    // future-proof for streaming-capable adapters (React's
    // renderToPipeableStream, Solid's resources). When the adapter streams,
    // the first chunk arrives before the last frame is produced, so a future
    // pipeline step can flush early.
    //
    // Chunk size: 8 KB. Small enough for reasonable TTFB once we wire a
    // streaming adapter, large enough that per-frame base64 + JSON framing
    // overhead (~33%) stays negligible.
    const chunks = [];
    for await (const chunk of stream) {
      chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
    }
    const html = Buffer.concat(chunks).toString("utf8");
    const jsonBytes = Buffer.from(JSON.stringify({ html }), "utf8");
    const CHUNK = 8192;
    for (let i = 0; i < jsonBytes.length; i += CHUNK) {
      const slice = jsonBytes.subarray(i, Math.min(i + CHUNK, jsonBytes.length));
      out({ t: "call_chunk", id, data_b64: b64(slice) });
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
async function dispatchLoaderRun(id, input) {
  const { module_path, route, params, url, method, headers } = input.payload ?? {};
  if (typeof module_path !== "string" || module_path.length === 0) {
    callErr(id, "JSH-LOADER", "loader:run requires a `module_path` string");
    return;
  }
  try {
    const mod = await import(versioned(pathToFileURL(module_path).href));
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
        });
        return;
      }
      throw err;
    }
    callOk(id, { ok: true, status: 200, location: null, data: result ?? {} });
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
  if (entry.kind === "framework:load") { await dispatchFrameworkLoad(id, input); return; }
  if (entry.kind === "framework:render") { await dispatchFrameworkRender(id, input); return; }
  if (entry.kind === "loader:run") { await dispatchLoaderRun(id, input); return; }
  if (entry.kind === "framework:invalidate") { await dispatchFrameworkInvalidate(id, input); return; }
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
    if (msg.t !== "call") continue;
    Promise.resolve(handleCall(msg)).catch((e) =>
      callErr(msg.id, "JSH-WORKER-ERR", String(e?.message ?? e)),
    );
  }
});
process.stdin.on("end", () => process.exit(0));
process.stdout.on("error", () => process.exit(0));
process.stdin.resume();
