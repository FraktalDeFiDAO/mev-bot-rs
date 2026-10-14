# Audited task-001 baseline evidence

Source revision: `c6f3aa466fb1230424a0dfb41b65e09b5084d72e`, clean isolated checkout.
Date: 2026-10-13. This inventory changes documentation only.

[Repository map and dependency sequence](../../../design/jev-liq-001-audited-map.md)
contains actual owners, callgraphs, integration gaps and reproducible commands.

- [Verification](verification.json): observed results, toolchains and excluded checks.
- [Manifest](manifest.json): SHA-256 hashes of archived audits and logs.
- [Feeds/state audit](reports/feeds-state.md): source ownership, provenance and canonicality.
- [Lending/execution audit](reports/lending-execution.md): discovery, exactness, scheduler,
  REVM versus eth_call, final-call mismatch and preserved signing/risk guards.
- [Storage/operations audit](reports/storage-ops.md): 58 packages/16 binaries, lockfile,
  persistence, CI, containers and unfinished branch status.
- [Rust log](logs/rust.log): fmt/clippy success followed by 564 passing tests / 24 ignored.
- [Foundry log](logs/foundry.log): format/build and 27 passing offline tests.
- [Browser log](logs/browser.log): 23 logic + 17 actual browser tests and clean shutdown.

Python tests passed 3/3; Podman Compose parsed successfully without starting services.
The host Docker command is a Podman alias, so no actual Docker parity is claimed.
These results do not validate current deployment ABI/rules, historical economic labels,
REVM liquidation parity, live scheduler coverage, Jev quality or production readiness.
No live signing, production credentials, funding or behavioral changes occurred.
