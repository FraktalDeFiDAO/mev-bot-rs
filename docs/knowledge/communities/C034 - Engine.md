---
graphify-generated: true
graphify-community: "34"
graphify-node-count: 50
graphify-internal-edges: 109
tags:
  - graphify/community
---

# Engine

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-engine/src/lib.rs` — 21 nodes
- `graph` — 21 nodes
- `crates/mev-sim/src/lib.rs` — 7 nodes
- `crates/mev-risk/src/lib.rs` — 1 node

## Nodes (50)

- **Engine** — `crates/mev-engine/src/lib.rs:L48`
- **lib.rs** — `crates/mev-engine/src/lib.rs:L1`
- **.bootstrap()** — `crates/mev-engine/src/lib.rs:L96`
- **wire_executor()** — `crates/mev-engine/src/lib.rs:L628`
- **build_liquidation()** — `crates/mev-engine/src/lib.rs:L670`
- **Simulator** — `crates/mev-sim/src/lib.rs:L18`
- **TxDone** — `crates/mev-engine/src/lib.rs:L81`
- **.on_plan()** — `crates/mev-engine/src/lib.rs:L476`
- **.simulate_liquidation()** — `crates/mev-sim/src/lib.rs:L53`
- **Option** — `graph`
- **.on_pool_update()** — `crates/mev-engine/src/lib.rs:L343`
- **.price()** — `crates/mev-engine/src/lib.rs:L320`
- **.simulate()** — `crates/mev-sim/src/lib.rs:L35`
- **LiqCtx** — `crates/mev-engine/src/lib.rs:L66`
- **SimOutcome** — `crates/mev-sim/src/lib.rs:L13`
- **.new()** — `crates/mev-sim/src/lib.rs:L25`
- **Address** — `graph`
- **ScanResult** — `crates/mev-engine/src/lib.rs:L72`
- **.on_runner_event()** — `crates/mev-engine/src/lib.rs:L461`
- **.scan_once()** — `crates/mev-engine/src/lib.rs:L266`
- **lib.rs** — `crates/mev-sim/src/lib.rs:L1`
- **.on_tx_done()** — `crates/mev-engine/src/lib.rs:L589`
- **check_allowlist()** — `crates/mev-engine/src/lib.rs:L718`
- **Mode** — `crates/mev-engine/src/lib.rs:L41`
- **now_secs()** — `crates/mev-engine/src/lib.rs:L742`
- **recv_opt()** — `crates/mev-engine/src/lib.rs:L735`
- **Result** — `graph`
- **short()** — `crates/mev-sim/src/lib.rs:L80`
- **.run()** — `crates/mev-engine/src/lib.rs:L213`
- **Result** — `graph`
- **U256** — `graph`
- **UnboundedSender** — `graph`
- **Vec** — `graph`
- **Address** — `graph`
- **Arc** — `graph`
- **DynProvider** — `graph`
- **loan_cap()** — `crates/mev-risk/src/lib.rs:L102`
- **String** — `graph`
- **String** — `graph`
- **UnboundedReceiver** — `graph`
- **.log_summary()** — `crates/mev-engine/src/lib.rs:L325`
- **.pool_count()** — `crates/mev-engine/src/lib.rs:L316`
- **HashMap** — `graph`
- **Instant** — `graph`
- **Path** — `graph`
- **PathBuf** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **TxHash** — `graph`

## Related communities

- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 9 cross-community edges
- [[docs/knowledge/communities/C021 - lib.rs (C021)|lib.rs (C021)]] — 6 cross-community edges
- [[docs/knowledge/communities/C026 - lib.rs (C026)|lib.rs (C026)]] — 6 cross-community edges
- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 4 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 3 cross-community edges
- [[docs/knowledge/communities/C027 - lib.rs (C027)|lib.rs (C027)]] — 2 cross-community edges
- [[docs/knowledge/communities/C029 - lib.rs (C029)|lib.rs (C029)]] — 2 cross-community edges
