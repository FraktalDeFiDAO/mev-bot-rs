# JEV-LIQ-001 — storage and operations inventory

Audited 2026-10-13. Authoritative baseline: `/tmp/mev-jev-liq-001-audit`, HEAD `c6f3aa466fb1230424a0dfb41b65e09b5084d72e`, branch `docs/jev-liq-001-audit`. All unqualified source references below belong to this checkout. Separate branch/WIP references are explicitly identified. Read-only audit: no trading behavior, repository source, secrets, services, or host controls changed; no compilation or test execution by this auditor.

Read the complete 623-line architecture, 32-line CLI handoff, CLAUDE.md, standing invariants, planning index and relevant progress/source ranges. Queried the shared navigation graph with `--budget 800`; its recorded commit matched c6f3aa4, but source facts were confirmed in the assigned checkout.

## Workspace and executables

`Cargo.toml:1-9` uses resolver 3, `members = ["crates/*"]`, edition 2024, workspace version 0.1.0. `cargo metadata --no-deps --locked --offline --format-version 1` succeeded and reported **58 packages / 16 executable binaries**, without compiling. No `go.mod` exists in this checkout; the Rust/Go wording in the handoff is an integration prompt, not evidence of a Go subsystem. `CLAUDE.md:7-9` requires Rust off-chain and Solidity/Foundry for the executor.

Every package below has the exact manifest `crates/<package-name>/Cargo.toml:1`; workspace path dependencies are enumerated at `Cargo.toml:34-90` (the bot itself is included through the wildcard):

```text
mev-analytics, mev-atlas, mev-bindings, mev-bot, mev-config, mev-decode,
mev-dex, mev-discovery, mev-engine, mev-exec, mev-ingest, mev-lending,
mev-lending-aave-v3, mev-lending-compound-v3, mev-lending-dolomite,
mev-lending-fluid, mev-lending-gearbox, mev-lending-index, mev-lending-morpho,
mev-lending-radiant, mev-lending-silo, mev-lending-spark, mev-liquidate,
mev-localsim, mev-metrics, mev-oracle, mev-outcomes, mev-presim, mev-protocol,
mev-protocol-aave-v3, mev-protocol-balancer, mev-protocol-camelot,
mev-protocol-compound-v3, mev-protocol-cow, mev-protocol-dolomite,
mev-protocol-fluid, mev-protocol-kyber, mev-protocol-morpho-blue,
mev-protocol-oneinch, mev-protocol-pancake-v3, mev-protocol-paraswap,
mev-protocol-sushi, mev-protocol-unidentified, mev-protocol-uniswap-router,
mev-protocol-uniswap-v2, mev-protocol-uniswap-v3, mev-protocol-uniswap-v4,
mev-risk, mev-rpc, mev-search, mev-sequencer, mev-sim, mev-state, mev-types,
seqscope-cli, seqscope-core, seqscope-server, seqscope-store
```

| Binary | Exact source entry point |
|---|---|
| mev-bot | `crates/mev-bot/src/main.rs:30` |
| seqscope | `crates/seqscope-cli/src/main.rs:1`; explicit target `crates/seqscope-cli/Cargo.toml:9-11` |
| mev-decode | `crates/mev-decode/src/main.rs:1` |
| mev-accounting | `crates/mev-analytics/src/bin/mev-accounting.rs:1` |
| mev-analyze | `crates/mev-analytics/src/bin/mev-analyze.rs:1` |
| mev-feedlead | `crates/mev-analytics/src/bin/mev-feedlead.rs:1` |
| mev-ledger-seed | `crates/mev-analytics/src/bin/mev-ledger-seed.rs:61` |
| mev-liq-audit | `crates/mev-analytics/src/bin/mev-liq-audit.rs:1` |
| mev-pnl | `crates/mev-analytics/src/bin/mev-pnl.rs:1` |
| mev-redact | `crates/mev-analytics/src/bin/mev-redact.rs:1` |
| mev-scan | `crates/mev-analytics/src/bin/mev-scan.rs:1` |
| mev-atlas-check | `crates/mev-atlas/src/bin/mev-atlas-check.rs:1` |
| mev-feed-probe | `crates/mev-sequencer/src/bin/mev-feed-probe.rs:1` |
| mev-diff-probe | `crates/mev-localsim/src/bin/mev-diff-probe.rs:1` |
| mev-localsim-probe | `crates/mev-localsim/src/bin/mev-localsim-probe.rs:1` |
| mev-presim-probe | `crates/mev-presim/src/bin/mev-presim-probe.rs:1` |

## Locked versions and tooling pins

