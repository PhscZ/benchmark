#!/usr/bin/env bash
# Builds the WebAssembly package into web/pkg/.
#
# Run from anywhere; paths are resolved relative to this script.
#
# Requires:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.126
#
# The wasm-bindgen CLI version MUST match the `wasm-bindgen` crate version in
# Cargo.toml exactly, or the CLI refuses to process the generated module.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Pinned to the `wasm-bindgen` crate version in the workspace Cargo.toml.
REQUIRED_BINDGEN="0.2.126"

if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "error: wasm-bindgen is not on PATH." >&2
  echo "       cargo install wasm-bindgen-cli --version ${REQUIRED_BINDGEN}" >&2
  exit 1
fi

ACTUAL_BINDGEN="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$ACTUAL_BINDGEN" != "$REQUIRED_BINDGEN" ]; then
  echo "error: wasm-bindgen CLI is ${ACTUAL_BINDGEN}, but the crate is pinned to ${REQUIRED_BINDGEN}." >&2
  echo "       cargo install wasm-bindgen-cli --version ${REQUIRED_BINDGEN} --force" >&2
  exit 1
fi

if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
  echo "error: the wasm32-unknown-unknown target is not installed." >&2
  echo "       rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi

echo "==> building flappy_web (wasm-release)"
cargo build --target wasm32-unknown-unknown --profile wasm-release -p flappy_web

WASM="target/wasm32-unknown-unknown/wasm-release/flappy_web.wasm"
if [ ! -f "$WASM" ]; then
  echo "error: expected $WASM to exist after the build." >&2
  exit 1
fi

echo "==> generating web/pkg"
rm -rf web/pkg
mkdir -p web/pkg
# --target web emits an ES module that index.html imports directly.
# --no-typescript skips the .d.ts files, which this project does not consume.
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir web/pkg \
  "$WASM"

echo "==> done"
ls -1 web/pkg
echo
echo "Serve it with:  python3 web/serve.py"
