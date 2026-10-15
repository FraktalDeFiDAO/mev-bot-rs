---
graphify-generated: true
graphify-community: "110"
graphify-node-count: 21
graphify-internal-edges: 44
tags:
  - graphify/community
---

# nonce.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-exec/src/nonce.rs` — 17 nodes
- `graph` — 4 nodes

## Nodes (21)

- **.new()** — `crates/mev-exec/src/nonce.rs:L22`
- **nonce.rs** — `crates/mev-exec/src/nonce.rs:L1`
- **NonceManager** — `crates/mev-exec/src/nonce.rs:L19`
- **.release()** — `crates/mev-exec/src/nonce.rs:L45`
- **.take()** — `crates/mev-exec/src/nonce.rs:L32`
- **concurrent_sends_with_failures_never_share_a_live_nonce_or_leave_a_gap()** — `crates/mev-exec/src/nonce.rs:L157`
- **reset_follows_the_chain_and_forgets_released_nonces()** — `crates/mev-exec/src/nonce.rs:L140`
- **a_stale_chain_refresh_preserves_held_nonces()** — `crates/mev-exec/src/nonce.rs:L95`
- **releasing_an_earlier_nonce_is_reused_before_a_new_one_so_no_collision_no_gap()** — `crates/mev-exec/src/nonce.rs:L118`
- **releasing_out_of_order_collapses_back_to_a_clean_range()** — `crates/mev-exec/src/nonce.rs:L130`
- **releasing_the_latest_nonce_just_rewinds()** — `crates/mev-exec/src/nonce.rs:L109`
- **.reset()** — `crates/mev-exec/src/nonce.rs:L63`
- **.snapshot()** — `crates/mev-exec/src/nonce.rs:L83`
- **releasing_a_nonce_that_was_never_handed_out_is_ignored()** — `crates/mev-exec/src/nonce.rs:L150`
- **State** — `crates/mev-exec/src/nonce.rs:L9`
- **hands_out_consecutive_nonces()** — `crates/mev-exec/src/nonce.rs:L103`
- **.hold()** — `crates/mev-exec/src/nonce.rs:L73`
- **BTreeSet** — `graph`
- **Mutex** — `graph`
- **Self** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C149 - Sender|Sender]] — 1 cross-community edges
