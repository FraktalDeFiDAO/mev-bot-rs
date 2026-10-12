# MEV bot audit — 11 October 2026

## Decision

**The system is not ready for unrestricted live operation.** The principal risks are incorrect accounting of pending exposure and earned profit, unsafe transaction reconciliation, stale pricing state, and deployment drift. Passing the existing tests does not resolve these defects.

This report records **27 findings: 10 high, 13 medium, and 4 low**. No critical unauthenticated fund-theft exploit was established. Two high findings concern the optional Atlas bidding path, which is disabled in both inspected configurations. Findings distinguish observed behavior, reproducible defects, and conclusions from source inspection.

Recommended immediate operator action: hold live arbitrage while reconciling outstanding transactions and deploying reviewed accounting fixes. Keep Atlas bidding and batching disabled until their findings are addressed. **This audit did not stop, restart, deploy, fund, or change the live bot.**

## Scope and evidence

- Local revision: `08d80455b8e1a16cc6fb4d7416ac24a4c4d0fa56`.
- Reviewed surfaces: Rust workspace, discovery/state/search/simulation, ingestion and decoding, lending adapters, signing/broadcast/receipts, risk and bidding, metrics and credentials, Seqscope parser/storage/API/UI, all three first-party Solidity contracts, configuration, container/CI scripts, and infrastructure launch scripts.
- Inventory: 57 crate manifests under `crates/`; three first-party Solidity contracts. Review concentrated on execution, security, correctness, and operational boundaries rather than claiming formal verification of every dependency.
- Production inspection: read-only SSH to `mev-ohio`; checkout `/home/ubuntu/src/mev-bot`, observed revision `cb248f5936679cbfbbb51a3ca3cc7b64e83f5c0a`, 65 commits behind local HEAD. The running container reported live mode and healthy status. The checkout revision is **not independently verified image provenance**.
- Runtime snapshot: 2026-10-11 13:17:33 UTC. Later log activity is outside that snapshot.
- Existing generated documentation changes and untracked infrastructure work were present before this audit. No production source was changed; additions are this report and isolated audit evidence.
- [Reproduction instructions and captured evidence](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/README.md).

Severity: high means plausible material loss, accounting failure, or unsafe submission under the documented operating model; medium means a bounded correctness, availability, or operational risk; low means maintainability or hardening. Disabled features retain their conditional severity and are explicitly labelled.

## Production observations

| Observation | Evidence and implication |
|---|---|
| Live process | `mev-bot-arbitrum`, healthy, approximately 47 hours uptime; arguments `--mode live --config /app/config/arbitrum.toml`. Healthy heartbeat does not establish profitable or safe trading. |
| Deployment drift | Checkout `cb248f5`, 65 commits behind local HEAD. Host source lacks `RiskGuard::with_persistence`; newer global stop/probe parameters are absent. |
| Risk settings | `min_profit_usd = 0.05`, total daily gas cap `$3`, three concurrent arbitrage sends, pre-send arbitrage simulation enabled, `fast_send = false`. |
| Feature settings | Atlas observing enabled, bidding disabled; batching disabled. Presimulation enabled; local mode and rollback of unconfirmed tentative state disabled. |
| Persisted accounting | `risk-state.json`, `tip-state.json`, and `gate-state.json` absent from inspected production data directory. |
| Kill switch / heartbeat | STOP absent; heartbeat approximately seven seconds old. |
| Monitoring | User watchdog timer inactive; matching watchdog unit files were not installed. |
| October 10 journal | 37 sends, 37 landed records, all `success = false`; recorded gas `$2.0030776152`; one cap event. |
| October 11 journal through snapshot | 10 sends, 10 landed records, all `success = false`; recorded gas `$0.4400511975`. |
| Combined sample | **47 landed transactions, 47 reversions, about `$2.44` recorded gas, zero successful trades.** This describes these journal files, not lifetime performance or an independently reconciled wallet balance. |

