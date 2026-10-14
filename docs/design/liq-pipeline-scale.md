# Liquidation snapshot pipeline at full scale

The first shadow deploy of the pipeline (deploy-20261014-030250) loaded every indexed Aave borrower
(188,127) on every 5 s tick, one SQLite query per borrower, and rechecked 30 accounts a minute. It drove
the bot past 1.6 GB RSS and was OOM-killed 15 times on the 2 GB host, and 300 s coverage was impossible
by four orders of magnitude. This note records how the pipeline now keeps every input bounded.

## Population

A sample of 233 stored snapshots on mainnet: 70% zero debt, 19% under $10, 11% at or above $10.

- **Zero debt.** The trigger indexer's reconciliation deactivates these, and with them their dirty marks
  (a later `Borrow` re-registers the borrower). `reconcile_batch` went from 50 to 500 per step, so the
  backlog clears in well under an hour instead of days.
- **Dust** (debt < `min_debt_usd`, default $10). Re-read every `cold_max_age_seconds` (3600 s) and at once
  when an event marks them dirty. The schedule pass leaves them out unless they are dirty.
- **Hot** (debt ≥ cutoff, or an invalidated snapshot). Carries the `max_account_age_seconds` (300 s) guarantee.
- **Never read.** At most `max_unknown_per_pass` (2000) per schedule pass. The rest are counted as
  `deferred_unknown`, and they still count toward the sweep-capacity check, so coverage is never overstated.

## Reads

`AaveAccountRechecker::recheck_many` reads a batch through Multicall3 `aggregate3`:

- 50 accounts per request.
- One block for the whole batch, pinned by hash, `PIN_LAG_BLOCKS` behind the head so every endpoint
  behind the rotating sweep provider has it.

It uses the sweep endpoints, not the fast endpoint the live path depends on. Each batch is stored in one
transaction (`store_rechecks`); a snapshot older than the stored block is counted as superseded.

Refresh order per tick (`refresh_candidates`, one bounded statement per stage):

1. Dirty accounts with a snapshot.
2. Invalidated, never-checked and expired hot accounts.
3. Never-read borrowers.
4. Expired dust.

The tick's share of `recheck_budget_per_minute` caps it.

## Capacity model

The schedule pass is configured with the refresh task's real capacity:

- `concurrency` = batches in flight × batch size.
- `recheck_cost_ns` is chosen so one minute holds exactly the budget.

A max-age window then holds 5 × budget rechecks, and the P1 sweep share is 30% of that. At 20,000/min
that is 30,000 hot accounts per 300 s. A larger hot set raises `CoverageViolation::SweepCapacity`, and the
budget must be raised before claiming coverage.

## View

- `liq_shadow_census` (every `census_seconds`): tier counts, freshness and the 10 lowest health factors.
  It is exported as `copper_liq_accounts{state}`.
- Refresh rounds: `copper_liq_rechecks_total{result}`.
- `scripts/run.sh liq` renders both from the bot's logs.

## Baseline

One schedule pass on a synthetic 188,000-borrower database:

- Built by `cargo run --release -p mev-liq-sched --example pass_cost`.
- Same 70/19/11 debt mix as the mainnet sample.
- Default `PolicyConfig`.
- Three passes each, run in the dev container on 2026-10-14.

| Phase | Load | Accounts in the pass | Pass time | Peak RSS |
|---|---|---|---|---|
| Warm-up (nothing read yet) | unbounded (`load_inputs`) | 188,000 | 6.2–7.2 s | 551 MB |
| Warm-up | bounded | 2,000 (+186,000 deferred) | 0.4–0.9 s | 29 MB |
| Steady (all read) | unbounded | 188,000 | 8.9–10.1 s | 475 MB |
| Steady | bounded | 20,680 hot | 1.2–1.8 s | 53 MB |
