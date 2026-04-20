#!/usr/bin/env bash
# Build the static deployment bundle for dux-web-browser.
#
# Output: ./dist/ — a fully relocatable directory with index.html, the
# hand-authored JS/CSS, and the wasm-pack-generated glue + .wasm module
# arranged so index.html can be served by any static host.
#
# Usage:
#   ./build.sh            # release build + wasm-opt (production)
#   ./build.sh --dev      # debug build, skips wasm-opt (faster, huge .wasm)
#
# Honours LLVM_PATH if clang isn't on the default PATH (ring's build
# script needs clang for wasm32).
set -euo pipefail

mode="--release"
if [[ "${1:-}" == "--dev" ]]; then
  mode="--dev"
fi

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

if [[ -n "${LLVM_PATH:-}" ]]; then
  export PATH="$LLVM_PATH:$PATH"
fi

command -v clang >/dev/null || {
  echo "dux-web-browser: clang is required for the iroh/ring WASM build." >&2
  echo "Install via your package manager (e.g. 'brew install llvm' or" >&2
  echo "'sudo dnf install clang') or set LLVM_PATH=/path/to/llvm/bin." >&2
  exit 1
}

command -v wasm-pack >/dev/null || {
  echo "dux-web-browser: wasm-pack not found. Install with 'cargo install wasm-pack'." >&2
  exit 1
}

rm -rf dist pkg
wasm-pack build --target web --out-dir pkg "$mode"

mkdir -p dist/pkg
cp web/index.html web/style.css web/app.js dist/
cp pkg/dux_web_browser.js pkg/dux_web_browser_bg.wasm dist/pkg/
cp pkg/dux_web_browser_bg.wasm.d.ts dist/pkg/ 2>/dev/null || true
cp pkg/dux_web_browser.d.ts dist/pkg/ 2>/dev/null || true

wasm_bytes=$(stat -c%s dist/pkg/dux_web_browser_bg.wasm 2>/dev/null || stat -f%z dist/pkg/dux_web_browser_bg.wasm)
wasm_mb=$(awk "BEGIN{printf \"%.2f\", $wasm_bytes/1048576}")
gzip_bytes=$(gzip -9 -c dist/pkg/dux_web_browser_bg.wasm | wc -c)
gzip_kb=$(awk "BEGIN{printf \"%.0f\", $gzip_bytes/1024}")
echo "dux-web-browser: built dist/ (wasm: ${wasm_mb} MiB raw, ~${gzip_kb} KiB gzipped)"
