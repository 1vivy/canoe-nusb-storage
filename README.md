# canoe-nusb-storage

Shared asynchronous USB Bulk-Only Transport/SCSI and thin FAT/ext4 composition
for CANOE BOOT MANAGER. This workspace is under bring-up, not release-qualified.

The transport owns one caller-selected vendor interface (`ff/06/50`); it does
not detach or claim OS-managed class `08` storage. On any uncertain command,
retire the session and reconnect. A command status is not a durability claim:
filesystem flush must reach SCSI SYNCHRONIZE CACHE and its successful CSW.

Live persist provisioning requires an independently retained backup before
writable access. Full-partition flashing is not a fallback in this workspace.

`nusb-scsi` owns USB/BOT once. `nusb-fatfs` and `nusb-ext4` compose a synchronous
block callback supplied by a filesystem worker; the separate async broker must
remain unblocked. Browser qualification and independent Linux oracles live in
`qualification/browser`; these memory-broker cases do not establish USB coverage.

## Browser fastboot facade

`canoe-usb-web` builds with `wasm-bindgen --target web` and exports
`requestFastboot()` (user gesture), `openFastboot(USBDevice)` and
`FastbootSession`: `getVar`, `command`, bounded `fetch`, `flash`, `hashRange`,
`usable`, and awaited `close`. Offsets/lengths represented by Rust `u64` use
JavaScript BigInt. An error retires the session and closes the selected device.
The application supplies review, backup, readback, capability and retry policy.

`hashRange` uses CANOE-BDS `sha256-range-v1` and decodes the 43-character
base64url digest to 32 bytes. Generic upstream code contains no CANOE commands.

All consumers should carry this workspace's `patch.crates-io.nusb` pin if
they integrate these crates into a larger Rust workspace.
