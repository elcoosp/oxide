# @oxide-fw/cli

Installs the prebuilt `oxide` CLI binary so `npm run dev` / `npm run build` /
`npm run start` work in a scaffolded project without a global install or a
Rust toolchain.

On `postinstall` the package downloads the release binary for your platform
from the matching GitHub Release, verifies it against the release's
`SHA256SUMS`, and caches it in `.oxide-bin/`. Running `oxide` then execs that
binary (exit codes and signals pass through).

## Environment overrides

| Variable | Effect |
| --- | --- |
| `OXIDE_BIN` | Use this existing binary instead of the downloaded one |
| `OXIDE_SKIP_DOWNLOAD=1` | Skip the postinstall download (air-gapped installs) |
| `OXIDE_CLI_DOWNLOAD_BASE` | Mirror base URL (`…/releases/download/v<version>`) |

## Supported platforms

`linux-x64`, `linux-arm64`, `linux-musl-x64`, `linux-musl-arm64`,
`darwin-x64`, `darwin-arm64`, `win32-x64` (produced by
`.github/workflows/release-binaries.yml`). Any other platform prints a hint to
`cargo install oxide-fw-cli`.

## License

MIT OR Apache-2.0
