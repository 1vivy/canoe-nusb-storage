# Browser storage APIs

Build with `CANOE_WASM_BINDGEN=/path/to/wasm-bindgen bash browser/build.sh`
using wasm-bindgen0.2.128. The generated module is
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
interface. They do not detach drivers or claim manual class08 storage.
GET_MAX_LUN, readiness and capacity complete before a session is exposed.

Access is fixed as `read-only` or `read-write`. Reading and writing use BigInt
byte offsets and lengths from1 byte through4MiB per call. The shared Rust range
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
