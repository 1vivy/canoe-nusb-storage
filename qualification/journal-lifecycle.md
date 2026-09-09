# Journal recovery and release

Android's `needs_recovery` / ext4 RECOVER flag is expected for a journaled
filesystem that was mounted. It is not by itself filesystem damage, a request
to run fsck manually, or a reason to format phone data. The ext4 dependency now
owns recovery for its qualified plain-JBD2 format.

`nusb-ext4::inspect` always uses read-only access and never replays. Its writable
`mount` calls `Filesystem::mount_recovering`; opening this handle can replay
committed transactions before the first file operation. Therefore the application
must save and independently reopen its mandatory backup, bind the raw persist
source, and acquire exclusive ownership **before writable mount**. `finish`
flushes the filesystem and clears RECOVER only after the journal is checkpointed
and the orphan chain is empty. Drop does not claim a clean release.

Supported: internal JBD2 v2, matching block size, first log block 1, one user,
compatibility/read-only flags zero, only REVOKE/64BIT incompatibility flags,
no outstanding journal or filesystem error. A pending cursor or orphan chain
enters dependency recovery. Unsupported orphan layouts propagate an error rather
than silently dropping the list. Checksummed, async-commit, fast-commit and unknown
journal formats remain refused **even with a clean cursor**; old successful
clean-image browser tests did not exercise interrupted checksummed transactions.
Ext4 metadata checksums/GDT_CSUM are separate from journal transaction checksums.

The upstream fix is [draft PR146](https://github.com/christhomas/rust-fs-ext4/pull/146),
integrated at `8315da024dc352408fe9f005ee5618ac8c4ab29f`. Replay retains only committed
writes and revokes, compares wrapping transaction IDs correctly, validates the
complete address plan before any replay write, flushes data before finalizing the
cursor, and refreshes superblock/group snapshots. Recovery errors propagate.
Failed journal writers retire; finish performs no further device I/O on a known
failed writer. The final clean-marker flush can leave its durability unknown, but
both clean and dirty cursor states are recoverable because data was flushed first.

Independent Linux qualification uses mkfs/debugfs and raw-encoded journal records,
not the driver's transaction encoder. Committed inode/superblock changes plus
uncommitted tail writes/revokes agree with e2fsck. Finished images pass e2fsck -fn
without skipping recovery, and repeat mount/finish is byte-stable. All 18 injected
interruptions (9 write/flush boundaries in each of two persistence models) recover
through independent e2fsck. Models cover immediately durable writes and writes
lost since the last successful flush; they do not model torn sectors or dishonest
flush acknowledgements. Native upstream baseline845 grows to851 tests; combined
runtime/GDT/recovery integration passes856 tests.

The existing 128 MiB private Android capture was tested only through disposable
copies. Its pending journal was real, not a cosmetic flag. Checked recovery and
Linux recovery both finished at journal start0/sequence55185 with RECOVER cleared.
Whole-image comparison found no unexplained filesystem-payload differences: ten
bytes differed only in filesystem accounting timestamps/counters and the journal
header sequence. Both finished copies passed e2fsck; the original capture stayed
unchanged. No private image, inventory or file contents are published.

This qualifies expected plain-JBD2 recovery on copied images. Actual USB/WinUSB,
exclusive ownership, device flush behavior, browser interruption/resume, and
publication operations such as directory-growing rename remain separate release
gates. App device writes remain disabled. No whole-persist flash fallback, application-side
journal bit clearing, automatic fsck or physical-phone operation is introduced.
