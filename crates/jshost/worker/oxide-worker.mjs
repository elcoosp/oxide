// oxide-worker.mjs — Oxide Node worker (spec §4.2, §4.6 tier 3, §4.7).
// Speaks the length-prefixed JSON op protocol. Handles:
//   render              → {html}
//   tooling:compile     → uppercase echo (test-only)
//   tooling:module      → require(call.module)[call.export](...call.args)
//   boom                → CallErr
//   die                 → exit(1)
//   sleep               → delayed reply
//   payload.chunked     → CallChunk* + CallEnd
// Pure Node ESM; no external deps.
import process from "node:process";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const b64 = (buf) => buf.toString("base64");
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

// Cache required modules across calls.
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
    out({ t: "call_ok", id, chunks: [b64(Buffer.from(JSON.stringify(json), "utf8"))], complete: true });
  } catch (e) {
    callErr(id, "JSH-TOOLING", `call ${name}.${fn} threw: ${e?.message ?? e}`);
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
  if (entry.kind === "tooling:module") {
    await dispatchToolingModule(id, input);
    return;
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
