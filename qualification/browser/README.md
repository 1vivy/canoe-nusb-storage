# Browser feasibility lab

This lab asks whether the proposed hosted application can execute its filesystem
work in a real browser. It is not a replacement adapter, driver qualification
suite, firmware implementation or phone installer. It serves **only generated
synthetic files**, on loopback, and never opens a USB device.

`prepare.py` copies pinned upstream sources into `.work/webapp-feasibility`,
applies small portability patches there, builds WASM and creates disposable
128 MiB ext4 / 32 MiB FAT16 images with Linux tools. Production sources and the
earlier ext4 qualification checkout are untouched. The source crate is a build
template; build its generated copy using the script, not Cargo in this folder.

## Run

Prerequisites: Rust's `wasm32-unknown-unknown` target, wasm-bindgen CLI **0.2.128**,
Bun, the repository's Playwright Chromium, e2fsprogs, dosfstools and mtools.
Install the browser with `node_modules/.bin/playwright-core install chromium`.

From the repository root:

```sh
python3 e2e/webapp-feasibility/prepare.py --wasm-bindgen /absolute/path/to/wasm-bindgen
bun e2e/webapp-feasibility/run.ts
python3 e2e/webapp-feasibility/verify.py
```

Use `--native-clock` with `prepare.py`, then run the browser probe to reproduce
the upstream `SystemTime::now()` panic. Use `--native-pid` instead to reproduce
the next `std::process::id()` panic after adapting the clock. Neither negative
case should reach a write. Build again without these flags for normal probes.
Other paths such as ext4 formatting/fsck still have native time calls; they are
not made portable or called by this experiment.

Results go under `.work/webapp-feasibility/results/`; build provenance is in
`build.json`. The checker reads the browser-produced images using independent
Linux programs and compares extracted bytes, rather than trusting the driver's
own cached readback. It never repairs a filesystem.

## What it exercises

- Pinned rust-fatfs: create/write/rename/read/delete/unmount, 2 MiB payload.
- Pinned am-fs-ext4: create/positional-write/rename/read/delete/flush, a complete
  32 MiB FAT image as payload. No removal of unsupported-feature guards.
- Short read, disconnected broker, failed flush and bounded timeout. The worker
  retires an uncertain session; a completed write is not treated as undone.
- Actual nusb browser permission-list enumeration. With a new isolated browser
  profile this is empty. This is **not USB transfer evidence**.
- The real `fastboot-protocol` nusb client's async API type-checks in the WASM
  build. No fastboot command is sent by the lab.

The filesystem worker blocks on a JavaScript SharedArrayBuffer mailbox. A
separate, unblocked browser context handles asynchronous requests. WASM linear
memory stays private: no wasm_thread, Rust atomics build or rebuilt standard
library is needed. HTTP COOP/COEP headers enable the shared JS mailbox.

The broker currently edits an in-memory image and deliberately yields between
requests. Its timings establish responsiveness, **not USB throughput**. Its
flush acknowledgement is simulated, **not device durability**. A real transport
must align/cache filesystem byte I/O into sectors and await SCSI status/flush;
raw FAT calls include tiny, unaligned reads and writes. Large transfer streaming,
mid-write unplug/reconnect, browser permission selection and background/tab-close
lifetimes remain separate gates.

See [the qualification record](../../docs/analysis/webapp-feasibility-qualification.md)
for results, preserved-stack commits and remaining browser gates.
