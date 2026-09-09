# Production filesystem worker qualification

Run from the repository root after installing e2fsprogs, dosfstools, Bun,
Playwright Chromium, Rust WASM target and matching wasm-bindgen:

```sh
python3 qualification/filesystem-browser/prepare.py
CANOE_WASM_BINDGEN=/path/to/wasm-bindgen bash browser/build.sh
bun qualification/filesystem-browser/run.ts
```

The script serves only local synthetic fixtures and built artifacts with
COOP/COEP, opens a fresh headless Chromium process, and uses no real USB device.
Eleven cases cover both filesystem engines, explicit read-only rejection,
exclusive ownership, named writes, unrelated-file preservation, fresh reopen,
plain-JBD2 committed recovery and I/O failures/timeouts. Three further cases
compose the actual nusb/BOT/range WASM module with the filesystem worker using
a synthetic WebUSB target, including aborting a pending USB read before close.

The pending journal fixture is independently encoded from Linux-created ext4
metadata: committed transaction42 changes the oracle inode and superblock;
uncommitted transaction43 must be ignored. Result images pass fsck.fat or
e2fsck read-only checks, and the oracle inode must have mode0600.

Evidence and disposable images stay under `.work/browser-storage`. These tests
do not qualify physical USB, power-loss atomicity or platform driver behavior.
