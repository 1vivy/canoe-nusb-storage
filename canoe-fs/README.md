# Confined filesystem composition

`canoe-fs` puts bounded, named operations over `nusb-fatfs` and `nusb-ext4`.
The native qualification tests and browser WASM worker execute this same code.
It receives a `BlockDevice`; it does not discover, open or claim a USB device.

Read-only access is enforced at the filesystem API and block callback. In
particular, inspection never replays an ext4 journal, and FAT32's in-memory
FSInfo updates from free-space scanning are discarded on read-only finish.
Writable ext4 mount uses the dependency's checked plain-JBD2 recovery and
finish lifecycle. Its supported feature envelope and backup prerequisite are
documented in `qualification/journal-lifecycle.md` at the repository root.

Paths are absolute within the mounted filesystem. Components are bounded,
and traversal, empty components, control characters, backslashes, FAT-invalid
characters and trailing dot/space aliases are rejected before I/O. Ext4 path
resolution does not follow symlinks; multiply linked files cannot be written
or truncated. FAT names are compared with the same Unicode uppercase behavior
as the underlying engine for exclusive creation.

File chunks are capped at 4 MiB. Directory listings are capped at 4096 entries
and 16 MiB of ext4 directory blocks; unsupported inline directories are refused.
`create_file` is exclusive; `write` requires an existing regular file;
`truncate` only shrinks; `rename` requires an absent destination; `remove`
does not recursively delete. There is no implicit overwrite, stage adoption,
directory migration or wildcard cleanup. The application owns those choices
and their review/receipt policy.

Any block-I/O failure retires the guarded device. A failed mutation also
retires the session, preventing destructor-time retries or more device I/O.
Finish consumes the mount and propagates flush failures. Abort/drop refuses
further I/O and does not claim a clean filesystem release. A successful finish
is not an application readback receipt; verify through a fresh read-only open.

`cargo test -p canoe-fs` uses disposable Linux-created FAT/ext4 fixtures only.
The browser worker tests also execute committed journal recovery and run
independent Linux filesystem checkers; they do not establish physical USB,
power-loss atomicity, or Windows/macOS/browser-device qualification.