## High findings

### H01 — Production lacks newer persistent limits and the global loss gate

**Affected:** inspected production checkout/runtime deployment. **Evidence:** direct observation and revision comparison.

Local code now loads persisted risk, route-gate, and tip state in [engine initialization](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:429), and the checked-in configuration enables a global arbitrage stop after 30 unsuccessful attempts with hourly probes. Production source and configuration predate these safeguards, and the expected state files are absent. Restarting the older guard resets its daily accounting; rotating across routes can continue losses without the newer global stop. The observed 47 reversions make this operationally relevant.

**Remediation:** reconcile wallet receipts first, deploy a reviewed revision with explicit image/commit provenance, seed accounting from chain/journal evidence, and verify persistence and the global gate with a controlled restart in shadow mode. Deploying local HEAD alone does not fix H02–H08.

### H02 — Daily budgets count settled spend but do not reserve pending liabilities

**Affected:** current live-capable risk path. **Evidence:** executable reproduction.

[Risk checks](/home/admministrator/projects/mev-bot/crates/mev-risk/src/lib.rs:144) compare `spent_usd + this_gas` against the cap. [mark_sent](/home/admministrator/projects/mev-bot/crates/mev-risk/src/lib.rs:207) increments a count without reserving funds. Multiple transactions can each pass against the same remaining budget. The harness records `$1.80` against a `$3` cap, admits three `$0.70` transactions, and settles at **`$3.90`**. The arbitrage sub-budget has the same defect.

The engine also checks risk before choosing the final priority bid, passing base gas cost without the bid into this check. Separate tip caps do not make the total gas budget conservative.

**Remediation:** reserve a conservative all-in liability atomically when admitting a transaction; persist it with the transaction hash/nonce; replace the reservation with actual cost on reconciliation. Include pending exposure in both aggregate and strategy budgets.

### H03 — Restart and receipt errors discard outstanding transaction exposure

**Affected:** current execution and persistence. **Evidence:** reproduction plus source inspection.

[Saved risk state](/home/admministrator/projects/mev-bot/crates/mev-risk/src/lib.rs:52) deliberately omits outstanding transactions, incorrectly assuming none remain after a restart. The harness confirms a restarted guard admits another send while a prior send remains outstanding. [Receipt polling](/home/admministrator/projects/mev-bot/crates/mev-exec/src/lib.rs:271) returns immediately on a transient RPC error. [Failure handling](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:2368) frees exposure and records zero gas even when the transaction hash is known and may subsequently land.

**Remediation:** journal signed/submitted/unknown/landed/replaced transaction states durably. An unavailable receipt must remain an unknown liability. Reconcile all unresolved hashes and nonces on startup and continue retrying bounded receipt lookups without declaring zero-cost failure.

### H04 — Nonce reset can collide with held reservations; ambiguous acceptance can trigger a fresh execution

**Affected:** concurrent transaction sender. **Evidence:** allocator reproduction and traced sender control flow.

[NonceManager::reset](/home/admministrator/projects/mev-bot/crates/mev-exec/src/nonce.rs:57) forgets reservations held by other send tasks. The reproduction allocates nonce 7, resets from an RPC pending count of 7, and allocates **7 again** while the first allocation is outstanding.

In [broadcast handling](/home/admministrator/projects/mev-bot/crates/mev-exec/src/lib.rs:224), “already known” is treated as accepted for endpoint diagnostics but still falls into the all-errors failure branch. The sender releases the nonce and [resynchronizes/re-signs](/home/admministrator/projects/mev-bot/crates/mev-exec/src/lib.rs:181). If the original was accepted and the pending count advanced, this can submit the same trade with a new nonce. Lost acknowledgements create a similar ambiguity. This can cause collisions, replacements, duplicate execution, and inaccurate risk accounting.

