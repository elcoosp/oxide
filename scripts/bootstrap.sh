#!/usr/bin/env bash
# Bootstrap the oxide build toolchain in this sandbox: rust stable, nasm >= 2.16, ship CLI.
set -euo pipefail

export HOME=/home/z
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
LOG=/home/z/my-project/scripts/bootstrap.log
exec > >(tee -a "$LOG") 2>&1

echo "== [1/3] rustup + stable toolchain =="
if ! command -v rustc >/dev/null 2>&1; then
  curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain stable
fi
rustc --version
cargo --version

echo "== [2/3] nasm 2.16.03 from source (nasm.us unreachable; GitHub mirror) =="
if ! command -v nasm >/dev/null 2>&1; then
  rm -rf /tmp/nasm-build
  mkdir -p /tmp/nasm-build
  git clone --depth 1 --branch nasm-2.16.03 https://github.com/netwide-assembler/nasm /tmp/nasm-build/nasm
  cd /tmp/nasm-build/nasm
  ./autogen.sh >/dev/null
  ./configure --prefix="$HOME/.local" >/dev/null
  make -j2 >/dev/null
  # the git checkout has no prebuilt nasm.1 (release tarball only), so
  # install the binaries directly instead of `make install`
  mkdir -p "$HOME/.local/bin"
  install -m 755 nasm ndisasm "$HOME/.local/bin/"
fi
nasm --version

echo "== [3/3] cargo install ship =="
if ! command -v ship >/dev/null 2>&1; then
  cargo install --git https://github.com/elcoosp/ship --locked
fi
ship --version || true

echo "== BOOTSTRAP DONE =="