Manifest requirements are ranges, not exact pins (`Cargo.toml:11-32`); preserve the checked-in lockfile. REVM requirement is `43` in `crates/mev-localsim/Cargo.toml:17`. Exact relevant resolved versions:

| Dependency | Cargo.lock name line / locked version |
|---|---|
| alloy / alloy-primitives | `33`: 2.5.0 / `352`: 1.7.3 |
| revm / revm-database / revm-interpreter | `4567`: 43.0.3 / `4631`: 43.0.1 / `4698`: 43.0.2 |
| tokio / futures / async-trait | `5712`: 1.53.2 / `2255`: 0.3.34 / `1198`: 0.1.92 |
| serde / serde_json | `5233`: 1.0.229 / `5263`: 1.0.151 |
| rusqlite / libsqlite3-sys | `4849`: 0.40.2 / `3031`: 0.38.2 |
| redb | `4475`: 4.3.0 — dev dependency of seqscope-store, not its production store |
| reqwest | `4530`: 0.13.5 — present transitively; no baseline Jev client |
| anyhow / thiserror | `801`: 1.0.104 / `5628`: 2.0.21 |
| tracing / tracing-subscriber | `5904`: 0.1.44 / `5948`: 0.3.23 |
| clap / chrono / dotenvy | `1590`: 4.6.7 / `1576`: 0.4.45 / `2028`: 0.15.7 |
| toml / axum / tower | `5807`: 1.1.6+spec-1.1.0 / `1276`: 0.8.9 / `5858`: 0.5.3 |
| proptest / tempfile | `4260`: 1.11.0 / `5615`: 3.27.0 |
| base64 | `1337`: 0.22.1 and `1343`: 0.23.1 |
| tokio-tungstenite | `5762`: 0.29.0 and `5778`: 0.30.0 |

No sqlx/PostgreSQL client, Redis/Kafka/NATS dependency, standalone migration directory, or PostgreSQL service was found in the baseline. `redb` is confined to `crates/seqscope-store/Cargo.toml:16-18` and benchmark `examples/bench.rs:186`; production uses rusqlite (`Cargo.toml:9-14`).

CI Rust is **1.99.0** (`.github/workflows/ci.yml:38-41`); Foundry **v1.8.4** (`59-61`); Solidity **0.8.28**, Cancun EVM, optimizer 1,000,000 runs (`contracts/foundry.toml:5-9`). forge-std gitlink is **c6fa5d82a3a287c4ff23f26c8f7e2958aafd32d9** (`git ls-tree HEAD contracts/lib/forge-std`; URL `.gitmodules:1-3`). Both Containerfiles pin Arch image digest **sha256:bf88f8143c3ef21b14d0ebb8140099f0bf75ecfc3699f357ff92317a73f5aba9**, but `pacman -Syu` selects mutable package/toolchain versions (`Containerfile:2-5,15-22`, `Containerfile.dev:2-5`). No rust-toolchain file. The dev image pins Foundry installer source commit **50af4efe189dc64bad2b75ed6990b835de66c4ae** and installed version v1.8.4 (`Containerfile.dev:12-16`).

## Existing persistence, checkpoints, and permissions

