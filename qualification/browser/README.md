# Maintained-fork browser qualification

This lab was extracted from canoe-boot-manager `7.0.0-b4-final`.
`evidence/2026-09-08.json` records its historical disposable-patch result.
`evidence/b5-maintained-forks.json` reruns the same eight cases against maintained
public forks without editing their sources. Both native checkers and exact
independent extraction pass. USB data transfers are not covered by this lab.

From the repository root:

```sh
bun install
python3 qualification/browser/prepare.py --wasm-bindgen /path/to/wasm-bindgen
bun qualification/browser/run.ts
python3 qualification/browser/verify.py
```

Prerequisites: Rust wasm32-unknown-unknown, wasm-bindgen 0.2.128, Bun,
Playwright Chromium, Linux mkfs.ext4/mkfs.fat/e2fsck/fsck.fat/debugfs/mcopy.
The build downloads checksum-pinned fork archives, locks Cargo dependencies,
and creates synthetic images under `.work/webapp-feasibility`.
It never opens a phone, native raw device or private persist backup.

The worker keeps WASM memory private and uses a JS SharedArrayBuffer mailbox
with a separate asynchronous broker. Short I/O, disconnect, failed flush and
timeout retire the fixture session. COOP/COEP headers are set by the local runner.
