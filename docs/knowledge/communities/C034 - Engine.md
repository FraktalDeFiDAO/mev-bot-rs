---
graphify-generated: true
graphify-community: "34"
graphify-node-count: 44
graphify-internal-edges: 91
tags:
  - graphify/community
---

# Engine

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 19 nodes
- `crates/mev-engine/src/lib.rs` — 17 nodes
- `crates/mev-sim/src/lib.rs` — 7 nodes
- `crates/mev-risk/src/lib.rs` — 1 node

## Nodes (44)

- **Engine** — `crates/mev-engine/src/lib.rs:L47`
- **lib.rs** — `crates/mev-engine/src/lib.rs:L1`
- **wire_executor()** — `crates/mev-engine/src/lib.rs:L654`
- **.bootstrap()** — `crates/mev-engine/src/lib.rs:L99`
- **LiqCtx** — `crates/mev-engine/src/lib.rs:L65`
- **Simulator** — `crates/mev-sim/src/lib.rs:L17`
- **TxDone** — `crates/mev-engine/src/lib.rs:L84`
- **.on_pool_update()** — `crates/mev-engine/src/lib.rs:L356`
- **.price()** — `crates/mev-engine/src/lib.rs:L333`
- **.simulate_liquidation()** — `crates/mev-sim/src/lib.rs:L52`
- **.simulate()** — `crates/mev-sim/src/lib.rs:L34`
- **Option** — `graph`
- **SimOutcome** — `crates/mev-sim/src/lib.rs:L12`
- **.new()** — `crates/mev-sim/src/lib.rs:L24`
- **ScanResult** — `crates/mev-engine/src/lib.rs:L75`
- **.scan_once()** — `crates/mev-engine/src/lib.rs:L279`
- **.on_tx_done()** — `crates/mev-engine/src/lib.rs:L615`
- **Address** — `graph`
- **lib.rs** — `crates/mev-sim/src/lib.rs:L1`
- **Mode** — `crates/mev-engine/src/lib.rs:L40`
- **now_secs()** — `crates/mev-engine/src/lib.rs:L703`
- **recv_opt()** — `crates/mev-engine/src/lib.rs:L696`
- **short()** — `crates/mev-sim/src/lib.rs:L79`
- **.run()** — `crates/mev-engine/src/lib.rs:L226`
- **Result** — `graph`
- **Result** — `graph`
- **U256** — `graph`
- **Address** — `graph`
- **Arc** — `graph`
- **DynProvider** — `graph`
- **HashMap** — `graph`
- **Instant** — `graph`
- **loan_cap()** — `crates/mev-risk/src/lib.rs:L102`
- **String** — `graph`
- **String** — `graph`
- **UnboundedReceiver** — `graph`
- **Vec** — `graph`
- **.log_summary()** — `crates/mev-engine/src/lib.rs:L338`
- **.pool_count()** — `crates/mev-engine/src/lib.rs:L329`
- **PathBuf** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **TxHash** — `graph`

## Related communities

- [[docs/knowledge/communities/C021 - lib.rs (C021)|lib.rs (C021)]] — 6 cross-community edges
- [[docs/knowledge/communities/C026 - lib.rs (C026)|lib.rs (C026)]] — 6 cross-community edges
- [[docs/knowledge/communities/C031 - lib.rs (C031)|lib.rs (C031)]] — 5 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 3 cross-community edges
- [[docs/knowledge/communities/C027 - lib.rs (C027)|lib.rs (C027)]] — 2 cross-community edges
- [[docs/knowledge/communities/C029 - lib.rs (C029)|lib.rs (C029)]] — 2 cross-community edges
- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 2 cross-community edges
- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 2 cross-community edges
