---
graphify-generated: true
graphify-community: "23"
graphify-node-count: 44
graphify-internal-edges: 78
tags:
  - graphify/community
---

# lib.rs (C023)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 19 nodes
- `crates/mev-exec/src/lib.rs` — 18 nodes
- `crates/mev-bindings/src/lib.rs` — 5 nodes
- `crates/mev-types/src/lib.rs` — 2 nodes

## Nodes (44)

- **Sender** — `crates/mev-exec/src/lib.rs:L23`
- **Opportunity** — `crates/mev-types/src/lib.rs:L69`
- **lib.rs** — `crates/mev-exec/src/lib.rs:L1`
- **.send_calldata()** — `crates/mev-exec/src/lib.rs:L102`
- **.send_liquidation()** — `crates/mev-exec/src/lib.rs:L90`
- **Hop** — `crates/mev-types/src/lib.rs:L58`
- **futures_join_all()** — `crates/mev-exec/src/lib.rs:L223`
- **liquidate_calldata()** — `crates/mev-exec/src/lib.rs:L211`
- **Result** — `graph`
- **.new()** — `crates/mev-exec/src/lib.rs:L43`
- **.send()** — `crates/mev-exec/src/lib.rs:L73`
- **.sign_and_broadcast()** — `crates/mev-exec/src/lib.rs:L125`
- **.wait()** — `crates/mev-exec/src/lib.rs:L164`
- **execute_calldata()** — `crates/mev-exec/src/lib.rs:L200`
- **TxHash** — `graph`
- **U256** — `graph`
- **Vec** — `graph`
- **Landed** — `crates/mev-exec/src/lib.rs:L34`
- **lib.rs** — `crates/mev-bindings/src/lib.rs:L1`
- **.eth_balance()** — `crates/mev-exec/src/lib.rs:L68`
- **hop_and_call_conversions_preserve_fields()** — `crates/mev-bindings/src/lib.rs:L162`
- **is_nonce_error()** — `crates/mev-exec/src/lib.rs:L191`
- **liquidate_calldata_decodes_to_the_plan()** — `crates/mev-exec/src/lib.rs:L283`
- **to_sol_call()** — `crates/mev-bindings/src/lib.rs:L144`
- **to_sol_hop()** — `crates/mev-bindings/src/lib.rs:L133`
- **Address** — `graph`
- **DynProvider** — `graph`
- **execute_calldata_decodes_to_the_opportunity()** — `crates/mev-exec/src/lib.rs:L263`
- **hop()** — `crates/mev-exec/src/lib.rs:L239`
- **AtomicU64** — `graph`
- **Duration** — `graph`
- **Error** — `graph`
- **EthereumWallet** — `graph`
- **F** — `graph`
- **IntoIterator** — `graph`
- **Item** — `graph`
- **liquidate_calldata_round_trips_through_the_abi()** — `crates/mev-bindings/src/lib.rs:L202`
- **Option** — `graph`
- **Output** — `graph`
- **recognizes_nonce_errors_from_nodes()** — `crates/mev-exec/src/lib.rs:L251`
- **Self** — `graph`
- **String** — `graph`
- **U256** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 7 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 7 cross-community edges
- [[docs/knowledge/communities/C025 - lib.rs (C025)|lib.rs (C025)]] — 4 cross-community edges
