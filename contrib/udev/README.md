# Linux managed USB access

The managed export uses USB ID `1209:ca0f`. Its vendor interface (`ff/06/50`)
does not bind Linux's mass-storage driver, so it has no OS block device.
Native nusb and browser WebUSB still need permission to open its USB device
node. Existing Android fastboot rules do not necessarily cover this ID.

`70-canoe-managed-usb.rules` grants access through systemd-logind to the active
local user. It matches only the managed export, leaving manual `1209:ca0e`
mass storage and unrelated devices unchanged. The `70-` filename puts the
`uaccess` tag before `73-seat-late.rules` applies the access list. It does not
grant all users access, detach a driver or mount a filesystem.

To install on a system with udev and logind, run from the repository root:

```sh
sudo install -m 0644 contrib/udev/70-canoe-managed-usb.rules /etc/udev/rules.d/70-canoe-managed-usb.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --action=change --subsystem-match=usb --attr-match=idVendor=1209 --attr-match=idProduct=ca0f --settle
```

The scoped trigger updates permission on an already enumerated managed export;
it does not reset or reconnect USB. A later enumeration also applies the rule.
Run the application or [read-only probe](../../nusb-scsi/examples/README.md)
as the local user after setup. Browser permission to select the device is a
separate requirement.

For a temporary qualification run, an operator can install the same file into
`/run/udev/rules.d/` instead of `/etc/udev/rules.d/`. Do not leave a second copy
under a different filename. After ending the export and removing the temporary
rule, reload rules; the temporary file also disappears at host reboot.

This active-session policy does not grant access to arbitrary SSH users or
headless services. Those environments need their own narrowly scoped device
access policy. Do not broaden this rule to every Android device or use global
world-writable USB permissions to bypass that distinction.

Rule syntax can be checked without touching a device:

```sh
udevadm verify contrib/udev/70-canoe-managed-usb.rules
```
