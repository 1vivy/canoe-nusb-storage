#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${CANOE_WASM_BINDGEN:=wasm-bindgen}"
RUSTFLAGS='--cfg=web_sys_unstable_apis' cargo build --locked --release --target wasm32-unknown-unknown -p canoe-usb-web -p canoe-fs
mkdir -p .work/browser-storage/pkg
"$CANOE_WASM_BINDGEN" --target web --out-dir .work/browser-storage/pkg target/wasm32-unknown-unknown/release/canoe_usb_web.wasm
"$CANOE_WASM_BINDGEN" --target web --out-dir .work/browser-storage/pkg target/wasm32-unknown-unknown/release/canoe_fs.wasm
cp browser/filesystem.js browser/filesystem-worker.js browser/filesystem.d.ts .work/browser-storage/
