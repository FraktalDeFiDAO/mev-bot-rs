# Full system audit — 2026-10-08

**Audited at:** `main` = `da182d5` (1 commit ahead of latest deploy tag `deploy-20261008-151603`).
**Method:** five parallel deep-dives (security, financial risk controls, invariant compliance, infra/ops, code health) + a deployment-state pass, each verifying claims against source with file:line evidence. Load-bearing config claims were independently re-verified.
**Limitations:** the live host (`mev-ohio`) is unreachable via SSH from the dev box, so *runtime* state (container, wallet balance, live event logs) was not verified — run `scripts/report.sh` on/at the host. `cargo audit` could not fetch the RustSec DB (offline); advisory status unknown. Provision/teardown scripts (35–41 KB each) were sampled, not fully read.

---

## Severity rollup

| Severity | Count | Headline items |
|---|---|---|
| CRITICAL | 3 | restart wipes all daily caps; f64 money math incl. float-derived on-chain `minProfit`; root↔`infra/` script drift with a real bug in the *tracked* copy |
| HIGH | 6 | live Alchemy key in copied artifacts (rotation pending); `fast_send` removes exact-model safety nets; on-chain floor `= 0` on missing price; PGA tip wei computed in f64; `mev-bindings`→`mev-lending` upward dep; 244 untracked files |
| MEDIUM | 14 | redaction URL/stdout-only; secondary bins bypass redaction; watchdog unscheduled; `resume`/`live` STOP-file handling; unbounded backruns; timeout-tx accounting; `heal_route` inline RPC; config assert *forces* `fast_send`; state JSONs unignored; node auto-start machinery; teardown residue; engine god-module; hot-path unwraps; `mev-bot` 0 tests |
| LOW/INFO | 15+ | see domain sections |

Worst-case financial exposure today: any single process run ≈ **$28/day** ($3 gas + $25 tips); a crash-restart loop defeats the caps but is bounded by the hot wallet (~0.005 ETH ≈ $15). Principal is never at risk (flash-loan repayment enforced on-chain).

---

## Mitigations already applied during this audit

- `chmod 600 .env.bak-1791224080` (was 0644, contains the live PRIVATE_KEY).
- `chmod 600` on the four artifacts containing the raw Alchemy key: `data/arbitrum/events-2026-10-03.jsonl`, `data/server-arbitrum/events-2026-10-03.jsonl`, `data/audit-contabo/events-2026-10-03.jsonl`, `data/shadow-presim.log`.

Both are stopgaps — see actions 1–2.

## Top actions (ordered)