| Owner | Persisted data and source seam | Durability / migration / isolation |
|---|---|---|
| mev-risk ledger | integer micro-dollar spend, signed pending liability/hash/nonce, wallet+chain identity and reconciliation block; `crates/mev-risk/src/ledger.rs:13-60,133-160,264-305` | Version 2 only; legacy/missing live state fails closed. Engine requires existing live ledger, isolates shadow under `shadow-accounting` (`crates/mev-engine/src/lib.rs:513-542`). Durable attach must finish before broadcast (`ledger.rs:547-570`). |
| mev-risk Store | `risk-state.json`, `gate-state.json`; engine owners at `mev-engine/src/lib.rs:529-536,864-868` | Exclusive sibling .lock file; 0600 creation; temp-write, fsync file, rename, fsync directory; storage/read/ownership error latches unhealthy (`mev-risk/src/persist.rs:21-45,60-72,82-111`). Uses synchronous filesystem operations; cannot be reused as hot-path nonblocking advisor transport. Existing files are not chmod-repaired by creation modes. |
| Ledger migration tool | canonical receipt-backed initialization, `mev-ledger-seed` | Candidate only, never overwrites existing output/signs/broadcasts (`mev-analytics/src/bin/mev-ledger-seed.rs:1-8,69-83`); `Ledger::initialize` requires complete nonce receipt range (`ledger.rs:136-160`). Not an automatic schema upgrade. |
| Lending EventIndex | account set + forward `next_block` / backward `back_block`; `mev-lending-index/src/lib.rs:42-69,94-129` | JSON cache; unreadable/corrupt cache resets to default; plain truncating `fs::write` (`187-192`), no fsync/atomic rename/explicit file mode. Old address-keyed representation translated (`196-215`). Block numbers only, not hash-bound canonical checkpoints. |
| Aave borrower cache | `data/<chain>/aave_v3_borrowers.json`, similarly compound/morpho (`mev-engine/src/lib.rs:3057-3069`) | Aave v1 cache migration occurs before EventIndex open (`mev-lending-aave-v3/src/lib.rs:81-90,534-547`); it writes in place. This is discovery state, not complete immutable borrower snapshots. |
| V4 discovery Registry | `v4_pools.json`: last scanned Initialize block and pools (`mev-engine/src/lib.rs:427`; `mev-discovery/src/v4.rs:42-63`) | Tolerant read; temp+rename, but no file/directory fsync or explicit 0600 mode. |
| Metrics | daily `events-YYYY-MM-DD.jsonl`, in-memory counters, heartbeat (`mev-metrics/src/lib.rs:20-58,82-98,119-142,150-220`) | Bounded 16,384-entry writer channel; full queue drops/counts; async thread append/flush; JSONL mode stamp and secret redaction; filesystem permissions rely on directory/umask. Diagnostics, not a transactional event source/checkpoint. |
| Feed/outcome recorders | `feedtx`, `chaintx`, and other prefix daily JSONL (`mev-presim/src/record.rs:1-5,60-115`; `mev-outcomes/src/lib.rs:1-8`) | Background thread, 16,384 queue, drop counter, periodic flush; retention by prefix and date (`record.rs:38-57,123-130`); not durable lease/ack transport. |
| seqscope-store | decoded transaction rows in SQLite; schema inline (`crates/seqscope-store/src/lib.rs:31,43-94`) | WAL, synchronous=NORMAL, user_version=1; newer schema rejected. Batch transaction (`118-155`); bounded writer/drop/failure counters (`158-180`); read-only/query_only reader with SQL limits (`387-401,498-545`); prune checkpoints WAL (`556-561`). No PostgreSQL roles or durable borrower/outbox interface. |
| Runtime state | pool store/version cache and local REVM state are in-memory owners | They must not be mistaken for durable borrower revisions. New Jev storage must attach to the actual canonical state/index owner rather than treat diagnostic JSONL or seqscope rows as authoritative snapshots. |

## Metrics and execution controls

Existing instrumentation is `tracing`, JSONL event records and counters, feed/provider request meters and periodic engine summaries (`mev-metrics/src/lib.rs:20-58,121-145`; `mev-engine/src/lib.rs:975,2062-2105`). No Prometheus exporter or OpenTelemetry dependency/instrumentation found in baseline source/manifests. The named `copper_*` metrics and asynchronous linked spans in architecture `397-419` are requirements, not shipped monitoring. The heartbeat is written through the metrics writer and used for container health (`mev-metrics/src/lib.rs:136-142,194-197`).

CLI defaults to shadow (`mev-bot/src/main.rs:23-24`), loads only `.env.shadow`, and uses `Secrets::read_only` (`41-58`); key lookup is gated by signing=false (`mev-config/src/lib.rs:699-721`). We inspected source, never these files/values. Compose default is `EXECUTION_MODE:-shadow` (`compose.yaml:17-19`); runtime image default also shadow (`Containerfile:32-33`). `scripts/run.sh:38-48` builds a 0600 allowlisted endpoint-only shadow env file without sourcing .env; live startup requires keys, explicit human confirmation and an absent STOP file (`59-68`), and resume separately asks (`75-81`). Shadow accounting is separate; production ledger ownership/schema/reconciliation failures prevent admission.

## Containers and CI/test seams

Podman is the project standard (`CLAUDE.md:9`). `scripts/dev.sh:9-19` builds the dev image if missing; rootless keep-id, host networking, source bind mount plus persistent cargo/target volumes, only HTTP/WS variables passed through the helper. `scripts/run.sh:19-20` explicitly chooses podman-compose. Runtime Compose has host networking, keep-id UID/GID, read-only config, writable data mount, read-only rootfs, no-new-privileges, all caps dropped, pids 128, 16m tmpfs, restart always and heartbeat healthcheck (`compose.yaml:13-43`); image runs unprivileged and creates its state directory 0700 (`Containerfile:25-32`). No separate advisor container, advisor DB credentials/roles, network egress allowlist, or resource limit for advisor exists. Docker equivalence is **unverified**: helper and user namespace settings are Podman-specific and CI has no Docker/Podman parity job.

