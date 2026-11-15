// V2-F15 worker pins: the `globalThis.__oxide_replay` install and the
// record-capture drain, exercised through the real framed protocol.
//
// Each pin spawns `oxide-worker.mjs` as a child process (the same way the
// jshost pool does) and speaks the length-prefixed JSON frames on stdio.
// The test doubles as a mini HOST: it answers the worker's `op_request`
// frames (fetch / cache_get) so a recorded-mode fetch completes without
// any network.
//
//   OXIDE_REPLAY=<file>  → the worker plays the recorded fetches and
//                          non-deterministic sequences back IN ORDER; an
//                          unexpected call is an error naming RPL-002.
//   OXIDE_RECORD=1       → the worker captures completed fetches, cache
//                          ops and the Date.now / Math.random /
//                          crypto.randomUUID observations; `replay:drain`
//                          returns and resets them.
//
// Run: node --test crates/jshost/worker/replay.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const WORKER = path.join(path.dirname(fileURLToPath(import.meta.url)), "oxide-worker.mjs");

// --- the framed protocol (the host half) ------------------------------------

function frame(obj) {
  const body = Buffer.from(JSON.stringify(obj), "utf8");
  const head = Buffer.alloc(5);
  head.writeUInt32LE(body.length + 1, 0);
  head.writeUInt8(0, 0 + 4); // kind 0 = JSON
  return Buffer.concat([head, body]);
}