**Remediation:** preserve held nonces across refreshes; reconcile reservations with chain state. Treat acceptance evidence consistently. Re-broadcast identical signed bytes for ambiguous transport outcomes and confirm the original hash/nonce before constructing another transaction.

### H05 — Estimated profit increases spend limits as if it were realized profit

**Affected:** risk reinvestment and profitability metrics. **Evidence:** source inspection.

[Successful receipt handling](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:2324) calls `record_profit(est_profit_usd - gas_usd)`. The receipt structure does not provide the actual profit transferred by the executor; arbitrage completion carries the original local opportunity estimate even when simulation produced a different result. Half this value expands the daily spending budget. Batch completion similarly credits estimates for successful items.

A transaction can satisfy its small on-chain floor while earning far less than the estimate. For example, a `$100` model estimate and `$0.04` gas can add nearly `$50` to budget even if only `$0.05` gross profit was received. This is an illustration of the accounting formula, not an observed production transaction. Liquidation paths additionally pass a net estimate and subtract gas again.

**Remediation:** derive actual proceeds from executor profit events or verified token/native balance deltas, use consistent gross/net units, and expand budgets only after receipt and proceeds reconciliation. Label estimates as estimates in metrics.

### H06 — The on-chain profit floor does not cover the submitted transaction's allowed gas cost

**Affected:** arbitrage/liquidation profit protection. **Evidence:** source inspection.

The engine computes gas cost using [a calibrated gas estimate and cached base price](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1776), builds a floor from this estimate plus estimated tip, then submits with [2.5 times the estimated gas limit](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1881). Fee policy permits up to twice base fee plus priority fee. A transaction can consume more gas than estimated or land after a base-fee increase, satisfy its token profit floor, and still lose money.

[Gas refresh failure](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:2568) silently substitutes 10,000,000 wei rather than retaining a known-good value with explicit stale status. `simulate_fast` uses [100,000 + 115,000 per hop](/home/admministrator/projects/mev-bot/crates/mev-sim/src/lib.rs:84), not actual gas estimation. Arbitrum includes parent-chain posting cost in its gas accounting, making simple hop constants insufficient as a proven upper bound. [Official gas documentation](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/gas-and-fees).

**Remediation:** use a conservative chain-aware cost bound for risk and profit protection, reserve the allowed liability, explicitly expire fee/price data, and skip when a safe bound cannot be obtained. This audit does **not** assert that receipt `gas_used * effective_gas_price` necessarily omits L1 posting fees.

### H07 — Background tick reload can overwrite newer event-derived state and combine different blocks

**Affected:** concentrated-liquidity pricing. **Evidence:** source inspection.

[Tick reload](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1388) clones pools and performs remote reads without capturing their version snapshot. [Applying its result](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1469) unconditionally replaces tick maps, unlike the guarded pool-state refresh. A Mint/Burn processed during loading can be erased by an older map. No correction record explains that overwrite.

[load_ticks](/home/admministrator/projects/mev-bot/crates/mev-discovery/src/ticks.rs:223) reads slot0/bitmap and liquidity nets in separate RPC rounds without pinning a block. The resulting map can mix chain states and be marked usable for “exact” crossings.

**Remediation:** pin the entire snapshot to one chain block, carry the read versions through maintenance results, reject stale results, and emit corrections for accepted disagreements. Test a liquidity event arriving between read and apply.

### H08 — Decision paths violate the required integer arithmetic invariant

**Affected:** state, pricing, sizing, profit floors, gas valuation, bid decisions. **Evidence:** source inspection; binary64 representation properties.

[DEX state and calculations](/home/admministrator/projects/mev-bot/crates/mev-dex/src/lib.rs:3) use `f64` raw amounts, reserves, liquidity, and prices. [Search](/home/admministrator/projects/mev-bot/crates/mev-search/src/lib.rs:65) casts floating sizes to `u128`; bid decisions and profit floors also cast floating monetary calculations. Binary64 cannot distinguish every integer above `2^53`; normal 18-decimal amounts exceed that range. Its rounding is not the protocols' integer rounding.

