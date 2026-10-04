---
graphify-generated: true
graphify-community: "34"
graphify-node-count: 55
graphify-internal-edges: 132
tags:
  - graphify/community
---

# Engine

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-engine/src/lib.rs` — 25 nodes
- `graph` — 22 nodes
- `crates/mev-sim/src/lib.rs` — 7 nodes
- `crates/mev-risk/src/lib.rs` — 1 node

## Nodes (55)

- **Engine** — `crates/mev-engine/src/lib.rs:L48`
- **lib.rs** — `crates/mev-engine/src/lib.rs:L1`
- **build_liquidation()** — `crates/mev-engine/src/lib.rs:L789`
- **.bootstrap()** — `crates/mev-engine/src/lib.rs:L102`
- **wire_executor()** — `crates/mev-engine/src/lib.rs:L741`
- **.on_plan()** — `crates/mev-engine/src/lib.rs:L591`
- **Option** — `graph`
- **Simulator** — `crates/mev-sim/src/lib.rs:L18`
- **TxDone** — `crates/mev-engine/src/lib.rs:L87`
- **.on_pool_update()** — `crates/mev-engine/src/lib.rs:L458`
- **.route()** — `crates/mev-engine/src/lib.rs:L350`
- **.simulate_liquidation()** — `crates/mev-sim/src/lib.rs:L53`
- **metered_rotating()** — `crates/mev-engine/src/lib.rs:L865`
- **.price()** — `crates/mev-engine/src/lib.rs:L409`
- **.simulate()** — `crates/mev-sim/src/lib.rs:L35`
- **Address** — `graph`
- **LiqCtx** — `crates/mev-engine/src/lib.rs:L72`
- **metered()** — `crates/mev-engine/src/lib.rs:L855`
- **SimOutcome** — `crates/mev-sim/src/lib.rs:L13`
- **Vec** — `graph`
- **.new()** — `crates/mev-sim/src/lib.rs:L25`
- **Result** — `graph`
- **ScanResult** — `crates/mev-engine/src/lib.rs:L78`
- **.on_runner_event()** — `crates/mev-engine/src/lib.rs:L576`
- **.scan_once()** — `crates/mev-engine/src/lib.rs:L293`
- **Arc** — `graph`
- **lib.rs** — `crates/mev-sim/src/lib.rs:L1`
- **.on_tx_done()** — `crates/mev-engine/src/lib.rs:L702`
- **check_allowlist()** — `crates/mev-engine/src/lib.rs:L876`
- **Mode** — `crates/mev-engine/src/lib.rs:L41`
- **now_secs()** — `crates/mev-engine/src/lib.rs:L900`
- **recv_opt()** — `crates/mev-engine/src/lib.rs:L893`
- **short()** — `crates/mev-sim/src/lib.rs:L80`
- **.gas_price()** — `crates/mev-engine/src/lib.rs:L392`
- **.run()** — `crates/mev-engine/src/lib.rs:L229`
- **Result** — `graph`
- **String** — `graph`
- **U256** — `graph`
- **UnboundedSender** — `graph`
- **Address** — `graph`
- **DynProvider** — `graph`
- **DynProvider** — `graph`
- **loan_cap()** — `crates/mev-risk/src/lib.rs:L102`
- **String** — `graph`
- **UnboundedReceiver** — `graph`
- **.log_summary()** — `crates/mev-engine/src/lib.rs:L414`
- **.pool_count()** — `crates/mev-engine/src/lib.rs:L343`
- **HashMap** — `graph`
- **Instant** — `graph`
- **Path** — `graph`
- **PathBuf** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **TxHash** — `graph`

## Related communities

- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 8 cross-community edges
- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 8 cross-community edges
- [[docs/knowledge/communities/C021 - lib.rs (C021)|lib.rs (C021)]] — 6 cross-community edges
- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 5 cross-community edges
- [[docs/knowledge/communities/C025 - lib.rs (C025)|lib.rs (C025)]] — 4 cross-community edges
- [[docs/knowledge/communities/C026 - lib.rs (C026)|lib.rs (C026)]] — 3 cross-community edges
- [[docs/knowledge/communities/C027 - lib.rs (C027)|lib.rs (C027)]] — 2 cross-community edges
- [[docs/knowledge/communities/C029 - lib.rs (C029)|lib.rs (C029)]] — 2 cross-community edges
