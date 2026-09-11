# Linux oracle for the production filesystem worker

This storage-owned integration check uses the built production broker and WASM
engine, a disposable 128 MiB ext4 image with 1 KiB blocks, uninitialized block
groups and legacy CRC16 group-descriptor checksums. It creates a 32 MiB file,
renames it, finishes the filesystem, then checks the result with Linux `e2fsck`
and `debugfs`. Exact extracted bytes and an unrelated file must be preserved.
It opens no USB device and uses no phone captures.

```sh
bash browser/build.sh
bun qualification/linux-oracle/run.ts .work/browser-storage
```

An explicit engine directory can qualify packaged consumer bytes without
building another engine:

```sh
bun qualification/linux-oracle/run.ts /absolute/path/to/dist/hosted/engine release
```

Results, worker SHA-256, `e2fsck` output and browser errors are retained under
`.work/linux-oracle/`. `CANOE_EXT4_FIXTURE_ROOT` can select another evidence root.

This is the former CBM `e2e/qualify-ext4-worker.ts` filesystem oracle. Its important
reserved-GDT allocation failure belongs here and in the ext4 driver's
`src/alloc.rs` and `tests/gdt_csum_writes.rs`; it must not become a CBM filesystem
rule. CBM retains its small adapter/API/error/ownership integration in its
existing hosting check. Other FAT/ext4/USB fault combinations stay in the
existing production-worker qualification, rather than being repeated here.
