---
graphify-generated: true
graphify-community: "30"
graphify-node-count: 80
graphify-internal-edges: 164
tags:
  - graphify/community
---

# lib.rs (C030)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 37 nodes
- `crates/mev-rpc/src/lib.rs` — 19 nodes
- `crates/mev-discovery/src/lib.rs` — 12 nodes
- `crates/mev-rpc/src/meter.rs` — 12 nodes

## Nodes (80)

- **Rpc** — `crates/mev-rpc/src/lib.rs:L21`
- **RpcMeter** — `crates/mev-rpc/src/meter.rs:L21`
- **lib.rs** — `crates/mev-rpc/src/lib.rs:L1`
- **lib.rs** — `crates/mev-discovery/src/lib.rs:L1`
- **metered_rotating_provider()** — `crates/mev-rpc/src/lib.rs:L148`
- **multicall()** — `crates/mev-rpc/src/lib.rs:L94`
- **active_tokens()** — `crates/mev-discovery/src/lib.rs:L251`
- **Address** — `graph`
- **load_pools()** — `crates/mev-discovery/src/lib.rs:L152`
- **metered_http_provider()** — `crates/mev-rpc/src/lib.rs:L140`
- **MeterLayer** — `crates/mev-rpc/src/meter.rs:L49`
- **Result** — `graph`
- **.connect()** — `crates/mev-rpc/src/lib.rs:L34`
- **.multicall()** — `crates/mev-rpc/src/lib.rs:L84`
- **candidate_pairs()** — `crates/mev-discovery/src/lib.rs:L328`
- **discover_pools()** — `crates/mev-discovery/src/lib.rs:L33`
- **Result** — `graph`
- **.default()** — `crates/mev-rpc/src/lib.rs:L177`
- **Backoff** — `crates/mev-rpc/src/lib.rs:L172`
- **balances()** — `crates/mev-discovery/src/lib.rs:L367`
- **build_store()** — `crates/mev-discovery/src/lib.rs:L386`
- **DynProvider** — `graph`
- **Vec** — `graph`
- **.poll_ready()** — `crates/mev-rpc/src/meter.rs:L82`
- **call()** — `crates/mev-rpc/src/lib.rs:L219`
- **calls_for()** — `crates/mev-discovery/src/lib.rs:L127`
- **connect_ws()** — `crates/mev-rpc/src/lib.rs:L162`
- **meter.rs** — `crates/mev-rpc/src/meter.rs:L1`
- **refresh()** — `crates/mev-discovery/src/lib.rs:L238`
- **retry_forever()** — `crates/mev-rpc/src/lib.rs:L200`
- **token_decimals()** — `crates/mev-discovery/src/lib.rs:L354`
- **.call()** — `crates/mev-rpc/src/meter.rs:L86`
- **.new()** — `crates/mev-rpc/src/meter.rs:L26`
- **.next_delay()** — `crates/mev-rpc/src/lib.rs:L188`
- **backoff_doubles_caps_and_resets()** — `crates/mev-rpc/src/lib.rs:L233`
- **Bytes** — `graph`
- **decode()** — `crates/mev-rpc/src/lib.rs:L224`
- **MeterService** — `crates/mev-rpc/src/meter.rs:L63`
- **MeterService<S>** — `crates/mev-rpc/src/meter.rs:L68`
- **v2()** — `crates/mev-discovery/src/lib.rs:L440`
- **.drain()** — `crates/mev-rpc/src/meter.rs:L40`
- **.layer()** — `crates/mev-rpc/src/meter.rs:L54`
- **.reconnect_ws()** — `crates/mev-rpc/src/lib.rs:L65`
- **.record()** — `crates/mev-rpc/src/meter.rs:L30`
- **Address** — `graph`
- **Arc** — `graph`
- **Arc** — `graph`
- **build_store_drops_illiquid_and_single_pool_pairs()** — `crates/mev-discovery/src/lib.rs:L458`
- **drain_sorts_by_count_and_resets()** — `crates/mev-rpc/src/meter.rs:L99`
- **http_provider()** — `crates/mev-rpc/src/lib.rs:L156`
- **Option** — `graph`
- **S** — `graph`
- **String** — `graph`
- **Vec** — `graph`
- **.reset()** — `crates/mev-rpc/src/lib.rs:L194`
- **Duration** — `graph`
- **HashMap** — `graph`
- **is_rate_limit()** — `crates/mev-rpc/src/lib.rs:L134`
- **RequestPacket** — `graph`
- **Self** — `graph`
- **Service** — `graph`
- **String** — `graph`
- **Bytes** — `graph`
- **C** — `graph`
- **Context** — `graph`
- **Default** — `graph`
- **Error** — `graph`
- **Error** — `graph`
- **F** — `graph`
- **Future** — `graph`
- **HashMap** — `graph`
- **Layer** — `graph`
- **Mutex** — `graph`
- **Poll** — `graph`
- **Result** — `graph`
- **Return** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **U256** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C034 - Engine|Engine]] — 8 cross-community edges
- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 7 cross-community edges
- [[docs/knowledge/communities/C021 - lib.rs (C021)|lib.rs (C021)]] — 4 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 3 cross-community edges
