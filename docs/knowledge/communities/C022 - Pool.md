---
graphify-generated: true
graphify-community: "22"
graphify-node-count: 33
graphify-internal-edges: 60
tags:
  - graphify/community
---

# Pool

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-dex/src/lib.rs` — 16 nodes
- `crates/mev-types/src/lib.rs` — 7 nodes
- `graph` — 6 nodes
- `crates/mev-bindings/src/lib.rs` — 2 nodes
- `crates/mev-state/src/lib.rs` — 2 nodes

## Nodes (33)

- **Pool** — `crates/mev-dex/src/lib.rs:L20`
- **PoolInfo** — `crates/mev-types/src/lib.rs:L34`
- **lib.rs** — `crates/mev-dex/src/lib.rs:L1`
- **.new()** — `crates/mev-dex/src/lib.rs:L29`
- **.amount_out()** — `crates/mev-dex/src/lib.rs:L78`
- **Address** — `graph`
- **info()** — `crates/mev-dex/src/lib.rs:L131`
- **.reserves()** — `crates/mev-dex/src/lib.rs:L52`
- **Address** — `graph`
- **PoolState** — `crates/mev-dex/src/lib.rs:L11`
- **.fee()** — `crates/mev-dex/src/lib.rs:L69`
- **.marginal_rate()** — `crates/mev-dex/src/lib.rs:L88`
- **.pools()** — `crates/mev-state/src/lib.rs:L45`
- **Hop** — `crates/mev-types/src/lib.rs:L58`
- **lib.rs** — `crates/mev-types/src/lib.rs:L1`
- **PoolKind** — `crates/mev-types/src/lib.rs:L8`
- **v2_matches_uniswap_formula()** — `crates/mev-dex/src/lib.rs:L142`
- **v3_virtual_reserves_give_spot_price()** — `crates/mev-dex/src/lib.rs:L157`
- **.reserve_of()** — `crates/mev-dex/src/lib.rs:L112`
- **empty_pool_returns_zero()** — `crates/mev-dex/src/lib.rs:L172`
- **sqrt_price_from_x96()** — `crates/mev-dex/src/lib.rs:L119`
- **.has()** — `crates/mev-types/src/lib.rs:L51`
- **.other()** — `crates/mev-types/src/lib.rs:L43`
- **.replace()** — `crates/mev-state/src/lib.rs:L65`
- **.with_directional_fees()** — `crates/mev-dex/src/lib.rs:L45`
- **Self** — `graph`
- **to_sol_hop()** — `crates/mev-bindings/src/lib.rs:L94`
- **.hop_kind()** — `crates/mev-types/src/lib.rs:L23`
- **.price0()** — `crates/mev-dex/src/lib.rs:L98`
- **Item** — `graph`
- **Iterator** — `graph`
- **lib.rs** — `crates/mev-bindings/src/lib.rs:L1`
- **String** — `graph`

## Related communities

- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 9 cross-community edges
- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 6 cross-community edges
- [[docs/knowledge/communities/C031 - lib.rs (C031)|lib.rs (C031)]] — 4 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 3 cross-community edges
