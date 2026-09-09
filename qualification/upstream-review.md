# Dependency integration review

- am-fs-core: native FileDevice gating reproduced red on wasm32, native suite and WASM green. Upstream draft: https://github.com/antimatter-studios/rust-fs-core/pull/69 .
- am-fs-ext4: injected mount Runtime, preserved native defaults and browser providers. Native baseline 845 tests passes; resulting suite 846 passes, clippy and wasm32 check pass. Upstream draft: https://github.com/christhomas/rust-fs-ext4/pull/144 .
- fastboot-rs: generic command/fetch extension, DATA/OUT completion lengths and finite buffer accounting. Native suite and wasm32 pass; actual USB and cancellation qualification pending. Upstream draft: https://github.com/boardswarm/fastboot-rs/pull/40 .
- rust-fatfs: PR120 (80c4807) and PR121 (1e15bdf) integrated with original attribution; changelog-only conflict reconciled. Full native suite passes, including their Unicode and moved-directory regressions. No duplicate PR opened.
- nusb: PR221 (4e664c7) inspected but not integrated. Its malformed-descriptor branch calls dec_and_maybe_close both inside and outside the construction result; it also cannot await Drop's close before reopen. The browser facade retains its USBDevice and awaits close explicitly. Native/real WebUSB lifetime qualification remains required.

No private persist image or physical device was used or published. The driver GDT_CSUM write guard remains active until its separate implementation and independent checks pass.
