# Journal lifecycle is a separate release gate

GDT_CSUM qualification does not qualify journal recovery or physical writes.
The copied OP15 capture had actual pending journal work before the experiment:
needs_recovery=true, JBD2 start3270, sequence55183, errno0, orphan head0.
After the library mounted and wrote the disposable copy: JBD2 start0,
sequence55218; needs_recovery remained true. Independent `e2fsck -fn` exited0
and warned that read-only checking skips journal recovery. This proves neither
that the original had only a cosmetic flag nor that interruption is recoverable.
No private image, pathname inventory or file content is published.

The driver owns journal parsing and replay. Current writable mount applies
journal records before opening its writer. Its replay path does not itself
finalize the journal cursor, and the mount's superblock/group snapshots were
loaded before replay. Orphan recovery errors are discarded by upstream mount.
These transitions need dedicated dependency changes, crash-point reproductions,
and independent Linux recovery before live managed persist writes are enabled.
Do not bypass them by clearing bits or flashing an edited full persist image.

`nusb-ext4::inspect` mounts a read-only wrapper first. Its writable `mount`
refuses recovery-marked volumes, pending/aborted journals and orphan chains
before granting the upstream writable mount any access. A synthetic regression
asserts zero writes on the recovery-marked preflight path. This is an explicit
bring-up barrier pending dependency lifecycle qualification, not an instruction
to format phone data. Valid clean synthetic filesystems still exercise GDT_CSUM.

Remaining gates: replay finalization, metadata snapshot refresh, orphan-error
propagation, checksum-validated journal input, directory-growth rename atomicity,
and failure at every write/flush boundary. Then repeat through actual USB with
fresh device reads, independently retained backup, and verified ownership.
