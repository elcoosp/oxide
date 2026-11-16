// V2-E01 worker pins: the Vue/Solid component-refresh emission — the
// compile step's HMR boundary. The spec (§V2-E01): "Vue `__VUE_HMR_RUNTIME__
// .reload(id, comp)` emitted by the Vue compile step (native or pool)";
// Solid modules get the `solid-refresh` registration boundary.
//
// The dev server (esm_dev) passes `hmr: true` on the pool payload; the
// BUILD's precompile passes nothing, so a production bundle never carries
// the boundary (byte-identical builds). The emission gates on:
//   - the `hmr` flag (dev-only),
//   - the client `generate` mode (SSR modules have no client runtime),
//   - the module not already carrying an `import.meta.hot` boundary
//     (the inject_hmr_boundary no-op convention — one definition site).
//
// Each pin spawns `oxide-worker.mjs` and speaks the framed protocol — the
// same harness `replay.test.mjs` uses. The Vue op needs the project's
// `@vue/compiler-sfc` (a `vue` install brings it); the Solid op needs the
// babel fallback (`@babel/core` + `babel-preset-solid`) and sucrase.
//
// Run: node --test crates/jshost/worker/hmr-refresh.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WORKER = path.join(path.dirname(fileURLToPath(import.meta.url)), "oxide-worker.mjs");
const PROJECT_ROOT = path.resolve(WORKER, "../../../..");

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
        this.child.stdin.write(
          frame({ t: "op_response", id: msg.id, ok: true, data: { code: String(msg.args?.code ?? "") } }),
        );
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

const VUE_SFC = `<script setup lang="ts">
import { ref } from 'vue'
const count = ref(0)
</script>

<template>
  <button data-testid="v-counter" @click="count++">{{ count }}</button>
</template>
`;

const SOLID_TSX = `export function Counter(props: { initial?: number }) {
  return <button data-testid="s-counter">{props.initial ?? 0}</button>;
}
`;

// --- pins: vue --------------------------------------------------------------

test("vue-compile with hmr:true emits the __VUE_HMR_RUNTIME__ reload boundary", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:vue-compile", {
      source: VUE_SFC,
      filename: "src/islands/Counter.vue",
      generate: "dom",
      hmr: true,
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `vue-compile failed: ${JSON.stringify(res.err ?? res)}`);
    const js = res.ok.js;
    // The spec-verbatim reload call rides the compile output.
    assert.match(js, /__VUE_HMR_RUNTIME__/);
    assert.match(js, /\.reload\(/);
    // The boundary self-accepts through import.meta.hot (so the generic
    // inject_hmr_boundary no-ops — one definition site).
    assert.match(js, /import\.meta\.hot/);
    assert.match(js, /import\.meta\.hot\.accept\(/);
    // The reload hands the FRESH module's default export (the component).
    assert.match(js, /mod\.default/);
    // The in-place mark: the boot replay must skip a URL the framework
    // already refreshed (the state the Vue reload preserves).
    assert.match(js, /markInPlaceRefresh/);
    // The id is the compile step's own stable short id (base64 of the path).
    const shortId = Buffer.from("src/islands/Counter.vue").toString("base64").replace(/[^A-Za-z0-9]/g, "").slice(0, 8);
    assert.ok(js.includes(`reload("${shortId}"`), "the reload id is the compile step's short id");
  } finally {
    await host.close();
  }
});

test("vue-compile without the hmr flag emits no boundary (build stays identical)", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:vue-compile", {
      source: VUE_SFC,
      filename: "src/islands/Counter.vue",
      generate: "dom",
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `vue-compile failed: ${JSON.stringify(res.err ?? res)}`);
    assert.ok(!res.ok.js.includes("import.meta.hot"), "no boundary in a build compile");
    assert.ok(!res.ok.js.includes("__VUE_HMR_RUNTIME__"), "no runtime import in a build compile");
  } finally {
    await host.close();
  }
});

test("vue-compile SSR output carries no client boundary", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:vue-compile", {
      source: VUE_SFC,
      filename: "src/islands/Counter.vue",
      generate: "ssr",
      hmr: true,
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `vue-compile failed: ${JSON.stringify(res.err ?? res)}`);
    assert.ok(!res.ok.js.includes("__VUE_HMR_RUNTIME__"), "SSR modules have no client runtime");
  } finally {
    await host.close();
  }
});

// --- pins: solid ------------------------------------------------------------

test("solid-compile with hmr:true emits the solid refresh boundary", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:solid-compile", {
      source: SOLID_TSX,
      filename: "src/islands/Counter.tsx",
      ssr: false,
      hmr: true,
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `solid-compile failed: ${JSON.stringify(res.err ?? res)}`);
    const js = res.ok.js;
    assert.match(js, /__oxide_solid_refresh/);
    assert.match(js, /import\.meta\.hot\.accept\(/);
  } finally {
    await host.close();
  }
});

test("solid-compile without the hmr flag emits no boundary", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:solid-compile", {
      source: SOLID_TSX,
      filename: "src/islands/Counter.tsx",
      ssr: false,
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `solid-compile failed: ${JSON.stringify(res.err ?? res)}`);
    assert.ok(!res.ok.js.includes("import.meta.hot"), "no boundary in a build compile");
  } finally {
    await host.close();
  }
});

test("solid-compile SSR output carries no client boundary", async () => {
  const host = new WorkerHost();
  try {
    await host.untilReady();
    const res = await host.call("tooling:solid-compile", {
      source: SOLID_TSX,
      filename: "src/islands/Counter.tsx",
      ssr: true,
      hmr: true,
      root: PROJECT_ROOT,
    });
    assert.ok(res.ok, `solid-compile failed: ${JSON.stringify(res.err ?? res)}`);
    assert.ok(!res.ok.js.includes("__oxide_solid_refresh"), "SSR modules have no client runtime");
  } finally {
    await host.close();
  }
});
