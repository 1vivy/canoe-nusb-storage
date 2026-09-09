
## Real Linux USB host-controller fixture (2026-09-09)

A separate Ubuntu 24.04 guest (6.8.0-138-generic) with Chromium
141.0.7390.37, QEMU 11.1.0 and the harness's managed descriptor extension
exercised real WebUSB, the production WASM facade and the production SAB FAT
worker. `navigator.usb` was not replaced. The native chooser granted only the
1209:ca0f ff/06/50 synthetic disk. The ordinary 1209:ca0e class08 fixture
remained an OS USB block device, while the managed interface had no block node.

The first attempt exposed a normal SCSI UNIT ATTENTION after reset. Independent
REQUEST SENSE returned `70 00 06 00 00 00 00 0a 00 00 00 00 29 00 00 00 00 00`.
New `RangeSession` initialization consumes only current fixed-format reset or
medium-change sense and retries TEST UNIT READY once, before reading geometry.
Unknown/deferred sense and repeated failure still stop; no data command or
transport uncertainty is retried. `nusb-scsi` has no separate upstream (it is
part of this repository), so this fix is maintained here.

After that fix the guest browser passed bounded reads, all six filesystem
read-only mutation refusals, three fresh close/reopen cycles, FAT file creation
and rename, fresh readback, SYNCHRONIZE CACHE, eject and awaited close. A native
`fsck.fat -n` independently accepted the resulting synthetic disk. The fixture
is backed by a host file; it qualifies the real guest OS USB/WebUSB path but
**does not qualify physical-phone USB or Windows WinUSB binding**. Exact build
hashes and the reusable virtual-device recipe live in `canoe-harnesses`'s
`qualification/managed-webusb` record. CDP DeviceAccess did not intercept the
USB chooser in this Chromium version; the fixture used its native X11 chooser.
