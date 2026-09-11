# Working in canoe-nusb-storage

## Test ownership

Before adding a test, identify the production contract, owning repository and smallest suitable test layer. Reuse existing coverage. Do not encode subjective acceptance, incidental presentation or the current implementation as requirements. Do not add regression tests for every reversible edit by default. Use human or agent exploration for usability assessment, and report its evidence separately.

The existing case inventory and ownership decisions are recorded in [docs/testing-audit.json](docs/testing-audit.json). Filesystem, USB and byte-format contracts stay with their implementation owner; application consumers should check their own integration and policy.