The project explicitly requires integer money paths across **all live code**. “Exact” in the tick model currently describes its covered liquidity window, not bit-for-bit protocol arithmetic. Simulation and executor floors limit some consequences but do not establish this invariant, particularly for live paths that skip remote simulation.

**Remediation:** implement protocol-specific integer arithmetic and conservative rounding on submission paths; use floats only for display. Verify integer quotes against on-chain implementations at boundaries, extreme values, and fee tiers.

### H09 — Atlas bidding float is swept as liquidation profit and can subsidize a losing trade

**Affected:** optional AtlasSolver; bidding currently disabled. **Evidence:** three isolated Solidity tests using production contracts.

[Flash repayment/profit accounting](/home/admministrator/projects/mev-bot/contracts/src/ArbExecutor.sol:355) calculates profit from the entire remaining token balance. [_liquidate](/home/admministrator/projects/mev-bot/contracts/src/ArbExecutor.sol:387) also wraps all native ETH and swaps all collateral balance. The documented Atlas WETH bid float is therefore included in a WETH-denominated liquidation's apparent profit and transferred away before [bid payment](/home/admministrator/projects/mev-bot/contracts/src/AtlasSolver.sol:110).

The evidence tests establish:

1. A profitable WETH liquidation with a one-ETH WETH float and a 0.2-ETH bid reverts when bid withdrawal finds no WETH remaining.
2. A zero-bid liquidation earning one ETH transfers two ETH to the receiver, consuming the initial float.
3. A liquidation losing half an ETH still passes a 0.1-ETH profit floor by consuming the float.

This is not an unauthorized caller theft claim: existing owner/Atlas controls remain relevant. The issue is balance-delta accounting and working-capital preservation. The existing [MockWETH9](/home/admministrator/projects/mev-bot/contracts/test/AtlasSolver.t.sol:10) permits withdrawing ETH without owning WETH, so its tests conceal this behavior.

**Remediation:** isolate/reserve bid and borrowed native funds; measure incremental proceeds net of starting balances and repayment. Replace the mock with balance-enforcing wrapped-token behavior and test both WETH debt and WETH collateral.

### H10 — Atlas submissions do not settle against local exposure or spending limits

**Affected:** optional Atlas bidding; currently disabled. **Evidence:** source inspection.

[Auction submission](/home/admministrator/projects/mev-bot/crates/mev-engine/src/atlas.rs:214) checks concurrent risk, signs, and queues a solver bid. It does not reserve risk exposure with `mark_sent`, debit the bid, or track realized gas/profit in a receipt reconciliation path. [Submit replies](/home/admministrator/projects/mev-bot/crates/mev-engine/src/atlas.rs:161) are logged rather than settled. Repeated enabled bids can keep passing against an unchanged budget while consuming contract float or Atlas bonded funds.

**Remediation:** integrate auction reservations, expiry/loss/win states, bid payment and gas settlement into the durable transaction ledger before enabling bidding. Explicitly account for both wallet and bonded/contract funds.

## Medium findings

### M01 — Backruns bypass the global outstanding transaction limit

[check_concurrent](/home/admministrator/projects/mev-bot/crates/mev-risk/src/lib.rs:133) bypasses `max_in_flight` completely. The harness admits 100 outstanding backruns with a configured limit of three. Per-update strategy limits and cooldowns do not enforce a global pending-exposure ceiling across repeated updates. Add a separate bounded backrun allowance and apply the same monetary reservations as H02.

### M02 — Persistence fails open and is not crash-durable

[Store::load/save](/home/admministrator/projects/mev-bot/crates/mev-risk/src/persist.rs:19) ignores read failures/corruption and merely warns on write failure. Corrupting the harness snapshot resets spend to zero and admits the full budget. Atomic rename helps avoid partial JSON but lacks file/directory durability synchronization. Risk, arbitrage spend, and profit are saved separately. Treat unreadable/unwritable live accounting as a submission blocker, retain recoverable history, and reconcile before resuming.

