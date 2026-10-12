// oxide-worker.mjs — Oxide Node worker (spec §4.2, §4.6 tier 3).
// Speaks the same op protocol as the Rust host over stdio: each message is a
// 4-byte little-endian length prefix + UTF-8 JSON (WorkerMsg, tag "t").
// Kinds handled: "render" → {html}; "tooling:compile" → {code: uppercase};
// "boom" → CallErr; "die" → exit(1) without replying (crash drill);
// "sleep" → reply after payload.ms. payload.chunked === true streams
// CallChunk* + CallEnd instead of a single CallOk. Pure Node, no deps.
import process from "node:process";

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
process.stdin.on("end", () => process.exit(0)); // host dropped the pool: exit cleanly
process.stdout.on("error", () => process.exit(0)); // parent gone: never crash on EPIPE
process.stdin.resume();