CI runs on PRs and main pushes, read-only contents permissions; Rust fmt, locked workspace/all-targets clippy and locked workspace tests, plus Python audit-guard tests (`.github/workflows/ci.yml:3-17,32-46`). It does **not** request all features. Foundry has separate build/fmt/network-free and live-RPC fork test steps (`48-69`). Browser job uses Node major 22 and `scripts/test-seqscope-ui.sh`, which itself builds/serves through Podman (`84-94`; script `7-9`). Gitleaks is a separate job (`71-82`); `ci-ok` depends on rust/contracts/secrets/browser and rejects failure/cancellation (`96-106`). No advisor HTTP/database/container failure-injection job exists.

Pinned CI action commits (`ci.yml`): checkout `11d5960a326750d5838078e36cf38b85af677262`; rust-toolchain `686976e191b89faba57d3206551f0f330d8cb249`; rust-cache `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`; foundry-toolchain `908c540300062bd5a7e473851cdb4282204cee09`; gitleaks `ff98106e4c7b2bc287b24eaf42907196329070c7`; setup-node `49933ea5288caeca8642d1e84afbd3f7d6820020`. PR title uses mutable `amannn/action-semantic-pull-request@v5` (`.github/workflows/pr-title.yml:15`). Ubuntu runner and Node patch are unpinned.

Reproducible baseline entry points (all listed shell harnesses verified present/executable, mode 755; parent owns observed run results):

```bash
scripts/dev.sh bash -c 'cargo fmt --all --check && cargo clippy -q --workspace --all-targets --locked -- -D warnings && cargo test -q --workspace --locked'
scripts/dev.sh python3 -m unittest discover -s scripts/tests -v
scripts/dev.sh bash -c "cd contracts && forge fmt --check && forge build --sizes && forge test --no-match-path 'test/{ArbExecutor,UniswapV4,FastFeedTicketVault}.t.sol'"
# Network-dependent, excluded from ordinary Rust baseline:
scripts/dev.sh scripts/test-integration.sh
scripts/dev.sh bash -c "cd contracts && forge test --match-path 'test/{ArbExecutor,UniswapV4,FastFeedTicketVault}.t.sol'"
```

The Foundry dependency needs `git submodule update --init --recursive` before reproduction; this is initialization, not proof of fork results. The exact parent Rust run succeeded (fmt/clippy/tests; 564 passed, 0 failed, 24 ignored, parent-reported). This auditor did not rerun it. `scripts/check.sh:8-16` is a local alternative but omits --locked and its production-stub search differs from CI. Unit/property/default integration coverage includes Rust unit modules, seqscope SQLite/API/CLI/corpus suites and Python `scripts/tests/test_audit_guards.py`. Network-ignored adapter tests: Aave (`tests/live.rs:31,90,163`), Comet (`22,66`), Morpho (`27,74`), Fluid (`20`); harness selects `--ignored --test-threads=1` (`scripts/test-integration.sh:7-8`). Ordinary cargo tests therefore do not prove deployed Aave ABI/fork validity. Foundry offline suites are ArbExecutorUnit, AtlasSolver and Batch; fork suites are ArbExecutor, UniswapV4 and FastFeedTicketVault (`ci.yml:64-69`). Historical block pinning and current provider availability are separate evidence needs. E2E/crash harnesses launch live-mode test bots against chain-id-31337 Anvil forks (`docs/planning/TESTING.md:12-13,28-30`; `scripts/test-e2e-crash.sh:8-13,48-52`); none executed here.

## Committed liquidation stack, WIPs, and unresolved persistence decision

Separate committed stack: `feat/liq-scheduler` HEAD **d0f7b52ff841e96c9f251bd4cf51469e222d9324**, rooted on main **968d580b59d7a63338ee9cdf572a848835d3cad1**, with spec `acbcb41`, deployment `91cca68`, snapshots `7113cf8`, event index `a6fc04b`, features `830a03e`, scheduler `d0f7b52`. Its audit source checkout is `/tmp/claude-1000/-home-admministrator-projects-mev-bot/407a3e0e-218e-4d2d-8d04-529438221647/scratchpad/wt-stack`. These are **not present in the pinned c6 baseline**, and this inventory does not accept their task completion/merge status.