### M03 — Non-finite configuration and guard inputs can bypass comparisons

The harness successfully loads `min_profit_usd = nan` through `ChainConfig::load` and admits NaN profit/gas through [RiskGuard](/home/admministrator/projects/mev-bot/crates/mev-risk/src/lib.rs:144). Comparisons with NaN are false. Reject non-finite/negative monetary configuration and inputs at load and admission; make undefined pricing an explicit blocked outcome. This is a configuration/arithmetic failure, not evidence of hostile remote configuration control.

### M04 — V4 liquidity events leave loaded tick maps stale

[apply_v4_liquidity](/home/admministrator/projects/mev-bot/crates/mev-state/src/lib.rs:313) adjusts only active liquidity. It neither changes lower/upper liquidity-net entries nor versions out-of-range changes, unlike the V3 Mint/Burn handler. Later cross-tick quotes can use obsolete depth until reload. Apply event deltas to the tick map, version all pricing-relevant changes, and invalidate confidence when a complete update is unavailable.

### M05 — Opportunity version protection ends before actual submission

Arbitrage stamps versions and checks them [after simulation](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1813), then [spawns a sender](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:1910) with no state-version check at signing/broadcast. The event loop can process further state changes while that task is delayed. Liquidation batch items carry [empty version vectors](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:2122). Simulation also runs while the main engine is awaiting it, so queued updates cannot invalidate a candidate during that wait. Carry exact read dependencies into every opportunity and validate them at simulation and submission boundaries against an independently advancing state owner.

### M06 — Reconnection leaves a snapshot/subscription gap without contagious degraded confidence

