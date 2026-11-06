// Shared platform → release-asset mapping for the @oxide-fw/cli installer.
// The asset names are produced by .github/workflows/release-binaries.yml:
//   oxide-<version>-<target>[.exe]
// and checksummed in SHA256SUMS at the same release.

import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const pkg = require('./package.json');

/** GitHub repository to fetch releases from. */
export const REPO = 'elcoosp/oxide';

/** Version of the binary to fetch (matches this package's release). */
export const VERSION = pkg.version;

/** npm platform/arch → Rust release target triple. */
export const PLATFORM_TARGETS = {
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-musl-x64': 'x86_64-unknown-linux-musl',
  'linux-musl-arm64': 'aarch64-unknown-linux-musl',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

/** Resolve this host's release target triple, or null when unsupported. */
export function hostTarget(platform = process.platform, arch = process.arch) {
  const musl = platform === 'linux' && (() => {
    try {
      // Best-effort musl detection: Alpine's ld-linux path is absent and
      // /etc/alpine-release exists.
      return require('node:fs').existsSync('/etc/alpine-release');
    } catch {
      return false;
    }
  })();
  const key = platform === 'linux' && musl ? `linux-musl-${arch}` : `${platform}-${arch}`;
  return PLATFORM_TARGETS[key] ?? null;
}

/** Asset file name for a target, e.g. `oxide-0.1.0-x86_64-unknown-linux-gnu`. */
export function assetName(target, version = VERSION) {
  const ext = target.startsWith('x86_64-pc-windows') ? '.exe' : '';
  return `oxide-${version}-${target}${ext}`;
}

/** Base URL for release downloads (overridable for mirrors/air-gapped envs). */
export function downloadBase(version = VERSION) {
  const base = process.env['OXIDE_CLI_DOWNLOAD_BASE'];
  if (base) return base.replace(/\/$/, '');
  return `https://github.com/${REPO}/releases/download/v${version}`;
}
