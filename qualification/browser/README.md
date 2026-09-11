# Historical browser feasibility evidence

The JSON files in `evidence/` record the original b5 feasibility experiments,
including the early clock/PID and legacy group-descriptor checksum findings.
They describe the sources and binaries tested at that time, not current release
qualification.

The experimental runner, copied fork sources and separate probe WASM crate have
been retired. They built old filesystem revisions and duplicated the now-shipped
worker interface. The maintained replacements are:

- [`qualification/filesystem-browser`](../filesystem-browser/README.md): current
  production broker/WASM filesystem operations, fault handling, recovery and
  independent FAT/ext4 checks.
- [`qualification/linux-oracle`](../linux-oracle/README.md): the 1 KiB block / CRC16
  group-descriptor allocation regression and independent payload extraction.
- [`qualification/managed-browser`](../managed-browser/run.ts): actual nusb/BOT
  composition with a synthetic USB target.

The old runnable sources remain recoverable from Git at
`7bc4aaf2f840d55addb59d8ec33d19ce8bffdbec`. No historical evidence was deleted.