[Reconnect handling](/home/admministrator/projects/mev-bot/crates/mev-engine/src/lib.rs:954) refreshes state before the next subscription; there is no intervening log catch-up pinned to a watermark. Direct refresh does not emit the same correction records as maintenance refresh. Feed continuity warnings are not a market confidence gate. A missed interval can leave stale tradable state and incomplete history. Subscribe/buffer, snapshot at a block, replay from its watermark, and prevent submissions until affected markets reconcile. Standard feed observations and Fast Feed tentative execution must retain distinct confidence states. [Official sequencer documentation](https://docs.arbitrum.io/how-arbitrum-works/deep-dives/sequencer).

### M07 — Trading hot path still performs remote and synchronous disk work

Pre-send simulation, route healing, and initial gas retrieval await remote calls within the event-driven trading path. Risk persistence uses synchronous filesystem calls; admission checks the STOP file, and state-saving callbacks can block the same loop. [Broadcast](/home/admministrator/projects/mev-bot/crates/mev-exec/src/lib.rs:224) waits for every endpoint before returning, even after one accepted the transaction. These violate the declared hot-path invariant and can consume an ordering round. Use background state/fee services, a durable admission boundary outside feed processing, bounded asynchronous diagnostics, and first-accept handling with later endpoint telemetry.

### M08 — Batch flush does not repeat admission checks and can build zero floors

**Batching currently disabled.** [flush_batch](/home/admministrator/projects/mev-bot/crates/mev-engine/src/batch.rs:119) rechecks pool versions but not STOP, sending mode, failure pause, or aggregate gas budget immediately before sending. Multiple queued items checked independently can exceed available exposure. [min_profits](/home/admministrator/projects/mev-bot/crates/mev-engine/src/batch.rs:61) returns zero for invalid prices; a current test explicitly accepts this. Repeat complete admission for the final batch, validate every price, reserve aggregate cost, and reject any item without a safe floor.

### M09 — Rule expression nesting can abort the Seqscope process

[Recursive parser](/home/admministrator/projects/mev-bot/crates/seqscope-core/src/expr.rs:327) has no nesting budget. A 40-KB expression consisting of 10,000 `not` operators causes stack overflow and process abort on a two-MiB thread stack. The API accepts rule/explanation inputs, and the configured request size limit does not prevent this. Default loopback binding limits remote exposure; enabling remote access increases the availability impact. Bound token count/nesting and use an iterative parser/evaluator. The evidence is a direct parser crash, not a separately executed HTTP attack.

### M10 — SQL row caps do not bound query work or response bytes

[Reader::sql](/home/admministrator/projects/mev-bot/crates/seqscope-store/src/lib.rs:493) limits collected rows after stepping the query. It has no progress/deadline budget or result-byte limit. A single large expression or recursive query can exhaust memory/CPU despite a one-row cap. [API dispatch](/home/admministrator/projects/mev-bot/crates/seqscope-server/src/lib.rs:696) uses blocking workers without a query-specific concurrency budget. Add SQLite interruption/progress limits, byte ceilings and bounded concurrent work; retain read-only database access. No destructive SQL bypass was established.

### M11 — Production watchdog is absent; its health pattern can accept “unhealthy”

Production user timer inspection returned inactive and no matching installed unit files. Separately, [watchdog status matching](/home/admministrator/projects/mev-bot/scripts/watchdog.sh:18) uses `Up*healthy*`, which also matches `Up ... (unhealthy)`. A separate heartbeat check catches some failures, but this status test does not reliably detect unhealthy containers. Install and exercise the timer and alert destination, parse a structured health field, and test stopped/unhealthy/stale-heartbeat and loss scenarios. A fresh heartbeat alone is not a financial safety check.

### M12 — CI omits substantial suites; Seqscope test wrapper can hide failing exit status

[CI contracts job](/home/admministrator/projects/mev-bot/.github/workflows/ci.yml:61) selects only ArbExecutor unit/fork files, omitting AtlasSolver, Batch, V4, and ticket-vault tests. Browser UI and ignored Rust integration tests are also absent from the default gate. [test-seqscope.sh](/home/admministrator/projects/mev-bot/scripts/test-seqscope.sh:38) runs tests twice and swallows pipeline exit codes with `|| true`, inferring failure from text. A nonzero exit without its selected words can appear successful. Preserve each command's exit status, run once, and gate the relevant suites explicitly. Separate tests requiring fixtures/providers and report their skips.

### M13 — Redaction does not cover URL userinfo; historical credential-like logging exists

[redact_secrets](/home/admministrator/projects/mev-bot/crates/mev-types/src/lib.rs:118) preserves the authority section, including `user:password@host`. The synthetic reproduction masks a path API key but retains the password. Main tracing is redacted, but errors outside that writer and other binaries require separate scrutiny. A path-only credential scan identified a credential-shaped RPC URL in `data/shadow-presim.log`; its permissions are `0600`, and validity/rotation were not tested. `.env` and its backup are also `0600` and ignored; inspected Git history did not contain those paths. Parse/redact URL userinfo and provider credential formats at all sinks, sanitize historical artifacts, and rotate any confirmed still-valid exposed credential. No production private-key disclosure was established.

## Low findings

### L01 — Two locked dependencies are marked unmaintained

`cargo-audit` scanned 603 locked dependencies against RustSec database commit `7eebec69c352c7191b1f13eb95dd510eeca5d1de` (updated October 9). It found **zero known vulnerability advisories**, and unmaintained notices for `derivative 2.2.0` (`RUSTSEC-2024-0388`) and `paste 1.0.15` (`RUSTSEC-2024-0436`). Plan upstream upgrades or replacement according to dependency ownership; these notices are not evidence of an exploitable vulnerability. The scan does not cover Solidity, container OS packages, or all JavaScript tooling.

### L02 — Nitro launch-script fixes exist outside the installed path

The pre-existing untracked [root nitro-start.sh](/home/admministrator/projects/mev-bot/nitro-start.sh:11) checks the persistent mount and completed snapshot marker. The [tracked launcher](/home/admministrator/projects/mev-bot/infra/arbitrum-node/nitro-start.sh:6) lacks those guards, while [bootstrap](/home/admministrator/projects/mev-bot/infra/arbitrum-node/bootstrap-arbitrum-node.sh:114) installs the tracked copy. Consolidate into one reviewed installed source. The tracked script's reuse of `region` first for the metadata token and then the region is confusing, but is not by itself a proved IMDS authentication bug.

### L03 — Module ownership and documentation make safety review harder

The central engine remains a large module with execution, strategy, accounting and operational responsibilities; `mev-bindings` depends on `mev-lending`, contrary to the documented layer direction. Older audit documents contain claims now fixed locally or insufficiently supported. Split durable transaction/accounting ownership from strategy evaluation, document the actual dependency boundary, and clearly supersede stale audit findings. These are maintainability issues, not independently proven fund-loss defects.

### L04 — Shell quality checks need cleanup

Shell syntax passed for 35 scripts; Python syntax passed for 10 scripts. ShellCheck produced six diagnostics, including unchecked `cd` in watchdog/mutation scripts, a single-iteration quoted loop in Atlas setup, and assignment/unused-variable warnings. Fix meaningful directory/error-handling warnings and annotate intentional cases. Containerized builds and locked Rust dependencies provide useful controls, but toolchain/action/image pinning should be strengthened where floating references remain.

## Standing invariant assessment

| Required invariant | Assessment | Evidence |
|---|---|---|
| Integer decision math | **Fail** | H08; `f64` state/sizing/fees/profits. |
| Chain state wins with correction records | **Partial** | Periodic pool refresh has version guards/corrections; tick application and direct reconnect refresh do not (H07/M06). |
| Exact opportunity versions at sim/send | **Partial** | Arbitrage stamps/checks versions; async submission and liquidation dependencies are incomplete (M05). |
| Hot path purity | **Fail** | RPC awaits, filesystem checks/writes, waits for all broadcasts (M07). |
| Shadow default and throwaway-key signing | **Partial** | Default mode remains shadow and a sender is constructed only in live mode. Shadow does not implement the required build-and-sign workflow with a throwaway key; compose still injects the shared env file. No audit transaction was signed or sent. |
| Own transactions follow ordinary ingestion/reconciliation | **Partial** | Normal chain log ingestion exists; receipt/accounting shortcuts discard unknown liabilities and do not reconcile actual proceeds (H03/H05). |
| Degraded confidence prevents submission | **Fail** | Continuity/rollback observations are not an enforceable strategy confidence gate; production rollback is disabled (M06). |
| No production stubs represented as finished | **No new violation established** | No production TODO/unimplemented execution path was identified in reviewed boundaries. This is not a proof of complete phase definitions of done. |

## Verification

| Check | Result |
|---|---|
| Rust formatting | `cargo fmt --all --check`: passed. |
| Rust lint | `cargo clippy -q --workspace --all-targets --locked -- -D warnings`: passed. |
| Rust default tests | `cargo test -q --workspace --locked`: **493 passed, zero failed, 16 ignored**, across 142 reported suite summaries. |
| Solidity formatting/build sizes | Passed. Runtime sizes: ArbExecutor 20,369 bytes, AtlasSolver 22,548 bytes, FastFeedTicketVault 6,521 bytes; all within reported deployment limits. |
| Solidity offline suites | **25 passed**, four suites including access-control fuzzing and invariant testing. |
| Solidity complete suite | `forge test -q`: passed, including the fork test files. Quiet success log does not supply a per-suite count. |
| UI logic | **23 passed**, zero failed. |
| Chromium UI | **17 passed**, zero failed; no browser/page errors. SIGTERM cleanly flushed 2,861 stored rows with zero dropped/failed. |
| Risk/nonce/config/redaction harness | Reproduced H02/H03/H04, M01/M02/M03/M13 without RPC, keys or signing. |
| Parser harness | Expected stack overflow reproduced; separate child exit 134. |
| Atlas balance harness | **Three passed**, demonstrating the defective behavior in H09. These tests assert the bug, not a fix. |
| Dependency scan | Passed after selecting a writable advisory DB; zero known vulnerabilities, two unmaintained notices (L01). |
| Shell/Python syntax | 35/10 files passed; six ShellCheck diagnostics (L04). |
| RPC-backed Rust tests | **14 passed, two failed** because Aave/Morpho borrower sampling exceeded the selected provider's block-range limit. Discovery: 6/6; Compound: 2/2; Fluid: 1/1; presimulation: 2/2; Aave: 2/3; Morpho: 1/2. An initial missing discovery fixture was corrected. These checks are not represented as an all-green gate. |

Validation used the repository's Podman development launcher. The normal workspace excludes the standalone audit harness, so its reproductions were run separately. No live bot invocation, funded fork transaction on a real chain, deployment, or production-key access was part of validation.

## Findings from older audits that should not be repeated as current facts

| Older concern | Current evidence |
|---|---|
| Local `fast_send` is on | False: checked-in and production values are false. |
| Single-send missing price yields zero profit floor | Fixed locally: `profit_floor` rejects missing/non-finite pricing. Batch helper still has M08. |
| Local daily risk never persists | Fixed for settled counters at HEAD, but fail-open/pending problems remain; not deployed on inspected host. |
| No watchdog installation files exist | Installation files exist locally; production installation/timer is absent. |
| STOP resume lacks confirmation | Local resume tooling includes confirmation; do not repeat the old claim without distinguishing revision. |
| IMDS token/region variable reuse always prevents startup | Not demonstrated; assignment evaluates the token before replacing the variable. |
| Receipt accounting definitely excludes L1 fees | Not established; no such claim is made here. |

## Remediation order and acceptance criteria

1. **Reconcile and contain live exposure.** Operator holds arbitrage, inventories pending/mined transactions, restores correct daily spend, verifies kill switch/monitoring, and records actual image provenance. Acceptance: no unaccounted pending hashes and no silent budget reset across a controlled restart.
2. **Create one durable transaction and money ledger.** Address H02–H05, M01–M03 and H10. Acceptance: concurrency, lost acknowledgements, transient receipt errors, replacement, restart and corrupt disk never release unknown liabilities; only verified realized proceeds extend budgets.
3. **Restore conservative submission boundaries.** Address H06–H08 and M04–M07. Acceptance: coherent pinned state, integer protocol rounding, mandatory dependency/confidence checks and conservative cost admission immediately before send.
4. **Repair optional execution before enablement.** H09/H10/M08 require realistic wrapped-token accounting, funded-float preservation, Atlas settlement, and atomic batch admission tests.
5. **Harden the operator interface and release gate.** Bound parser/SQL resources, fix watchdog and CI exit handling, broaden suites, finish credential hygiene, consolidate launch scripts, and document remaining limitations.

## Limits of this audit

This is a complete point-in-time review of the accessible repository and the specified read-only deployment observations, **not formal verification or certification**. External protocol implementations and full third-party dependency source were not exhaustively audited. AWS resource inventory, IAM/network exposure, billing, external alert delivery, on-chain deployed bytecode/allowlists, signer security, and current credential validity were not independently verified. Provider-dependent tests can skip branches or fail for environmental limits; their results are preserved rather than treated as proof of coverage. Production journals were not independently matched transaction-by-transaction against chain receipts.

The reviewed safeguards materially reduce some risks: owner-gated contract entry points, checked callback identities, route validation, target allowlists, approval reset, executor profit checks, shadow default, missing-price rejection for single sends, and locked dependency builds. The high findings still prevent a clean live-safety assessment.