class WorkerHost {
  constructor(env) {
    this.child = spawn(process.execPath, [WORKER], {
      env: { ...process.env, ...env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.buf = Buffer.alloc(0);
    this.waiters = [];
    this.opReplier = null; // (op, args) => data | null  — the mini host
    this.exit = new Promise((res) => this.child.on("exit", res));
    const stderr = [];
    this.child.stderr.on("data", (c) => stderr.push(String(c)));
    this.stderrText = () => stderr.join("");
    this.child.stdout.on("data", (chunk) => this.#onData(chunk));
    // The real host always answers `CompileModule` (the B-13 compile
    // passes); for a plain JS module the passthrough IS the compiled form.
    // The wire uses the serde `Op` variant names.
    this.baseReplier = (op, args) => {
      if (op === "CompileModule") return { code: String(args.code ?? "") };
      return undefined;
    };
  }

  #onData(chunk) {
    this.buf = Buffer.concat([this.buf, chunk]);
    for (;;) {
      if (this.buf.length < 4) return;
      const len = this.buf.readUInt32LE(0);
      if (this.buf.length < 4 + len) return;
      const payload = this.buf.subarray(5, 4 + len);
      this.buf = this.buf.subarray(4 + len);
      const msg = JSON.parse(payload.toString("utf8"));
      if (msg.t === "ready") {
        for (const w of this.waiters.splice(0)) w.ready?.();
        continue;
      }
      if (msg.t === "op_request") {
        // The mini host: the base answers `compile_module`; the pin's
        // `opReplier` answers the ops it cares about; everything else is a
        // JSH-OP refusal (the worker's own fallback rule).
        let data = null;
        let ok = false;
        const fromBase = this.baseReplier(msg.op, msg.args ?? {});
        if (fromBase !== undefined) {
          ok = true;
          data = fromBase;
        } else if (this.opReplier) {
          const replied = this.opReplier(msg.op, msg.args ?? {});
          if (replied !== undefined) {
            ok = true;
            data = replied;
          }
        }
        this.child.stdin.write(
          frame({ t: "op_response", id: msg.id, ok, data }),
        );
        continue;
      }
      for (const w of this.waiters.splice(0)) {
        if (w.id === msg.id) w.msg = msg;
      }
    }
  }

  async untilReady() {
    await new Promise((res) => {
      const w = { ready: res };
      this.waiters.push(w);
      // The ready frame already arrived: the waiter loop fired it.
      setTimeout(res, 2000);
    });
  }

  // Sends one call and resolves with the parsed reply (call_ok payload or
  // call_err diagnostic).
  async call(kind, payload) {
    const id = Math.floor(Math.random() * 1e9);
    const w = { id, msg: null };
    this.waiters.push(w);
    this.child.stdin.write(
      frame({ t: "call", id, entry: { kind, module: "", export: "" }, input: { payload } }),
    );
    const deadline = Date.now() + 8000;
    while (w.msg === null && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 10));
    }
    assert.notEqual(
      w.msg,
      null,
      `no reply for ${kind}; worker stderr: ${this.stderrText().slice(-800)}`,
    );
    if (w.msg.t === "call_err") return { err: w.msg.diagnostic };
    const json = Buffer.from(w.msg.chunks[0], "base64").toString("utf8");
    return { ok: JSON.parse(json) };
  }

  async close() {
    this.child.stdin.end();
    await Promise.race([this.exit, new Promise((r) => setTimeout(r, 1000))]);
    this.child.kill("SIGKILL");
  }
}

// --- fixtures ---------------------------------------------------------------

function writeRecord(dir, overrides = {}) {
  const record = {
    version: 1,
    requestId: "req-replay-1",
    buildId: "b-replay",
    request: { method: "GET", url: "/x", headers: [] },
    routeId: "routes/x",
    params: {},
    locals: null,
    loaders: [],
    fetches: overrides.fetches ?? [],
    cacheOps: [],
    sequences: overrides.sequences ?? { nowMs: [], randoms: [], uuids: [] },
    response: { status: 200, headers: [], bodyBase64: "", truncated: false },
  };
  const file = path.join(dir, "req-replay-1.json");
  writeFileSync(file, JSON.stringify(record));
  return file;
}

// A loader module the pins import through `loader:run`: it fetches
// `url`, and reports the non-deterministic sources verbatim.
function writeLoader(dir, url) {
  const file = path.join(dir, "pin-loader.mjs");
  writeFileSync(
    file,
    `export async function load(event) {
  const out = { now: Date.now(), rand: Math.random(), uuid: crypto.randomUUID() };
  try {
    const r = await fetch(${JSON.stringify(url)});
    out.status = r.status;
    out.body = await r.text();
  } catch (e) {
    out.fetchError = String(e && e.message ? e.message : e);
  }
  return out;
}
`,
  );
  return file;
}

function loaderCall(modulePath) {
  return {
    module_path: modulePath,
    route: "routes/x",
    params: {},
    url: "/x",
    method: "GET",
    headers: [],
    locals: {},
    locale: "",
  };
}

function b64Of(s) {
  return Buffer.from(s, "utf8").toString("base64");
}

// --- the pins ---------------------------------------------------------------

test("the replay worker plays the recorded fetch instead of the network", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ox-replay-"));
  const record = writeRecord(dir, {
    fetches: [
      {
        method: "GET",
        url: "http://127.0.0.1:9/recorded-health",
        requestBodyBase64: "",
        truncated: false,
        response: {
          status: 200,
          headers: [["content-type", "application/json"]],
          bodyBase64: b64Of('{"ok":true,"source":"record"}'),
          truncated: false,
        },
      },
    ],
    // The pin's loader also reads the non-deterministic sources; a replayed
    // run needs them in the record (a drained stream is RPL-004).
    sequences: { nowMs: [1731711000111], randoms: [0.25], uuids: ["00000000-0000-4000-8000-00000000000a"] },
  });
  const loader = writeLoader(dir, "http://127.0.0.1:9/recorded-health");
  const host = new WorkerHost({ OXIDE_REPLAY: record });
  try {
    await host.untilReady();
    const { ok } = await host.call("loader:run", loaderCall(loader));
    assert.equal(ok.status, 200);
    // The recorded body came back: the call never reached the network
    // (127.0.0.1:9 is the discard port — nothing listens there).
    assert.ok(
      ok.data.body.includes('"source":"record"'),
      `expected the recorded body, got ${ok.data.body}`,
    );
    assert.equal(ok.data.status, 200);
    // The install exposes the record identity on globalThis.
    const { ok: probe } = await host.call("replay:identity", {});
    assert.equal(probe.requestId, "req-replay-1");
    assert.equal(probe.buildId, "b-replay");
  } finally {
    await host.close();
  }
});

