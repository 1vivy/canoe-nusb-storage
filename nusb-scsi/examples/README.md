# Managed USB read-only qualification

Build with `cargo build --locked -p nusb-scsi --example managed_probe`.
An operator who owns the device can then run:

```sh
timeout 40s target/debug/examples/managed_probe
```

Use `--bus BUS_ID --address ADDRESS` together to identify one export when needed.
The probe always requires exactly one matching `1209:ca0f` device and an active
`ff/06/50` interface. It never chooses the manual `1209:ca0e` export, changes a
configuration/alternate setting, detaches a kernel driver, resets USB, mounts a
filesystem or issues SCSI WRITE commands.

Opening `NusbIo` first sends the interface GET_MAX_LUN control request and
validates its reply before making BOT available. This is also required to arm
the current managed device's first CBW receive. A valid maximum LUN is 0–15;
the standard single-LUN STALL response is accepted as LUN0. The selected LUN
cannot exceed the reported maximum. Native initialization waits for nusb's
five-second control timeout and its completion instead of abandoning EP0.

The actual `nusb-scsi` BOT implementation then executes INQUIRY, TEST UNIT READY,
READ CAPACITY(10), a read of at most 4096 bytes from LBA0, SYNCHRONIZE CACHE and
START STOP UNIT eject. Each command has a five-second deadline. A framed SCSI
failure may be followed by REQUEST SENSE; uncertain framing retires the session.
The probe stops instead of retrying.

JSON Lines record stages, selected USB bus/address, geometry, sample size and
SHA256, initialization, flush/eject results and teardown. Sample contents are never printed.
Teardown cancels/drains endpoint transfers with a bounded wait, then explicitly
waits for interface release. Native nusb closes the device through final handle
drop and provides no separate awaited device-close API; evidence distinguishes
those two events. A failed or timed-out stage exits nonzero.

Compiling or passing the protocol unit tests is not evidence of physical USB
coverage. Only an operator's device run qualifies that particular transport,
firmware build and export target. Eject is expected to end the managed export.

The shared browser constructor is `NusbIo::connect_web`: it owns the raw
user-granted USBDevice, checks the active alternate setting, and applies an
independent deadline because nusb's WebUSB control timeout is not implemented.
On failure it drops endpoint owners, explicitly awaits interface release, then
awaits device.close() before returning. Its consuming close uses the same
ordering and propagates release or close rejection. Dropping an interface alone
starts an unawaited release and races device close in Chromium. An external caller
cancelling the constructor must close its USBDevice explicitly; dropping a
Rust future cannot abort a WebUSB promise. The managed-browser qualification runs this ordering against a delayed-release
WebUSB fake in Chromium. The native device probe does not qualify physical
browser behavior.
