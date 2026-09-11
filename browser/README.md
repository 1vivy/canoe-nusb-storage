# Browser storage APIs

Build with `CANOE_WASM_BINDGEN=/path/to/wasm-bindgen bash browser/build.sh`
using wasm-bindgen 0.2.128. The generated module is
`.work/browser-storage/pkg/canoe_usb_web.js` plus its WASM and snippets.
Import it into the application's WASM facade or serve the complete generated
directory. It also exports the existing fastboot session API.

```js
const session = await requestManagedStorage('read-only'); // user gesture
const device = session.usbDevice(); // retain for a later fresh open
const identity = session.identity();
const {bytes, blockSize} = session.capacity();
const part = await session.readRange(0n, Math.min(bytes, 4 * 1024 * 1024));
await session.close();
```

`openManagedStorage(device, access)` reopens a previously granted device.
Both functions require the exact managed `1209:ca0f`, active `ff/06/50`
interface. They do not detach drivers or claim manual class 08 storage.
GET_MAX_LUN, readiness and capacity complete before a session is exposed.

Access is fixed as `read-only` or `read-write`. Reading and writing use BigInt
byte offsets and lengths from 1 byte through 4 MiB per call. The shared Rust range
layer validates bounds and preserves untouched parts of unaligned sectors.
`capacity()` returns exactly representable Number fields: blocks, blockSize
and bytes. `identity()` returns observed interface facts, negotiated maximum
LUN, access mode and the implementation's maximum range size; it does not bind
the export to an earlier phone or assert a filesystem identity.

Writable access is an application decision. Before writable ext4 mount, the
app must independently retain/reopen its backup, compare raw source identity
and establish exclusive ownership. Journal recovery during writable mount is
already a mutation. These APIs do not invent a backup or approval receipt.

`writeRange(offset, Uint8Array)` copies the bounded source before its first
await. It does not implicitly flush or verify. Use filesystem finish, then
`sync()` (SCSI SYNCHRONIZE CACHE), then `close()`, and a fresh read-only open
and filesystem mount for verification. Close does not sync or eject.
`eject()` ends the export, retires the session and awaits device close; it
does not substitute for filesystem finish or readback.

Operations are serialized by their session. Timeouts or uncertain transport
framing retire the session and await WebUSB close. A complete SCSI refusal or
pre-I/O validation error may retain synchronized framing. The application
must still treat a failed write as unfinished and inspect its effects.
Close errors are propagated. Dropping/freeing a Rust/JS wrapper alone is not
an awaited close; call close explicitly. Cancelling an initialization promise
externally requires the owner to await the raw USBDevice.close().

Run `bun qualification/managed-browser/run.ts` after the build for actual
Chromium/WASM tests against the synthetic WebUSB target. This executes the
real nusb/SCSI stack but does not qualify physical browser USB or persistence.

## Filesystem worker

The same build generates `pkg/canoe_fs.js` and copies `filesystem.js` and
`filesystem-worker.js` beside `pkg/`. Serve that complete directory with:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

Import `openFilesystem` from `filesystem.js`:

```js
const fs = await openFilesystem({storage: session, kind: 'ext4', access: 'read-only'});
const info = await fs.inspect();
const existing = await fs.list('/');
await fs.finish();
await session.close();
```

The exported TypeScript interface is in `filesystem.d.ts`. Methods are
inspect, stat, list, read, createFile, write, truncate, mkdir, remove, rename,
finish and abort. Creation is exclusive, rename never overwrites, truncation
only shrinks, and directory removal requires it to be empty. Use explicit
staging/publication steps in the application instead of assuming atomic
replacement. No stage or old directory is implicitly adopted or migrated.

The broker allows one filesystem owner per storage session and serializes file
operations. `flush()` and `freshRead()` preserve the mounted owner. `finish()`
releases that filesystem owner and leaves the caller's transport available; it
can be reused by a later mount or explicitly synchronized/closed/ejected by its
owner. A failed session is retired rather than reused.
Abort terminates the worker. If I/O is pending, the broker uses its retained
USB grant to abort the request, awaits settlement, then awaits session close; it does not clear a
journal's recovery marker or claim clean release.

Each mailbox request has a sequence and ownership token. Delayed completions,
short reads, failed writes/flushes and timeouts retire the worker and close its
transport. WASM memory is private; a SharedArrayBuffer carries at most 1 MiB by
default between the blocked filesystem worker and a separate async broker.
The main context remains free to execute WebUSB promises and update the UI.
The caller must not concurrently use the raw USBDevice or storage object.
A non-USB transport may supply `abortPending()`; otherwise its `close()` must
abort and settle outstanding requests. Teardown is bounded and propagates
failure instead of reporting a clean release.

`inspect()` includes filesystem geometry and free bytes. Ext4 additionally
reports reserved bytes, free inodes, UUID, filesystem/journal feature flags,
recovery state whether the checked write format is supported, and its current write blocker. Pending
recovery is normal; space counts read before replay describe that pre-replay
state. Inspect again after an authorized writable mount before allocation.

Validation commands:

```sh
python3 qualification/filesystem-browser/prepare.py
bash browser/build.sh
bun qualification/filesystem-browser/run.ts
bun qualification/linux-oracle/run.ts .work/browser-storage
```

Fixtures and resulting images stay under `.work/browser-storage/`. The tests
cover named FAT/ext4 operations, read-only enforcement, exclusive ownership,
fresh readback, plain-JBD2 committed recovery, failure retirement and a live
UI heartbeat. Independent fsck.fat/e2fsck checks run on the resulting files.
