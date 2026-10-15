---
graphify-generated: true
graphify-community: "44"
graphify-node-count: 40
graphify-internal-edges: 96
tags:
  - graphify/community
---

# liq_shadow.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-engine/src/liq_shadow.rs` — 26 nodes
- `graph` — 12 nodes
- `crates/mev-config/src/lib.rs` — 1 node
- `crates/mev-liq-state/src/tiering.rs` — 1 node

## Nodes (40)

- **liq_shadow.rs** — `crates/mev-engine/src/liq_shadow.rs:L1`
- **refresh_once()** — `crates/mev-engine/src/liq_shadow.rs:L196`
- **ShadowParts** — `crates/mev-engine/src/liq_shadow.rs:L39`
- **LiquidationShadowConfig** — `crates/mev-config/src/lib.rs:L116`
- **.new()** — `crates/mev-engine/src/liq_shadow.rs:L126`
- **census()** — `crates/mev-engine/src/liq_shadow.rs:L277`
- **open_repo()** — `crates/mev-engine/src/liq_shadow.rs:L48`
- **schedule()** — `crates/mev-engine/src/liq_shadow.rs:L90`
- **Budget** — `crates/mev-engine/src/liq_shadow.rs:L119`
- **parts()** — `crates/mev-engine/src/liq_shadow.rs:L394`
- **snap()** — `crates/mev-engine/src/liq_shadow.rs:L412`
- **spawn_refresh()** — `crates/mev-engine/src/liq_shadow.rs:L311`
- **failed_reads_count_as_rpc_errors_and_stay_due()** — `crates/mev-engine/src/liq_shadow.rs:L533`
- **refresh_batches_within_budget_and_concurrency_and_fills_the_repo()** — `crates/mev-engine/src/liq_shadow.rs:L490`
- **tiers()** — `crates/mev-engine/src/liq_shadow.rs:L74`
- **.available()** — `crates/mev-engine/src/liq_shadow.rs:L142`
- **Instant** — `graph`
- **.take()** — `crates/mev-engine/src/liq_shadow.rs:L148`
- **now_ns()** — `crates/mev-engine/src/liq_shadow.rs:L186`
- **startup_wiring_builds_a_bounded_schedule()** — `crates/mev-engine/src/liq_shadow.rs:L444`
- **.roll()** — `crates/mev-engine/src/liq_shadow.rs:L134`
- **Batch** — `crates/mev-engine/src/liq_shadow.rs:L176`
- **per_tick()** — `crates/mev-engine/src/liq_shadow.rs:L156`
- **Result** — `graph`
- **slots()** — `crates/mev-engine/src/liq_shadow.rs:L83`
- **StoreOutcome** — `crates/mev-liq-state/src/tiering.rs:L62`
- **Address** — `graph`
- **alert_disabled()** — `crates/mev-engine/src/liq_shadow.rs:L65`
- **Arc** — `graph`
- **budget_is_a_rolling_per_minute_cap_spread_over_ticks()** — `crates/mev-engine/src/liq_shadow.rs:L473`
- **Path** — `graph`
- **RefreshReport** — `crates/mev-engine/src/liq_shadow.rs:L162`
- **ShadowSink** — `graph`
- **SnapshotRepository** — `graph`
- **String** — `graph`
- **Error** — `graph`
- **F** — `graph`
- **Self** — `graph`
- **unopenable_db_is_an_error_not_a_panic()** — `crates/mev-engine/src/liq_shadow.rs:L461`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 3 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 3 cross-community edges
- [[docs/knowledge/communities/C108 - ChainConfig|ChainConfig]] — 2 cross-community edges
- [[docs/knowledge/communities/C086 - lib.rs (C086)|lib.rs (C086)]] — 2 cross-community edges
- [[docs/knowledge/communities/C087 - Self|Self]] — 2 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C029 - ShadowSchedule|ShadowSchedule]] — 1 cross-community edges
- [[docs/knowledge/communities/C005 - api.rs|api.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
