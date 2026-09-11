# canoe-nusb-storage

Shared asynchronous USB Bulk-Only Transport/SCSI and thin FAT/ext4 composition
for CANOE BOOT MANAGER. This workspace is under bring-up, not release-qualified.

The transport owns one caller-selected vendor interface (`ff/06/50`); it does
not detach or claim OS-managed class `08` storage. On any uncertain command,
retire the session and reconnect. A command status is not a durability claim:
filesystem flush must reach SCSI SYNCHRONIZE CACHE and its successful CSW.

Linux hosts can use the scoped [managed USB access rule](contrib/udev/README.md)
to grant the active local user access to `1209:ca0f`.

Live persist provisioning requires an independently retained backup before
writable access. Full-partition flashing is not a fallback in this workspace.

`nusb-scsi` owns USB/BOT once. `nusb-fatfs` and `nusb-ext4` compose a synchronous
block callback supplied by a filesystem worker; the separate async broker must
remain unblocked. Production-worker qualification and independent Linux oracles live
in `qualification/filesystem-browser` and `qualification/linux-oracle`. The
earlier experimental probe in `qualification/browser` is retained as historical
evidence only. Actual Linux Chromium and Windows Edge USB/worker
qualification is recorded in the separate
[canoe-harnesses managed-WebUSB fixture](https://github.com/1vivy/canoe-harnesses/tree/ac45912dea4ade8eafdb856180e773bae1291121/qualification/managed-webusb).
Those runs use the production WASM engines through the guest OS USB controller
and WinUSB/Linux USB stack, with synthetic QEMU-backed disks. They cover FAT and
ext4 mutations, read-only refusal, fresh readback, plain-JBD2 replay, sync/eject,
read interruption, and fresh paired-device reconnect. They do not establish
physical-phone browser-write or full application-deployment coverage.

## Browser fastboot facade

`canoe-usb-web` builds with `wasm-bindgen --target web` and exports
`requestFastboot()` (user gesture), `openFastboot(USBDevice)` and
`FastbootSession`: `getVar`, `command`, bounded `fetch`, `flash`, `hashRange`,
`usable`, and awaited `close`. Offsets/lengths represented by Rust `u64` use
JavaScript BigInt. Transport/protocol uncertainty or timeout retires the session and closes the selected device. A complete device FAIL keeps framing synchronized and permits optional capability probes.
The application supplies review, backup, readback, capability and retry policy.

`hashRange` uses CANOE-BDS `sha256-range-v1` and decodes the 43-character
base64url digest to 32 bytes. Generic upstream code contains no CANOE commands.

All consumers should carry this workspace's `patch.crates-io.nusb` pin if
they integrate these crates into a larger Rust workspace.

The [browser managed storage API](browser/README.md) exposes fixed-access
sessions, bounded byte ranges, explicit sync/eject and awaited close over the
same SCSI implementation used by the native probe.

CANOE BOOT MANAGER implements direct managed writes behind reviewed operations,
an independently retained full persist backup, fresh target identity checks,
and exclusive export ownership. The ext4 adapter supports checked plain-JBD2
replay before file operations and explicit clean finish, while refusing
checksummed/unsupported or aborted journals; see
[journal lifecycle](qualification/journal-lifecycle.md). Writable mount/replay is
itself a mutation and must follow the backup gate. Isolated Linux and Windows
browser runs verified the complete raw image stays unchanged during read-only
inspection before replay. Physical-phone browser writes and checksummed journal
recovery remain separate acceptance limits.
