---
graphify-generated: true
graphify-community: "51"
graphify-node-count: 38
graphify-internal-edges: 98
tags:
  - graphify/community
---

# Rpc

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-discovery/src/lib.rs` — 22 nodes
- `graph` — 9 nodes
- `crates/mev-discovery/tests/exact_camelot_live.rs` — 2 nodes
- `crates/mev-discovery/tests/v4_live.rs` — 2 nodes
- `crates/mev-dex/src/lib.rs` — 1 node
- `crates/mev-engine/src/lib.rs` — 1 node
- `crates/mev-rpc/src/lib.rs` — 1 node

## Nodes (38)

- **Rpc** — `crates/mev-rpc/src/lib.rs:L21`
- **lib.rs** — `crates/mev-discovery/src/lib.rs:L1`
- **load_pools_at()** — `crates/mev-discovery/src/lib.rs:L176`
- **load_pools()** — `crates/mev-discovery/src/lib.rs:L171`
- **Address** — `graph`
- **Vec** — `graph`
- **algebra_max_fees()** — `crates/mev-discovery/src/lib.rs:L314`
- **Result** — `graph`
- **Snapshot** — `crates/mev-discovery/src/lib.rs:L412`
- **active_tokens()** — `crates/mev-discovery/src/lib.rs:L481`
- **candidate_pairs()** — `crates/mev-discovery/src/lib.rs:L576`
- **split_store()** — `crates/mev-discovery/src/lib.rs:L649`
- **build_store()** — `crates/mev-discovery/src/lib.rs:L638`
- **discover_pools()** — `crates/mev-discovery/src/lib.rs:L38`
- **v4_pool()** — `crates/mev-discovery/src/lib.rs:L371`
- **balances()** — `crates/mev-discovery/src/lib.rs:L616`
- **sqrt_price_from_x96()** — `crates/mev-dex/src/lib.rs:L681`
- **calls_for()** — `crates/mev-discovery/src/lib.rs:L132`
- **cross_pairs()** — `crates/mev-discovery/src/lib.rs:L560`
- **refresh()** — `crates/mev-discovery/src/lib.rs:L455`
- **token_decimals()** — `crates/mev-discovery/src/lib.rs:L603`
- **refresh_ticks()** — `crates/mev-discovery/src/lib.rs:L466`
- **v2()** — `crates/mev-discovery/src/lib.rs:L708`
- **a()** — `crates/mev-discovery/src/lib.rs:L770`
- **build_store_drops_illiquid_and_single_pool_pairs()** — `crates/mev-discovery/src/lib.rs:L726`
- **cross_pairs_skip_bases_and_take_the_most_active_first()** — `crates/mev-discovery/src/lib.rs:L775`
- **HashMap** — `graph`
- **Option** — `graph`
- **B256** — `graph`
- **BlockId** — `graph`
- **Bytes** — `graph`
- **camelot_integer_math_equals_the_pair()** — `crates/mev-discovery/tests/exact_camelot_live.rs:L31`
- **failed_pin_disables_aave_only_in_live_mode()** — `crates/mev-engine/src/lib.rs:L3705`
- **our_v4_model_prices_swaps_like_the_official_quoter()** — `crates/mev-discovery/tests/v4_live.rs:L29`
- **split_store_returns_the_dropped_pools_as_the_watch_tier()** — `crates/mev-discovery/src/lib.rs:L741`
- **exact_camelot_live.rs** — `crates/mev-discovery/tests/exact_camelot_live.rs:L1`
- **U256** — `graph`
- **v4_live.rs** — `crates/mev-discovery/tests/v4_live.rs:L1`

## Related communities

- [[docs/knowledge/communities/C047 - lib.rs (C047)|lib.rs (C047)]] — 10 cross-community edges
- [[docs/knowledge/communities/C026 - load_ticks_at|load_ticks_at]] — 8 cross-community edges
- [[docs/knowledge/communities/C032 - Pool|Pool]] — 8 cross-community edges
- [[docs/knowledge/communities/C045 - PoolInfo|PoolInfo]] — 8 cross-community edges
- [[docs/knowledge/communities/C025 - PoolStore|PoolStore]] — 7 cross-community edges
- [[docs/knowledge/communities/C108 - ChainConfig|ChainConfig]] — 4 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 4 cross-community edges
- [[docs/knowledge/communities/C109 - Hub|Hub]] — 3 cross-community edges
- [[docs/knowledge/communities/C269 - applying_a_real_modify_liquidity_log_reproduces_the_managers_liquidity|applying_a_real_modify_liquidity_log_reproduces_the_managers_liquidity]] — 2 cross-community edges
- [[docs/knowledge/communities/C280 - chain_state|chain_state]] — 2 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C298 - Pool_flat|Pool_flat]] — 1 cross-community edges
