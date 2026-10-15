---
graphify-generated: true
graphify-community: "72"
graphify-node-count: 30
graphify-internal-edges: 61
tags:
  - graphify/community
---

# lib.rs (C072)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-protocol/src/lib.rs` — 20 nodes
- `graph` — 6 nodes
- `crates/mev-protocol-pancake-v3/src/lib.rs` — 1 node
- `crates/mev-protocol-uniswap-v2/src/lib.rs` — 1 node
- `crates/mev-protocol-uniswap-v3/src/lib.rs` — 1 node
- `crates/mev-protocol-uniswap-v4/src/lib.rs` — 1 node

## Nodes (30)

- **lib.rs** — `crates/mev-protocol/src/lib.rs:L1`
- **LiquidityIntent** — `crates/mev-protocol/src/lib.rs:L129`
- **Address** — `graph`
- **Option** — `graph`
- **PriceUpdate** — `crates/mev-protocol/src/lib.rs:L57`
- **LiquidationIntent** — `crates/mev-protocol/src/lib.rs:L107`
- **sqrt_price_update()** — `crates/mev-protocol/src/lib.rs:L279`
- **uint()** — `crates/mev-protocol/src/lib.rs:L220`
- **.price_update()** — `crates/mev-protocol-pancake-v3/src/lib.rs:L31`
- **.price_update()** — `crates/mev-protocol-uniswap-v2/src/lib.rs:L133`
- **B256** — `graph`
- **reserves_update()** — `crates/mev-protocol/src/lib.rs:L291`
- **U256** — `graph`
- **addr()** — `crates/mev-protocol/src/lib.rs:L212`
- **intent()** — `crates/mev-protocol-uniswap-v4/src/lib.rs:L39`
- **liquidity()** — `crates/mev-protocol-uniswap-v3/src/lib.rs:L48`
- **LiquidityAction** — `crates/mev-protocol/src/lib.rs:L121`
- **pool_id()** — `crates/mev-protocol/src/lib.rs:L246`
- **PoolRef** — `crates/mev-protocol/src/lib.rs:L70`
- **PoolSet** — `crates/mev-protocol/src/lib.rs:L150`
- **word()** — `crates/mev-protocol/src/lib.rs:L207`
- **address_word()** — `crates/mev-protocol/src/lib.rs:L238`
- **stand_in()** — `crates/mev-protocol/src/lib.rs:L272`
- **split()** — `crates/mev-protocol/src/lib.rs:L225`
- **Vec** — `graph`
- **address_words_with_dirty_high_bytes_are_rejected()** — `crates/mev-protocol/src/lib.rs:L303`
- **Option** — `graph`
- **split_needs_a_selector()** — `crates/mev-protocol/src/lib.rs:L340`
- **sqrt_price_and_reserves_are_read_from_log_data()** — `crates/mev-protocol/src/lib.rs:L313`
- **v4_pool_ids_match_ids_seen_in_real_swap_events()** — `crates/mev-protocol/src/lib.rs:L349`

## Related communities

- [[docs/knowledge/communities/C102 - EventKind|EventKind]] — 12 cross-community edges
- [[docs/knowledge/communities/C103 - SwapIntent|SwapIntent]] — 5 cross-community edges
- [[docs/knowledge/communities/C157 - UniswapV4|UniswapV4]] — 5 cross-community edges
- [[docs/knowledge/communities/C114 - Contract|Contract]] — 4 cross-community edges
- [[docs/knowledge/communities/C136 - Decoded|Decoded]] — 4 cross-community edges
- [[docs/knowledge/communities/C113 - Protocol|Protocol]] — 2 cross-community edges
- [[docs/knowledge/communities/C189 - main.rs (C189)|main.rs (C189)]] — 2 cross-community edges
- [[docs/knowledge/communities/C091 - events.rs|events.rs]] — 1 cross-community edges
