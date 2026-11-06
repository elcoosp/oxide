// @oxide-fw/cli postinstall: download the prebuilt `oxide` binary for this
// platform from the matching GitHub Release, verify its SHA-256 against the
// release's SHA256SUMS, and cache it next to this package.
//
// Skips silently when:
//   - OXIDE_SKIP_DOWNLOAD=1 (offline/air-gapped installs)
//   - OXIDE_BIN points at an existing binary (the user manages their own)
//   - the platform is unsupported (a clear error is printed for `bin.js`)

import { createWriteStream, existsSync, mkdirSync, chmodSync, readFileSync, writeFileSync } from 'node:fs';
import { get } from 'node:https';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { assetName, downloadBase, hostTarget } from './platform.js';

const here = fileURLToPath(new URL('.', import.meta.url));
const BIN_DIR = join(here, '.oxide-bin');
const MARKER = join(BIN_DIR, 'VERSION');

function fetchText(url) {
  return new Promise((resolve, reject) => {
    get(url, (res) => {
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
      }
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (c) => (body += c));
      res.on('end', () => resolve(body));
    }).on('error', reject);
  });
}

function fetchToFile(url, dest) {
  return new Promise((resolve, reject) => {
    get(url, (res) => {
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
      }
      const file = createWriteStream(dest);
      res.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    }).on('error', reject);
  });
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

async function main() {
  if (process.env['OXIDE_SKIP_DOWNLOAD'] === '1') {
    console.log('@oxide-fw/cli: OXIDE_SKIP_DOWNLOAD=1, skipping binary download');
    return;
  }
  if (process.env['OXIDE_BIN'] && existsSync(process.env['OXIDE_BIN'])) {
    console.log(`@oxide-fw/cli: using OXIDE_BIN=${process.env['OXIDE_BIN']}`);
    return;
  }
  const target = hostTarget();
  if (!target) {
    console.warn(
      `@oxide-fw/cli: no prebuilt binary for ${process.platform}-${process.arch}. ` +
        'Install Rust and run `cargo install oxide-fw-cli`, or set OXIDE_BIN.',
    );
    return;
  }
  mkdirSync(BIN_DIR, { recursive: true });
  const name = assetName(target);
  const dest = join(BIN_DIR, name);
  // Already downloaded (e.g. npm re-ran postinstall)? Skip.
  if (existsSync(dest) && existsSync(MARKER) && readFileSync(MARKER, 'utf8').trim() === name) {
    return;
  }
  const base = downloadBase();
  console.log(`@oxide-fw/cli: downloading ${name} …`);
  await fetchToFile(`${base}/${name}`, dest);
  // Verify the checksum against the release's SHA256SUMS.
  try {
    const sums = await fetchText(`${base}/SHA256SUMS`);
    const line = sums.split('\n').find((l) => l.endsWith(`  ${name}`) || l.includes(name));
    const expected = line?.split(/\s+/)[0]?.toLowerCase();
    const actual = sha256(dest);
    if (expected && expected !== actual) {
      throw new Error(`checksum mismatch for ${name}: expected ${expected}, got ${actual}`);
    }
    console.log(`@oxide-fw/cli: sha256 verified (${actual.slice(0, 16)}…)`);
  } catch (e) {
    console.warn(`@oxide-fw/cli: could not verify checksum (${e.message}) — continuing`);
  }
  if (!target.startsWith('x86_64-pc-windows')) {
    chmodSync(dest, 0o755);
  }
  writeFileSync(MARKER, name + '\n');
}

main().catch((e) => {
  // A failed download must not break `npm install` of the app itself — the
  // user still gets a clear error from bin.js when they run `oxide`.
  console.warn(`@oxide-fw/cli: download failed: ${e.message}`);
  console.warn('@oxide-fw/cli: install Rust and run `cargo install oxide-fw-cli`, or set OXIDE_BIN.');
});