1. **Rotate the Alchemy key** and shred/purge the four artifacts above + `.env.bak-1791224080` (`shred -u`). The `audit-contabo`/`audit-ohio` dirs imply copies left this host — inventory them. Consider hot-wallet rotation given the key sat world-readable since Oct 5.
2. **Persist daily-cap state across restarts** (`mev-risk`): rehydrate `spent_usd`/tip-day/halt flags from `data/arbitrum/events-*.jsonl` at startup (or persist on change); make `run.sh live` refuse to delete a STOP file younger than N hours; give `resume` the same y/N confirmation `live` has. — *fixes the CRITICAL restart-wipe*
3. **Sharpen `minProfit` under `fast_send`** (small diff, big edge): skip the send when `loan_px <= 0` instead of sending with floor 0; derive the floor from `gas_limit` (2.5× est), not the estimate (`crates/mev-engine/src/lib.rs:1645-1652`, `batch.rs:72`).
4. **Reconcile root ↔ `infra/` node scripts**: port the root fixes (IMDSv2 token/region fix, snapshot guards, `--wait-for-quota`) into tracked `infra/arbitrum-node/`, commit the untracked `infra/ethereum-node/` + 8 identical files, delete root strays. The tracked `infra/arbitrum-node/nitro-start.sh:16-28` currently stores the IMDS *token* in `$region` → SSM fetch always fails; `bootstrap` installs from whichever dir it's run from.
5. **Schedule the watchdog** (timer/cron, 2–5 min) and let it (or the bot via `eth_balance`, exists at `mev-exec/src/lib.rs:87`) create the STOP file on low balance. Today a dead watchdog = silence.
6. **Un-force `fast_send`**: `mev-config/src/lib.rs:719-720` *asserts* it on — invert to warn; and exempt `AlgebraV3` hops from fast_send (self-declared "never exact", `mev-dex/src/lib.rs:166-171`).
7. **Shared redaction init for all binaries** (`mev-scan`, `mev-analyze`, `mev-feedlead`, `mev-liq-audit`, `mev-presim-probe`) — this leak class produced action 1. Extend `redact_secrets()` to standalone 64-hex tokens; route panics through the redacting writer.
8. **Integer math stack** (roadmap, invariant #1): f64 pool state (`mev-state/src/lib.rs:500-508`) flows float into every price, the PGA tip wei (`mev-risk/src/bid.rs:83-97`), and the on-chain `minProfit` floor (`engine/lib.rs:1646`). Not patchable in place — planned `mev-amm`/`mev-quote` integer crates are the fix.
9. **Housekeeping**: `.gitignore` root `arbitrum-*.json`, `arbitrum-node-resources.env*`, `__pycache__/`; decide commit-vs-ignore for `docs/knowledge/` (kills the pre-deploy stash dance); hoist the `revm` pin into workspace deps; run `cargo audit` where network exists; `scripts/graph.sh optimize` (graph is stale at `89021b15`).

---

# Domain reports

## 1. Security

**Verified good:** contracts clean on full read — `ArbExecutor.sol` owner-gated everywhere, single-use source-checked callbacks, exact-amount approvals reset to zero, `rescue` only to immutable `profitReceiver`, holds no funds at rest; `AtlasSolver` bid-capped with `maxBid` default 0; `FastFeedTicketVault` bounded permissionless `renew()` + 2-step ownership. Git history clean of the live key and Alchemy key (full-history scan). Compose/runtime hardening real: `read_only`, `cap_drop: ALL`, `no-new-privileges`, non-root UID, `pids_limit: 128`, digest-pinned images, no listening sockets in bot code.

- **HIGH — live Alchemy key unredacted in `data/` artifacts** (mode 644 → now 600), predating the redaction fix `d4138e0` (2026-10-04); copied between ≥2 hosts; never rotated. → action 1.
- **HIGH — `.env.bak-1791224080` 0644 with live PRIVATE_KEY** (byte-identical to `.env`; now 600). → action 1.
- **MEDIUM — redaction is URL-only and stdout-only** (`mev-types/src/lib.rs:118` `redact_secrets()` needs `://`; bare 0x-64-hex passes; `Redacting` wraps only tracing stdout — `panic!`/`eprintln!` bypass).
- **MEDIUM — secondary binaries load the real `.env` with no redaction at all** (`mev-scan.rs`, `mev-analyze.rs`, `mev-feedlead.rs`, `mev-liq-audit.rs`; `mev-presim-probe.rs:107` prints the RPC URL). Root cause of the leak class. → action 7.
- **LOW — shadow containers receive the live PRIVATE_KEY** (`compose.yaml` `env_file: .env` unconditional). Per-mode env overlay would fix.
- **LOW — `arbitrum-node-resources.env` + state JSONs at root untracked & not ignored** (infra metadata, no key material). → action 9.
- **LOW — dev supply chain**: `Containerfile.dev` `curl | bash` unpinned foundryup; runtime `pacman -Syu` floats.
- **INFO — ops scripts pass the key via argv** (`deploy-executor.sh:31,42`, `atlas-solver.sh:27,48`, `allow-targets.sh:18`); `fast-feed-ticket.sh` shows the better keystore pattern.

## 2. Financial risk controls

**Verified good:** tip/bid invariants enforced **at send time**, not just tests — `BidPolicy::decide` clamps `budget = min(fraction, hard cap, net − min_net)` (`mev-risk/src/bid.rs:90-93`), called from the live path (`engine/lib.rs:1099`); per-route tip books can only *skip*, never overbid; STOP file checked before every send and survives restarts; lost races do not advance the failure pause (by design). Caps: $3/day gas, $1/tx tip + ≤50% of net, $5/min and $25/day tips, 3 concurrent arb sends, $0.05 min profit, on-chain `minProfit` reverts losers.

- **CRITICAL — every daily cap is in-memory only** (`RiskGuard::new` zeroes `spent_usd`/tips/in-flight/halt flags, `mev-risk/src/lib.rs:48-59`; no persistence or rehydration anywhere). With `restart: always` + `panic="abort"` a crash loop resets caps each cycle, and `run.sh live` deletes the STOP file (`scripts/run.sh:49`). Backstops: wallet balance + node insufficient-funds (10 → `Paused`). → action 2.
- **HIGH — `fast_send = true` (live, `config/arbitrum.toml:105`, DECISIONS #88) skips every pre-send verification** (`arb_presimulate`, inexact-route, unvetted-route, V4 checks — `engine/lib.rs:1469-1475`); profit = local f64 model; no `eth_estimateGas` on the send path. Sharp edges: floor from *estimated* gas while the tx may burn 2.5× (`lib.rs:1652`); `minProfit = 0` when `loan_px <= 0` (`lib.rs:1645-1649`). → actions 3, 6.
- **MEDIUM — `rollback_unconfirmed = false` (live, DECISIONS #86) leaves tentative pool state unversioned** (`mev-state/lib.rs:423-458` — no restore, no version bump, so `superseded()` can't flag it) until a real log or ≤60 s resync; under fast_send a phantom edge goes out and only the on-chain revert stops it. Within a run: bounded by caps ($28/day). Across restarts: unbounded (CRITICAL above).
- **MEDIUM — unbounded concurrent backruns**: `check_concurrent` bypasses the in-flight limit (`mev-risk/src/lib.rs:102-104`); nothing counts outstanding backruns. Add `max_concurrent_backruns`.
- **MEDIUM — receipt-timeout txs never accounted** (`mev-exec/src/lib.rs:284-292` reports `gas_usd = 0`; the tx can still land on Arbitrum) → caps undercount. Reconcile timed-out hashes at rollover.
- **LOW — L1 data fee likely not in `gas_usd`** (`gas_used × effective_gas_price` only, `engine/lib.rs:2081-2082`) — cap math and on-chain floor both understate true cost slightly.
- **INFO — win-rate gate `cost_usd` uses `default_tip_wei`, not the actual bid** (`engine/lib.rs:1577`) — EV misestimate only.

## 3. Correctness & invariant compliance (matrix re-verified at HEAD)

Matrix vs 2026-10-07: **#3 version-tagged opps now COMPLIANT (interim)** (stamped `engine/lib.rs:1427`, rechecked `:1578`, `batch.rs:131`; `set_ticks` bumps version); #5 shadow-default and #8 no-stubs remain COMPLIANT; #2 chain-state-wins and #4 hot-path purity remain PARTIAL (I/O causes genuinely fixed; one new inline path found); #6 own-txs PARTIAL (unchanged, awaiting Phase 3); **#1 integer math and #7 degraded-contagious remain VIOLATED** (Wave 2 not landed — zero matches for confidence/degraded/exclusion in engine/state/search/config).

- **CRITICAL — invariant #1: pool state itself is f64** (`mev-state/src/lib.rs:500-508`); ~49 float sites in mev-dex, 34 engine, 30 risk, 25 search, 21 state. All money math downstream is float. → action 8.
- **HIGH — the on-chain `minProfit` guard is float-derived** (`engine/lib.rs:1646`, `batch.rs:72` — f64 USD math over f64 pool prices; sub-unit truncation rounds the floor *down*, weakening it). Same root as action 3/8.
- **HIGH — PGA tip (wire value) computed in f64** (`mev-risk/src/bid.rs:83-97` — `floor() as u128`, `ceil() as u128` on the submitted wei). → action 8.
- **HIGH — fast_send × known-inexact models**: routes flagged "never exact" (Algebra, `mev-dex/lib.rs:166-171`, live mispriced reverts 2026-10-05) send on the f64 model alone; only backstop is the float-derived minProfit. → actions 3, 6.
- **MEDIUM — config validation *forces* `fast_send`** (`mev-config/src/lib.rs:719-720` asserts it on) — fail-safe direction inverted. → action 6.
- **MEDIUM — `heal_route` awaits RPC inline in the event loop** (`engine/lib.rs:990` from decision path `:1541`) — a hot-path-purity violation the matrix missed; route through the `Maint` pattern.
- **MEDIUM — tick-map reload silently overwrites state** (`mev-state/lib.rs:225-233`, `engine/lib.rs:1255-1260`) — version-bumps (good) but emits no `state_correction` record, unlike resync/rollback/heal.
- **LOW — first-ever `gas_price()` blocks the loop** (`engine/lib.rs:852-855`) — prefetch at startup. **LOW — V4 quoter-equivalence test is `#[ignore = "network"]`** and `check.sh` never runs it. **LOW — V4 pools get no presim tentative updates** (`presim/lib.rs:56-60` lacks `IPoolManager::Swap` while confirmed ingest has it) — latency/coverage, not correctness.
- **INFO — latent double-apply landmine**: presim applies only absolute-state events today, so #75's fix holds; but `apply_log` (`state/lib.rs:397-403`) drops the tentative snapshot without restoring — if the log filter ever widens to deltas (`+=` applies at `:276-296`) it double-applies. Guard comment + test.
- **Regression scan #75–#87**: all live-path behavior changes are test-covered or flag-only (#80); #81–#86 are shadow/diagnostic or off-path. No untested live-path change beyond #80's flag.
- **Mispricing can reach a live send today** (fast_send + float floor); the shadow decode_check + `state_correction` telemetry are the detection net — keep them on.

## 4. Infra & ops

**Verified good:** compose hardening claims TRUE (`compose.yaml:29-42`); deploy gates (main-only, clean, ==origin, `check.sh`, `--yes` for live, server-side dirty refusal, log gz rotation); CI solid (fmt/clippy `--locked -D warnings`/tests + forge incl. fuzz + gitleaks, `permissions: contents: read`, fork-safe); watchdog read-only by design (no false-positive spend path).

- **CRITICAL — root ↔ `infra/` drift**: 9/18 shared files differ; root copies carry fixes the tracked ones lack; tracked `infra/arbitrum-node/nitro-start.sh:16-28` has the IMDS token/region bug; `bootstrap-arbitrum-node.sh:110-122` installs from `$(dirname $0)`. → action 4.
- **HIGH — 244 untracked files** (197 in `docs/knowledge/`, all root node ops files, `infra/ethereum-node/` 13 files, `.noticks.toml`, `.scan.sh`) → deploy-refusal stash dance + no provenance. → actions 4, 9.
- **MEDIUM — watchdog has no scheduler anywhere** — alerts fire only when someone runs `report.sh`. → action 5. **MEDIUM — webhook alerts fire-and-forget, no dedup/cooldown** (`watchdog.sh:70-71`). **MEDIUM — `run.sh resume` re-enables live trading with zero confirmation** (`run.sh:57`). **MEDIUM — bootstrap can re-enable auto-start of the deleted paid node** (enables nitro + activate/health timers; `Restart=on-failure` + `StartLimitIntervalSec=0` + 2-min timer; safe today — all units stopped/disabled, instance terminated). **MEDIUM — teardown deletes only instance + opt-in volume**; key pair/VPC/IAM/SSM remain ($0 but clutter); same in ethereum-node. **MEDIUM — state JSONs/env at root not gitignored** (`.gitignore:16` covers only the infra path).
- **LOW — no container log cap / ALERTS.log rotation; `Containerfile.dev` curl|bash; CI path-filter job is a no-op + actions pinned by tag not SHA; `nitro-node.service` runs as root, TasksMax=infinity** (moot while deleted).
- **Cost leaks:** no new ones beyond the known other-account EBS volumes (user confirming deletion). Only EBS ever bills (~$490/mo for 6 TiB gp3 if it existed).

## 5. Code health & dependencies

**Verified good:** `cargo check`/`clippy -D warnings` workspace-wide clean; **zero `unsafe`**; deps healthy (rustls-only, no OpenSSL, no git/wildcard pins, no version skew; alloy 2.5.0, tokio 1.53.2, revm 43.0.3 single copies); 261 tests, all run suites pass (`mev-risk` 29/29, `mev-engine` 14/14); the graph-reported compound-v3 self-import cycle is **not real**.

- **HIGH — layering inversion**: `mev-bindings` (L0) depends on `mev-lending` (L2) (`crates/mev-bindings/Cargo.toml`). Move adapter-facing types down or extract an L0 trait crate.
- **MEDIUM — hot-path `lock().unwrap()`s are only safe because `panic="abort"`** kills the process before poisoning (11 in `mev-engine/src/lib.rs` event loop, 3 in `mev-exec/src/nonce.rs`, 3 in `mev-presim`) — silent reliance; use `parking_lot` or `unwrap_or_else(|e| e.into_inner())`.
- **MEDIUM — `mev-engine/src/lib.rs` ≈ 2,500 lines** (event loop, tips, presim wiring, batching, maintenance) — extract private mods.
- **MEDIUM — `mev-bot` (live binary) has 0 tests** — add config-parse + smoke boot test.
- **MEDIUM — CRATES.md drift**: `mev-presim`/`mev-localsim` absent; `mev-oracle` listed at two tiers; discovery→state upward dep is doc-sanctioned but rule-breaking.
- **LOW — `revm` pinned outside workspace deps** (`crates/mev-localsim/Cargo.toml:17`); six 1-test lending crates would benefit from a shared `LiquidationAdapter` conformance test.

## 6. Deployment & live state (this audit's own pass)

- This machine is the **dev box**, not the live host; `mev-ohio` SSH times out from here → live runtime state unverified in this audit.
- Deploy drift minimal: HEAD is exactly **1 commit** ahead of `deploy-20261008-151603`; 5 deploys on Oct 8 alone.
- Contracts: 932 lines Solidity / 69 test functions across `ArbExecutor`, `AtlasSolver`, `FastFeedTicketVault`.
- Known PnL context (STATE.md): ~180 landed sends, 0 wins, ~$4–5 gas, net negative — accepted; not re-audited here (event logs live on the host).
- Graph stale (`89021b15` vs `da182d5`) — run `scripts/graph.sh optimize`.