The stack adds `mev-liq-state`, `mev-liq-features`, `mev-liq-sched` (61 packages), using SQLite snapshots/reverse dependencies/gaps/history: `crates/mev-liq-state/src/repo.rs:23-70,300-344` schema versions 1–3, WAL/FULL, IMMEDIATE transaction migration; `rollback_to:347-356` destroys rolled-back table data. No explicit POSIX mode/DB roles in `open`; deployment permissions remain an operations seam. `crates/mev-liq-state/src/index.rs:704-707` atomically applies scan effects/cursor/gap bookkeeping. `ShadowSchedule` is disabled by default, single-flight spawn_blocking, records only, and can attach to Runner (`crates/mev-liquidate/src/shadow.rs:3-6,28-48,68-82`; `lib.rs:213-217`). Engine does **not** configure/attach that scheduler or `AaveV3Adapter::attach_triggers`, and Runner still uses legacy timers: confirmed source searches and explicit limitation `docs/design/jev-liq-005-scheduler.md:43-58`. Deployment verification is engine-wired (`mev-engine/src/lib.rs:3085-3092`), but libraries alone do not establish production canonical indexing, mandatory dispatch, or Jev isolation.

Current relevant worktree observations (read-only; may change after audit):

| Branch/head | Non-graph state and significance |
|---|---|
| feat/liq-scheduler d0f7b52 | Source clean; generated graph/knowledge modifications only. |
| feat/liq-outbox d0f7b52 | **Uncommitted WIP** modifies `mev-liq-state/src/lib.rs`, `repo.rs`, `tests/history.rs`; untracked `src/advisor.rs`, `tests/advisor.rs`, `docs/design/jev-liq-008-outbox.md`. |
| feat/liq-jev-client d0f7b52 | **Uncommitted WIP** Cargo.toml/Cargo.lock modifications and untracked `crates/jev-advisor/`; no accepted client/service. |
| feat/liq-baseline-replay cc4484480ea1 | Committed branch beyond scheduler; source clean. Historical/live four-arm evaluation acceptance is not established by its branch name or synthetic artifacts. |
| jev-liq-003 4fecbf35dc20; jev-liq-005 9d8321653658; jev-liq-007 f7a36889c9bd | Separate clean branches on c6 ancestry, respectively +2/+4/+4 commits; do not equate them with the main-based stack. |

Git ancestry counts: c6 versus main 66/6 commits; c6 versus d0f7b52 66/12. Squash/rebased histories mean ancestry alone says nothing about missing audit fixes. Exact full `git diff --name-only c6f3aa466fb1230424a0dfb41b65e09b5084d72e 968d580b59d7a63338ee9cdf572a848835d3cad1` was confirmed: five source files (`crates/mev-decode/src/lib.rs`, `crates/mev-engine/src/lib.rs`, `crates/mev-engine/src/race.rs`, `crates/mev-protocol-fluid/src/lib.rs`, `crates/mev-sim/src/lib.rs`) and four docs (`docs/copper_jev_cli_handoff.md`, `docs/copper_jev_liquidation_architecture.md`, `docs/design/ecosystem-coverage.md`, `docs/design/jev-liq-001-inventory.md`). No broad missing-fix claim is made.

**Unresolved design decision:** authoritative spec requires PostgreSQL 16+ as checkpoint/revision/evaluation source of truth, transactional state+enqueue, SKIP LOCKED consumers, and separate DB roles (`docs/copper_jev_liquidation_architecture.md:73-74,265-353,494`). Existing repo has no Postgres. Committed snapshots already use SQLite, and outbox WIP explicitly calls itself a SQLite translation. In the outbox checkout `/home/admministrator/projects/mev-bot/.claude/worktrees/agent-a4abef5fe3dc12126`, `docs/design/jev-liq-008-outbox.md:5-36` proposes two DB files: snapshot-owned export+current/shed tables; advisor-owned queue/decisions/outcomes. It explicitly acknowledges lack of SQLite roles, no cross-WAL-file atomicity, and non-atomic decision CAS versus snapshot read, compensated by rechecking revision/digest when retrieving decisions. WIP `advisor.rs:287-292,668-676` uses read-only snapshot connection and separate writable WAL/FULL advisor DB; `repo.rs:23,71-75` adds schema v4. These are implementation proposals, **not accepted PostgreSQL design or task-008 completion evidence**. No explicit spec amendment/sign-off, signed migration/rollback plan, advisor permissions/container integration or PostgreSQL contention evidence was found. Resolve persistence and isolation contract before treating 008 as dependency-ready/accepted.

Unknowns: actual rootless host/subuid configuration, built-image toolchain versions, Docker compatibility, running deployment revision, runtime file modes, storage backups/retention recovery, fork/provider outcomes, Jev API contract/results, workload/latency/capacity gates and production scheduler wiring. This read-only inventory does not validate any of them.