test("an unexpected call is refused with RPL-002 naming the call", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ox-replay-"));
  const record = writeRecord(dir, {
    fetches: [
      {
        method: "GET",
        url: "http://127.0.0.1:9/recorded-health",
        requestBodyBase64: "",
        truncated: false,
        response: { status: 200, headers: [], bodyBase64: b64Of("ok"), truncated: false },
      },
    ],
    sequences: { nowMs: [1731711000112], randoms: [0.5], uuids: ["00000000-0000-4000-8000-00000000000b"] },
  });
  const loader = writeLoader(dir, "http://127.0.0.1:9/NOT-in-the-record");
  const host = new WorkerHost({ OXIDE_REPLAY: record });
  try {
    await host.untilReady();
    const { ok } = await host.call("loader:run", loaderCall(loader));
    assert.ok(
      ok.data.fetchError.includes("RPL-002"),
      `expected RPL-002 in the fetch error, got ${ok.data.fetchError}`,
    );
    assert.ok(
      ok.data.fetchError.includes("http://127.0.0.1:9/NOT-in-the-record"),
      `the error must name the unexpected call, got ${ok.data.fetchError}`,
    );
  } finally {
    await host.close();
  }
});

test("Date.now, Math.random and crypto.randomUUID play from the record", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ox-replay-"));
  const record = writeRecord(dir, {
    sequences: { nowMs: [1731711000123], randoms: [0.42], uuids: ["00000000-0000-4000-8000-000000000001"] },
  });
  const loader = writeLoader(dir, "http://127.0.0.1:9/never-fetched");
  const host = new WorkerHost({ OXIDE_REPLAY: record });
  try {
    await host.untilReady();
    const { ok } = await host.call("loader:run", loaderCall(loader));
    assert.equal(ok.data.now, 1731711000123);
    assert.equal(ok.data.rand, 0.42);
    assert.equal(ok.data.uuid, "00000000-0000-4000-8000-000000000001");
  } finally {
    await host.close();
  }
});

test("the record mode captures fetches and sequences; drain returns and resets", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ox-replay-"));
  const loader = writeLoader(dir, "http://127.0.0.1:9/captured-call");
  const host = new WorkerHost({ OXIDE_RECORD: "1" });
  // The mini host answers the worker's fetch op so the call COMPLETES
  // (a captured op needs a response).
  host.opReplier = (op, args) => {
    if (op === "Fetch") {
      return {
        status: 201,
        headers: [["x-captured", "yes"]],
        body_b64: b64Of("captured-body"),
      };
    }
    return undefined; // JSH-OP refusal — the worker's own fallback rule
  };
  try {
    await host.untilReady();
    const { ok } = await host.call("loader:run", loaderCall(loader));
    assert.equal(ok.data.status, 201);
    assert.equal(ok.data.body, "captured-body");
    const { ok: drain1 } = await host.call("replay:drain", {});
    assert.equal(drain1.fetches.length, 1, "one completed fetch captured");
    assert.equal(drain1.fetches[0].method, "GET");
    assert.equal(drain1.fetches[0].url, "http://127.0.0.1:9/captured-call");
    assert.equal(drain1.fetches[0].response.status, 201);
    assert.equal(drain1.fetches[0].response.bodyBase64, b64Of("captured-body"));
    assert.equal(drain1.sequences.nowMs.length, 1, "one Date.now observation");
    assert.equal(drain1.sequences.randoms.length, 1, "one Math.random observation");
    assert.equal(drain1.sequences.uuids.length, 1, "one randomUUID observation");
    // The drain resets: a second one is empty.
    const { ok: drain2 } = await host.call("replay:drain", {});
    assert.equal(drain2.fetches.length, 0);
    assert.equal(drain2.sequences.nowMs.length, 0);
  } finally {
    await host.close();
  }
});

test("without the env vars the worker keeps today's fetch path", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ox-replay-"));
  const loader = writeLoader(dir, "http://127.0.0.1:9/plain-op-path");
  const host = new WorkerHost({});
  host.opReplier = (op) => {
    if (op === "Fetch") {
      return { status: 200, headers: [], body_b64: b64Of("op-path") };
    }
    return undefined;
  };
  try {
    await host.untilReady();
    const { ok } = await host.call("loader:run", loaderCall(loader));
    assert.equal(ok.data.body, "op-path", "the fetch op path is untouched");
    // The non-deterministic sources are NOT overridden (Math.random is a
    // real random; Date.now is the wall clock).
    const { ok: again } = await host.call("loader:run", loaderCall(loader));
    assert.notEqual(again.data.rand, 0.42);
    assert.ok(Math.abs(again.data.now - Date.now()) < 60_000, "Date.now is the wall clock");
    const { ok: drain } = await host.call("replay:drain", {});
    assert.deepEqual(drain, { fetches: [], cacheOps: [], sequences: { nowMs: [], randoms: [], uuids: [] } });
  } finally {
    await host.close();
  }
});
