---
graphify-generated: true
graphify-community: "78"
graphify-node-count: 28
graphify-internal-edges: 65
tags:
  - graphify/community
---

# pending.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-engine/src/pending.rs` — 21 nodes
- `graph` — 7 nodes

## Nodes (28)

- **pending.rs** — `crates/mev-engine/src/pending.rs:L1`
- **PendingSwaps** — `crates/mev-engine/src/pending.rs:L43`
- **.insert()** — `crates/mev-engine/src/pending.rs:L58`
- **Address** — `graph`
- **DecodedSwap** — `crates/mev-engine/src/pending.rs:L28`
- **B256** — `graph`
- **h()** — `crates/mev-engine/src/pending.rs:L207`
- **Instant** — `graph`
- **Vec** — `graph`
- **.conflict()** — `crates/mev-engine/src/pending.rs:L99`
- **pools_by_token_pairs()** — `crates/mev-engine/src/pending.rs:L144`
- **resolve_pools()** — `crates/mev-engine/src/pending.rs:L179`
- **.prune()** — `crates/mev-engine/src/pending.rs:L107`
- **a()** — `crates/mev-engine/src/pending.rs:L210`
- **an_entry_that_nothing_settles_expires()** — `crates/mev-engine/src/pending.rs:L324`
- **token_pairs_and_fee_tiers_select_the_pools_of_a_derived_program()** — `crates/mev-engine/src/pending.rs:L215`
- **watched_pools_in()** — `crates/mev-engine/src/pending.rs:L128`
- **.settle()** — `crates/mev-engine/src/pending.rs:L76`
- **a_transaction_with_several_swaps_blocks_the_union_of_their_pools()** — `crates/mev-engine/src/pending.rs:L293`
- **an_open_entry_blocks_trades_on_its_pools_only()** — `crates/mev-engine/src/pending.rs:L284`
- **settling_clears_the_entry_and_an_unknown_hash_is_harmless()** — `crates/mev-engine/src/pending.rs:L312`
- **the_scan_finds_exactly_the_watched_pools_in_the_calldata()** — `crates/mev-engine/src/pending.rs:L252`
- **.open_count()** — `crates/mev-engine/src/pending.rs:L114`
- **Option** — `graph`
- **.is_empty()** — `crates/mev-engine/src/pending.rs:L122`
- **.take_counts()** — `crates/mev-engine/src/pending.rs:L90`
- **AtomicU64** — `graph`
- **HashMap** — `graph`

## Related communities

- [[docs/knowledge/communities/C025 - PoolStore|PoolStore]] — 4 cross-community edges
- [[docs/knowledge/communities/C103 - SwapIntent|SwapIntent]] — 2 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 2 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 2 cross-community edges
