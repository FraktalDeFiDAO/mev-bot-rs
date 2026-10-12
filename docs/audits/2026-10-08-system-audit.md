# System Audit — mev-bot

**Date:** 2026-10-08
**Audited at:** local repo `ef664de` (#89, `feat(decode): standalone mev-decode crate`); latest deploy tag `deploy-20261008-154831` → `da182d5` (#87)
**Production:** `mev-ohio` (AWS us-east-2, t3.small), live mode, hot wallet ~0.005 ETH
**Infra:** Arbitrum Nitro node deleted 2026-10-05 (volumes, SG, IAM role removed; $0/mo EBS)
**Scope:** full sweep — secrets/security, MEV correctness & fund safety, architecture/invariants, test coverage, ops & runtime, config/docs drift
**Method:** static review of all execution-path crates + contracts, invariant compliance checks, `cargo audit`, container/CI/deploy-script review, git history secret scan, local runtime inspection. Read-only; no code changed.

---

## Verdict

**No Critical findings.** Posture is unusually disciplined for a solo operation: secrets never touched git, container and deploy gates are strict, the executor contract has real on-chain guards and 60+ Foundry tests, and the two HIGH code risks (f64 math, hot-path RPC) are known, documented in the project's own audit (`docs/design/audit-sequencer-first.md` §6), and scheduled behind the sequencer-first phases. Remaining exposures are gas-bounded, not principal-bounded.

---

## Severity summary

| Sev | ID | Finding |
|---|---|---|
| HIGH | H1 | Gas-price fallback + 1×base profit floor under-covers 2×base+tip inclusion cost |
| HIGH | H2 | f64 in every profit/gas/gate decision (invariant #1 violated, known/accepted) |
| HIGH | H3 | Hot-path RPC calls remain: pre-send sim, heal, per-tx presim (invariant #4, knowingly retained) |
| MED | M1 | Nonce resync can double-send an already-broadcast trade (narrow, self-healing) |
| MED | M2 | Sim gate (`minProfit=0`) vs send floor mismatch; floor → 0 when `loan_px ≤ 0` |
| MED | M3 | localsim zero-fills missing state; balance/nonce checks off → ≤3 s phantom-price window |
| MED | M4 | Decoder: 3 selectors/3 routers only; UR/1inch/Camelot/EntryPoint undecoded — gates DECISIONS #89 |
| MED | M5 | Deploy drift: main = #89, latest deploy tag = #87 |
| MED | M6 | `docs/knowledge/` 211 tracked files constantly dirty → manual stash before every deploy |
| MED | M7 | Contabo-era EBS volumes in the *other* AWS account unconfirmed deleted |
| LOW | L1 | Pool-claim TTL (30 s) == receipt wait (30 s) → self-race window |
| LOW | L2 | Presim trigger verification runs on `latest` — races the block (guarded) |
| LOW | L3 | Receipt-timeout nonce reset can lag a slow tx (self-healing) |
| LOW | L4 | `_v4Swap` delta decode truncates >2²⁸⁸ magnitudes (unreachable at bot sizes; mis-account, not drain) |
| LOW | L5 | Liquidation opportunities carry `versions: vec![]`; single staleness re-check site (pre-send only) |
| LOW | L6 | `state_correction` reasons are ad-hoc strings, no taxonomy |
| LOW | L7 | Pool-level busy marking persists (invariant #6, Wave 2 pending) |
| LOW | L8 | `.env` clutter: 7 retired `EXECUTOR_ADDRESS` comment lines |

---

## HIGH findings

### H1 — Gas floor undercoverage during congestion
`crates/mev-engine/src/lib.rs:2347-2352` — `fetch_gas_price` silently falls back to a fixed **0.01 gwei** on RPC error. Every profit decision (`net_usd`), the on-chain `min_profit` floor, and the bid's `max_fee_per_gas` derive from this value. On error, or when the 10 s cache (`lib.rs:866-889`) goes stale during a base-fee spike, the floor under-covers actual inclusion cost — a landed Arbitrum tx pays up to `2×base + tip` while the floor only encodes `base + tip`. Additionally `min_profit` is computed at 1× base (`lib.rs:1662-1666`) while the bid policy authorizes up to `2×base` headroom (`mev-risk/src/bid.rs:119`): **the floor never covers the headroom the bot itself bids**. Loss is bounded to gas per tx, but it is systematic under exactly the congested conditions where backruns matter most.
**Fix direction:** fail-closed (skip sends) when gas fetch errors rather than falling back to a constant; compute the on-chain floor at `2×base + tip` to match worst-case inclusion.

### H2 — f64 throughout profit decisions (invariant #1)
Non-test f64 usage: `mev-dex` 49, `mev-search` 25, `mev-risk/bid.rs` 30, `mev-engine` 33, `mev-state` 21, lending crates ~180. `PoolState` is all-`f64` (`mev-dex/src/lib.rs:10-18`), `est_profit`/`est_profit_usd` are `f64` (`mev-types/src/lib.rs:100-102`), and they feed sort/select (`engine/lib.rs:781,1439`) and the cost gate (`cost_usd = gas as f64 * tip as f64 * native_px` → `gate.check` `:1602`, `risk.check_arb` `:1617`). Known and accepted in the project's own audit; remediation is sequencer-first Phases 4/7/8 (integer pricing). Recorded because a "known violation" is still a live risk until then.

### H3 — Hot-path RPC (invariant #4)
Two of four causes fixed (async metrics via `sync_channel` + writer thread; maintenance spawned off-loop). Remaining in the hot path: pre-send `eth_simulateV1` (`engine/lib.rs:1508`) and `eth_call` (`:1513,1535`), `heal_route` (`:1558→:982`), and per-candidate presim RPC with out-of-order result application (`mev-presim/src/lib.rs:404-415`). Knowingly retained (removing it re-opens phantom-send losses; blocked on Phases 2-5/7-9).

---

## MEDIUM findings

### M1 — Nonce-resync double-send
`crates/mev-exec/src/lib.rs:181-192` + `is_nonce_error` (`:305-311`) classifies "already known" as a nonce error. If the first `sign_and_broadcast` fails on **all** endpoints with "already known" (tx already in mempool), the nonce is released (`:245-253`), resync+retry then signs identical calldata on a fresh nonce → duplicate submission; the second copy reverts on `minProfit` and burns gas+tip. Narrow trigger, self-healing after, but a live double-send path.
**Fix direction:** on all-endpoints-"already known", treat the tx as sent (hold the nonce / skip retry), not as a nonce failure.

### M2 — Simulation/send floor mismatch
Simulation verifies with `minProfit = 0` (`mev-sim/src/lib.rs:69-78, 94-102`; `engine/lib.rs:1507, 1535, 1967`); the send uses `minProfit = gas + tip`. A route whose simulated profit sits between 0 and the floor passes sim and reverts on-chain. Mostly closed by the bid-budget invariant (`tip ≤ net - min_net`, `bid.rs:90-93`) — except PGA-disabled (tip 0, trivially safe) and **`loan_px ≤ 0`, where the floor degrades to zero** (`engine/lib.rs:1662-1666, 1913-1917, 2033-2037`): an unpriced token sends with no on-chain floor. Blocked in practice only by the earlier `net_usd < min_profit_usd` check — an indirect guard chain.
**Fix direction:** simulate with the same floor the send will use; hard-fail sends when `loan_px ≤ 0` instead of degrading the floor.

### M3 — localsim zero-filled state
`crates/mev-localsim/src/lib.rs:125-186, 229-236` — missing state executes as zero-filled, and balance/nonce/base-fee checks are disabled. A pending tx that would fail on-chain can "succeed" locally and be applied as tentative pool state (`Presimulated`). Mitigations are real (`complete` flag gates emission; `TENTATIVE_TTL` 3 s rollback; version-based `superseded`), so residual exposure is a ≤3 s window of trading on a phantom price — gas, not principal.

### M4 — Decoder coverage gates the RPC-replacement plan
`crates/mev-decode/src/swap.rs:28-32`: 3 selectors on 3 routers; the `CONTRACTS` table (`lib.rs:43-84`) lists Universal Router, V3 router v1, 1inch, Camelot router, EntryPoint as `decoded: false`. Today decode feeds only shadow telemetry, so gaps **hide opportunities, not misprice trades** — the live state path runs full simulation. But this is the planned RPC-replacement path (DECISIONS #89): switching the feed trigger to decode before Universal Router / 1inch coverage would blind the engine to a large share of pool-moving transactions. Malformed-but-matching calldata is silently skipped (`swap.rs:91-93`).
**Also noted:** when the tick window doesn't reach the limit, `decode_check.rs:240-254` falls back to the amount-based move and misreports a "differ" — telemetry quality only (path never mutates state).

### M5 — Deploy drift
Latest deploy tag `deploy-20261008-154831` → `da182d5` (#87). Main is `ef664de` (#89): the outcomes/decoder-coverage work (#88) and the standalone `mev-decode` crate (#89) are **not deployed**. Deliberate or not, the live host and main have diverged 2 commits — reconcile (deploy #89 or record why not).

### M6 — Generated docs block the deploy gate
`docs/knowledge/` has **211 uncommitted tracked files** (regenerated Obsidian projection, never hand-edited per CLAUDE.md). `scripts/deploy.sh` requires a clean tracked tree, so every deploy needs a manual `git stash` of `docs/knowledge/` first (STATE.md documents the dance). This is friction plus a mistake surface on the exact script that ships code to a funds-signing host.
**Fix direction:** stop tracking `docs/knowledge/` (gitignore it, like `graphify-out/`); publish it elsewhere if needed.

### M7 — Other-account EBS volumes unconfirmed
STATE.md open item: the stopped Contabo-era Nitro volumes live in an AWS account not visible from profile `mev-bot`; deletion unconfirmed. If still provisioned, they are a silent monthly charge.
**Action:** user confirms deletion in that account.

---

## LOW findings

| ID | Detail | Evidence |
|---|---|---|
| L1 | In-flight pool claim TTL (30 s) equals the receipt wait (30 s); a >30 s receipt frees pools while the tx may still land → self-race, gas-only | `engine/lib.rs:95, 1720` |
| L2 | Presim trigger+arb verification on `latest`; txs sequenced between presim and send invisible (guarded by `already_included` + floor) | `mev-presim/src/lib.rs:140-180, 92-94` |
| L3 | `wait()` receipt-timeout nonce reset can lag a slow tx → "already known" → resync loop; self-healing, no brick | `mev-exec/src/lib.rs:284-292` |
| L4 | `_v4Swap` delta decode truncates >2²⁸⁸ magnitudes silently; unreachable at bot sizes; mis-account, not drain (flash accounting forces balance) | `contracts/src/ArbExecutor.sol:534-538` |
| L5 | Liquidation opportunities carry `versions: vec![]`; the single staleness re-check is pre-send only — the audit doc claims "before simulation and before send" (doc drift) | `engine/lib.rs:1595-1601, 1905, 2024` |
| L6 | `state_correction` recorded but reasons are ad-hoc strings (`"resync"`); no taxonomy / first-divergence | `mev-state/src/lib.rs:131-148`; `engine/lib.rs:1288-1306` |
| L7 | Pool-level busy marking persists (own-tx special-casing, invariant #6 Wave 2 pending) | `engine/lib.rs:157-158, 1627, 1633, 902-913` |
| L8 | `.env` accumulates history: 7 retired `EXECUTOR_ADDRESS` comment lines; prune (they are on-chain anyway, but hygiene) | `.env` |

Doc-drift extras: PROGRESS names `mev-feedlead`; no such crate — it is `crates/mev-ingest/src/lead.rs`. Invariant #7 (degraded-state confidence) has **zero implementation** (`rg Confidence` → no hits) and is untestable today. Knowledge graph is 2 commits stale (built from #88-era `aedffbc5`) — run `scripts/graph.sh optimize` at convenience.

---

## Invariant compliance (sequencer-first)

| # | Invariant | Status |
|---|---|---|
| 1 | Integer math only | **VIOLATED** (H2) |
| 2 | Chain wins + `StateCorrection` | PARTIAL — recorded, no reason taxonomy (L6) |
| 3 | Opportunities carry state versions | PARTIAL — interim counters; liquidations unversioned; one re-check site (L5) |
| 4 | Hot-path purity | **VIOLATED** (H3) — 2 of 4 causes fixed |
| 5 | `EXECUTION_MODE` defaults shadow; agent never sets live | **COMPLIANT** — clap default, compose default, run.sh, engine early-return, cap-hit auto-switch (`main.rs:24`, `compose.yaml:17`, `engine:918-929,957`) |
| 6 | No special-casing own txs | PARTIAL — busy marking (L7) |
| 7 | Degraded state contagious | **UNIMPLEMENTED** |
| 8 | No stubs reported done | **COMPLIANT** — zero `todo!/unimplemented!`; enforced by `scripts/check.sh:10` |

---

## Test coverage

Per-crate `#[test]`/proptest counts: `mev-risk` 30 · `mev-engine` 14 · `mev-exec` 12 · `mev-decode` 10 · `mev-presim` 9 · `mev-sequencer` 9 · `mev-sim` 4 · `mev-localsim` 4 · **`mev-outcomes` 1** (197-line lib whose sampler PROGRESS lists as done).

**Covered well:** executor callbacks + floor (60+ Foundry tests), nonce manager (unit + concurrency), bid policy math (+ `pga_replay` backtest), state apply/rollback, `already_included` guard.

**Untested, in risk order:**
1. `fast_send` flag combination bypassing presim (`engine/lib.rs:1481-1492`) — the one path where model error converts directly into on-chain gas loss.
2. PGA engine wiring (`tip_budget.record` before send, TipBook interplay) — policy tested, wiring not.
3. The M1 all-endpoints-"already-known" double-send edge.
4. Engine presim fallback branch (`simulate_after` → `simulate_fast`, `engine/lib.rs:1505-1534`) — needs a node; worth an integration test.

---

## What passed (keep as-is)

- **Secrets:** `.env` never in git history (verified), 0600, gitignored with `!.env.example`; keys env-only, never baked into images; tests use `PrivateKeySigner::random()`; pre-commit blocks `.env` files, 64-hex strings, known RPC-key URL patterns + gitleaks; Nitro used SSM-scoped parameters under `/mev-bot/arbitrum/`.
- **Supply chain:** `cargo audit` clean — only 2 allowed unmaintained-transitive warnings (`derivative` 2.2.0, `paste` 1.0.15). CI: `clippy -D warnings` + `cargo test --locked` on PR/push, pinned action versions, least-privilege `permissions`. Hooks: conventional-commit enforcement, direct-main-push block (free-plan ruleset substitute).
- **Container:** read-only rootfs, `cap_drop ALL`, `no-new-privileges`, non-root + `userns keep-id`, `pids_limit`, tmpfs `/tmp`, pinned-digest base, `--locked` release build, heartbeat healthcheck, host networking documented as a latency tradeoff.
- **Deploy:** main-only + clean-tree + `== origin/main` gates, `check.sh` in dev container, `--yes` required for live, read-only deploy key on host, old-container log archival.
- **Executor contract:** owner-gating, transient callback guards, exact approvals, on-chain `minProfit` floor; nonce manager gap-free by construction; rounding directions consistently conservative (`min_profit` truncates down, bid floors, competition ceils).
- **Robustness:** zero `unsafe`, zero `todo!/unreachable!`, 23 total non-test unwraps (all guarded lock/length cases), locks never held across `.await`, blocking I/O quarantined (writer thread + `spawn_blocking`).
- **Mode safety & ops:** shadow default everywhere; cap-hit auto-fallback to shadow; STOP kill switch; `watchdog.sh` (container, heartbeat, kill switch, wallet gas, disk, cap events, webhook); `SECURITY.md` exposure playbook (pause → sweep → rotate → redeploy).
- **Infra teardown:** Nitro deletion complete and evidenced (`arbitrum-node-deleted.json`): instance terminated, 3 volumes deleted, SG + IAM role/profile removed, $0 EBS; provisioning scripts deliberately preserved locally.
- **Rounding audit:** the only rounding that favors taking a trade is the `minProfit` floor losing <1 raw unit — negligible.

---

## Blocked / not verified

1. **Live `mev-ohio` runtime** — AWS session expired at audit time. After `aws login`, run (read-only):
   - `ssh mev-ohio 'systemctl list-timers | grep -i watchdog; crontab -l'` — is `watchdog.sh` actually scheduled?
   - `ssh mev-ohio 'podman ps --filter name=mev-bot-arbitrum; df -h /; tail -20 ~/src/mev-bot/data/arbitrum/ALERTS.log 2>/dev/null; cat ~/src/mev-bot/data/arbitrum/heartbeat 2>/dev/null'`
   - `aws --profile mev-bot --region us-east-2 ec2 describe-instances --instance-ids i-0acc917f9a4d5f83b --query 'Reservations[].Instances[].State.Name'`
2. **Live-log-derived claims** ("93% decode shadow-match", e2e timings, ~180 sends / 0 wins) — not re-derivable read-only; spot-check via `scripts/report.sh` on the host.
3. **PnL verdicts** — out of scope; noted only that STATE.md records net negative PnL with tips losing to faster bidders (structural, not a bug).

---

## Recommended remediation order

1. **H1** — fail-closed on gas-fetch error; floor at `2×base + tip`. Small diff, direct PnL impact.
2. **M7** — confirm other-account EBS deletion (money leaking, if any).
3. **M1** — hold nonce on all-endpoints-"already-known"; add the regression test.
4. **M2** — sim floor == send floor; hard-fail `loan_px ≤ 0`.
5. **M5** — deploy #89 or record the pin reason.
6. **M6** — untrack `docs/knowledge/`; end the stash dance.
7. **Coverage** — the four untested paths above, starting with `fast_send`.
8. Then the known roadmap items (H2/H3/M4) via sequencer-first phases — already sequenced in `docs/tasks/`.
