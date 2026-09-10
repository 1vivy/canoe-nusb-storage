# USB failure ownership

Build and inspect the real browser facade with synthetic WebUSB failures:

```sh
cargo build --locked -p canoe-usb-web --target wasm32-unknown-unknown
/path/to/wasm-bindgen --target web --out-dir .work/usb-error-probe target/wasm32-unknown-unknown/debug/canoe_usb_web.wasm
bun qualification/usb-errors/run.ts
```

Use wasm-bindgen 0.2.128 and Playwright Chromium. Six cases cover failed
fastboot initialization, managed-interface initialization and nested nusb
initialization, with successful and rejected cleanup. Each checks that the
original failure survives and cleanup detail remains available; a direct
browser exception also retains its identity through `cause`. The fixture
forbids `navigator.usb` access and never connects to a physical device.
