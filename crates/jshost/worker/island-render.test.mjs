// V2-F01 worker pins: the `island:render` op — the mixed-framework
// islands' server-side half. The page adapter renders islands of its own
// framework; a FOREIGN island rides the self-contained pending placeholder
// (`<ox-island data-ox-fw ... data-ox-pending='1'>`) and the worker host
// resolves it through this op: every adapter in the project registers its
// island renderer in the isolate (`globalThis.__oxide_island_renderers[fw]`)
// when its adapter module is imported, and the op dispatches
// `(name, propsB64)` to the named framework's renderer.
//
// Each pin spawns `oxide-worker.mjs` as a child process (the same way the
// jshost pool does) and speaks the length-prefixed JSON frames on stdio —
// the same harness `replay.test.mjs` uses. The renderer fixture registers
// through the existing `tooling:module` op (require → call), so the pins
// exercise the real registration path an adapter module takes.
//
// Run: node --test crates/jshost/worker/island-render.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
  constructor() {
    this.child = spawn(process.execPath, [WORKER], {
      env: { ...process.env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.buf = Buffer.alloc(0);
    this.waiters = [];
    this.exit = new Promise((res) => this.child.on("exit", res));
    const stderr = [];
    this.child.stderr.on("data", (c) => stderr.push(String(c)));
    this.stderrText = () => stderr.join("");
    this.child.stdout.on("data", (chunk) => this.#onData(chunk));
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
      if (msg.t === "ready") continue;
      if (msg.t === "op_request") {
        // The island pins never exercise host ops; answer nothing so an
        // unexpected op_request surfaces as the worker's own JSH-OP refusal.
        this.child.stdin.write(frame({ t: "op_response", id: msg.id, ok: false, data: { code: "JSH-OP", message: "no ops in this pin" } }));
        continue;
      }
      for (const w of this.waiters.splice(0)) {
        if (w.id === msg.id) w.msg = msg;
      }
    }
  }

  async untilReady() {
    await new Promise((res) => setTimeout(res, 300));
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

// A module the pins import through `tooling:module`: its `register` export
// installs island renderers on the isolate — the same registration surface
// (`globalThis.__oxide_island_renderers[fw]`) an adapter module populates
// when the worker imports it (§V2-F01 item 4).
function writeRegistryFixture(dir) {
  const file = path.join(dir, "island-renderers.mjs");
  writeFileSync(
    file,
    `globalThis.__oxide_island_renderers = {
  vue: (name, propsB64) => \`<span data-fw="vue">\${name}:\${propsB64}</span>\`,
  svelte: (name, propsB64) => \`<b data-fw="svelte">\${name}</b>\`,
  boom: () => { throw new Error("island blew up"); },
};
export function register() { return Object.keys(globalThis.__oxide_island_renderers); }
`,
  );
  return file; // the plain path: `tooling:module` requires CJS-style
}

async function registerRenderers(host, fixtureUrl) {
  const res = await host.call("tooling:module", {
    module: fixtureUrl,
    export: "register",
    args: [],
  });
  assert.ok(res.ok, `register failed: ${JSON.stringify(res.err ?? res)}`);
  assert.deepEqual(res.ok.sort(), ["boom", "svelte", "vue"]);
}

// --- pins -------------------------------------------------------------------

test("island:render dispatches to the named framework's registered renderer", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const fixtureUrl = writeRegistryFixture(mkdtempSync(path.join(tmpdir(), "oxide-island-")));
    await registerRenderers(host, fixtureUrl);
    const res = await host.call("island:render", {
      fw: "vue",
      name: "Price",
      propsB64: "eyJhIjoxfQ==",
    });
    assert.ok(res.ok, `island:render failed: ${JSON.stringify(res.err ?? res)}`);
    assert.equal(res.ok.html, '<span data-fw="vue">Price:eyJhIjoxfQ==</span>');
  } finally {
    await host.close();
  }
});

test("island:render renders each framework through its own renderer", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const fixtureUrl = writeRegistryFixture(mkdtempSync(path.join(tmpdir(), "oxide-island-")));
    await registerRenderers(host, fixtureUrl);
    const vue = await host.call("island:render", { fw: "vue", name: "A", propsB64: "eA==" });
    const svelte = await host.call("island:render", { fw: "svelte", name: "B", propsB64: "eA==" });
    assert.equal(vue.ok.html, '<span data-fw="vue">A:eA==</span>');
    assert.equal(svelte.ok.html, '<b data-fw="svelte">B</b>');
  } finally {
    await host.close();
  }
});

test("island:render refuses an unregistered framework (the registry is the contract)", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("island:render", { fw: "solid", name: "X", propsB64: "" });
    assert.ok(res.err, "an unregistered framework must refuse");
    assert.match(res.err.code, /JSH-ISLAND/);
    assert.match(res.err.message, /solid/);
  } finally {
    await host.close();
  }
});

test("island:render names the island when the renderer throws", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const fixtureUrl = writeRegistryFixture(mkdtempSync(path.join(tmpdir(), "oxide-island-")));
    await registerRenderers(host, fixtureUrl);
    const res = await host.call("island:render", { fw: "boom", name: "Counter", propsB64: "" });
    assert.ok(res.err, "a throwing renderer must refuse");
    assert.match(res.err.code, /JSH-ISLAND/);
    assert.match(res.err.message, /Counter/);
    assert.match(res.err.message, /island blew up/);
  } finally {
    await host.close();
  }
});

test("island:render requires a framework", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("island:render", { name: "X", propsB64: "" });
    assert.ok(res.err, "a missing fw must refuse");
    assert.match(res.err.code, /JSH-ISLAND/);
    assert.match(res.err.message, /fw/);
  } finally {
    await host.close();
  }
});

process.on("exit", () => {
  // best-effort temp cleanup
  try { rmSync(path.join(tmpdir(), "oxide-island-"), { recursive: true, force: true }) } catch { /* ignore */ }
});
