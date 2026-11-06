#!/usr/bin/env node
// @oxide-fw/cli bin shim: exec the cached prebuilt `oxide` binary, or point
// the user at a fix when it is missing. Passes through exit codes and stdio;
// signals are forwarded so `oxide dev` shuts down cleanly on Ctrl-C.
import { spawn } from 'node:child_process';
import { existsSync, chmodSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { assetName, hostTarget } from './platform.js';

const here = fileURLToPath(new URL('.', import.meta.url));

function binaryPath() {
  // OXIDE_BIN always wins (lets a cargo-built binary satisfy the shim).
  if (process.env['OXIDE_BIN']) return process.env['OXIDE_BIN'];
  const target = hostTarget();
  if (!target) return null;
  const p = join(here, '.oxide-bin', assetName(target));
  if (!existsSync(p)) return null;
  if (!target.startsWith('x86_64-pc-windows')) {
    try { chmodSync(p, 0o755); } catch { /* best effort */ }
  }
  return p;
}

const bin = binaryPath();

if (!bin || !existsSync(bin)) {
  console.error(
    'oxide: the prebuilt CLI binary is not installed.\n\n' +
      'Fixes:\n' +
      '  - reinstall with `npm install @oxide-fw/cli` (downloads the binary in postinstall)\n' +
      '  - or build from source: `cargo install oxide-fw-cli`\n' +
      '  - or point OXIDE_BIN at an existing oxide binary',
  );
  process.exit(127);
}

const child = spawn(bin, process.argv.slice(2), {
  stdio: ['inherit', 'inherit', 'inherit'],
  windowsHide: true,
});

for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(sig, () => {
    if (!child.killed && child.pid) child.kill(sig);
  });
}

child.on('error', (e) => {
  console.error(`oxide: failed to exec ${bin}: ${e.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  process.exit(code ?? 1);
});
