---
graphify-generated: true
graphify-community: "34"
graphify-node-count: 80
graphify-internal-edges: 161
tags:
  - graphify/community
---

# Engine

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 37 nodes
- `crates/mev-engine/src/lib.rs` — 23 nodes
- `crates/mev-rpc/src/meter.rs` — 12 nodes
- `crates/mev-sim/src/lib.rs` — 7 nodes
- `crates/mev-liquidate/src/lib.rs` — 1 node

## Nodes (80)

- **Engine** — `crates/mev-engine/src/lib.rs:L48`
- **lib.rs** — `crates/mev-engine/src/lib.rs:L1`
- **RpcMeter** — `crates/mev-rpc/src/meter.rs:L21`
- **build_liquidation()** — `crates/mev-engine/src/lib.rs:L724`
- **.bootstrap()** — `crates/mev-engine/src/lib.rs:L100`
- **wire_executor()** — `crates/mev-engine/src/lib.rs:L682`
- **.on_plan()** — `crates/mev-engine/src/lib.rs:L530`
- **RunnerEvent** — `crates/mev-liquidate/src/lib.rs:L45`
- **Simulator** — `crates/mev-sim/src/lib.rs:L18`
- **TxDone** — `crates/mev-engine/src/lib.rs:L85`
- **.on_pool_update()** — `crates/mev-engine/src/lib.rs:L397`
- **.simulate_liquidation()** — `crates/mev-sim/src/lib.rs:L53`
- **Option** — `graph`
- **.price()** — `crates/mev-engine/src/lib.rs:L351`
- **.simulate()** — `crates/mev-sim/src/lib.rs:L35`
- **LiqCtx** — `crates/mev-engine/src/lib.rs:L70`
- **metered()** — `crates/mev-engine/src/lib.rs:L784`
- **MeterLayer** — `crates/mev-rpc/src/meter.rs:L49`
- **SimOutcome** — `crates/mev-sim/src/lib.rs:L13`
- **.new()** — `crates/mev-sim/src/lib.rs:L25`
- **Address** — `graph`
- **ScanResult** — `crates/mev-engine/src/lib.rs:L76`
- **.on_runner_event()** — `crates/mev-engine/src/lib.rs:L515`
- **.poll_ready()** — `crates/mev-rpc/src/meter.rs:L82`
- **.scan_once()** — `crates/mev-engine/src/lib.rs:L279`
- **lib.rs** — `crates/mev-sim/src/lib.rs:L1`
- **meter.rs** — `crates/mev-rpc/src/meter.rs:L1`
- **Result** — `graph`
- **Vec** — `graph`
- **.call()** — `crates/mev-rpc/src/meter.rs:L86`
- **.new()** — `crates/mev-rpc/src/meter.rs:L26`
- **.on_tx_done()** — `crates/mev-engine/src/lib.rs:L643`
- **Arc** — `graph`
- **check_allowlist()** — `crates/mev-engine/src/lib.rs:L795`
- **MeterService** — `crates/mev-rpc/src/meter.rs:L63`
- **MeterService<S>** — `crates/mev-rpc/src/meter.rs:L68`
- **Mode** — `crates/mev-engine/src/lib.rs:L41`
- **now_secs()** — `crates/mev-engine/src/lib.rs:L819`
- **recv_opt()** — `crates/mev-engine/src/lib.rs:L812`
- **short()** — `crates/mev-sim/src/lib.rs:L80`
- **.drain()** — `crates/mev-rpc/src/meter.rs:L40`
- **.gas_price()** — `crates/mev-engine/src/lib.rs:L334`
- **.layer()** — `crates/mev-rpc/src/meter.rs:L54`
- **.record()** — `crates/mev-rpc/src/meter.rs:L30`
- **.run()** — `crates/mev-engine/src/lib.rs:L226`
- **Arc** — `graph`
- **drain_sorts_by_count_and_resets()** — `crates/mev-rpc/src/meter.rs:L99`
- **Result** — `graph`
- **S** — `graph`
- **U256** — `graph`
- **UnboundedSender** — `graph`
- **Address** — `graph`
- **DynProvider** — `graph`
- **RequestPacket** — `graph`
- **Service** — `graph`
- **String** — `graph`
- **String** — `graph`
- **String** — `graph`
- **UnboundedReceiver** — `graph`
- **.log_summary()** — `crates/mev-engine/src/lib.rs:L356`
- **.pool_count()** — `crates/mev-engine/src/lib.rs:L329`
- **Context** — `graph`
- **DynProvider** — `graph`
- **Error** — `graph`
- **Future** — `graph`
- **HashMap** — `graph`
- **HashMap** — `graph`
- **Instant** — `graph`
- **Layer** — `graph`
- **Mutex** — `graph`
- **Path** — `graph`
- **PathBuf** — `graph`
- **Poll** — `graph`
- **Result** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **TxHash** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 8 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 7 cross-community edges
- [[docs/knowledge/communities/C021 - lib.rs (C021)|lib.rs (C021)]] — 6 cross-community edges
- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 5 cross-community edges
- [[docs/knowledge/communities/C054 - Plan|Plan]] — 4 cross-community edges
- [[docs/knowledge/communities/C053 - AccountRef|AccountRef]] — 3 cross-community edges
- [[docs/knowledge/communities/C027 - lib.rs (C027)|lib.rs (C027)]] — 2 cross-community edges
- [[docs/knowledge/communities/C029 - lib.rs (C029)|lib.rs (C029)]] — 2 cross-community edges
- [[docs/knowledge/communities/C056 - .plan|.plan]] — 2 cross-community edges
- [[docs/knowledge/communities/C042 - lib.rs (C042)|lib.rs (C042)]] — 1 cross-community edges
- [[docs/knowledge/communities/C055 - lib.rs (C055)|lib.rs (C055)]] — 1 cross-community edges
