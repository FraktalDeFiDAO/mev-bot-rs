---
graphify-generated: true
graphify-community: "25"
graphify-node-count: 50
graphify-internal-edges: 149
tags:
  - graphify/community
---

# PoolStore

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-state/src/lib.rs` — 37 nodes
- `graph` — 13 nodes

## Nodes (50)

- **PoolStore** — `crates/mev-state/src/lib.rs:L52`
- **Address** — `graph`
- **.get()** — `crates/mev-state/src/lib.rs:L233`
- **Option** — `graph`
- **Log** — `graph`
- **.apply_log_tentative()** — `crates/mev-state/src/lib.rs:L651`
- **.apply_refresh_inner()** — `crates/mev-state/src/lib.rs:L196`
- **.expire_tentative()** — `crates/mev-state/src/lib.rs:L667`
- **.apply_coherent_refresh()** — `crates/mev-state/src/lib.rs:L188`
- **.apply_log_core()** — `crates/mev-state/src/lib.rs:L711`
- **.apply_log_inner()** — `crates/mev-state/src/lib.rs:L743`
- **.bump()** — `crates/mev-state/src/lib.rs:L152`
- **Vec** — `graph`
- **.pools_for()** — `crates/mev-state/src/lib.rs:L282`
- **.apply_refresh()** — `crates/mev-state/src/lib.rs:L179`
- **.apply_v4_liquidity()** — `crates/mev-state/src/lib.rs:L457`
- **.apply_v3_liquidity()** — `crates/mev-state/src/lib.rs:L333`
- **.apply_v4_swap()** — `crates/mev-state/src/lib.rs:L572`
- **.log_pool()** — `crates/mev-state/src/lib.rs:L626`
- **.replace()** — `crates/mev-state/src/lib.rs:L290`
- **.usd_price_integer()** — `crates/mev-state/src/lib.rs:L831`
- **a_float_state_write_and_a_rollback_keep_exact_state_honest()** — `crates/mev-state/src/lib.rs:L1997`
- **a_resync_or_replacement_confirms_too_and_two_pending_txs_keep_the_first_confirmed_state()** — `crates/mev-state/src/lib.rs:L1792`
- **Overwrite** — `crates/mev-state/src/lib.rs:L35`
- **RefreshOutcome** — `crates/mev-state/src/lib.rs:L23`
- **.rate()** — `crates/mev-state/src/lib.rs:L861`
- **.siblings()** — `crates/mev-state/src/lib.rs:L270`
- **HashMap** — `graph`
- **PoolState** — `graph`
- **.set_state()** — `crates/mev-state/src/lib.rs:L317`
- **.usd_per_raw_unit()** — `crates/mev-state/src/lib.rs:L810`
- **v3_swap_state()** — `crates/mev-state/src/lib.rs:L107`
- **.log_addresses()** — `crates/mev-state/src/lib.rs:L251`
- **.set_ticks()** — `crates/mev-state/src/lib.rs:L307`
- **pair_key()** — `crates/mev-state/src/lib.rs:L133`
- **Rollback** — `crates/mev-state/src/lib.rs:L13`
- **.addresses()** — `crates/mev-state/src/lib.rs:L245`
- **.submission_revision()** — `crates/mev-state/src/lib.rs:L159`
- **materially_different()** — `crates/mev-state/src/lib.rs:L76`
- **.has_token()** — `crates/mev-state/src/lib.rs:L278`
- **Arc** — `graph`
- **AtomicU64** — `graph`
- **Instant** — `graph`
- **U256** — `graph`
- **.is_empty()** — `crates/mev-state/src/lib.rs:L241`
- **.len()** — `crates/mev-state/src/lib.rs:L237`
- **.tentative_len()** — `crates/mev-state/src/lib.rs:L706`
- **Confirmed** — `graph`
- **Duration** — `graph`
- **HashSet** — `graph`

## Related communities

- [[docs/knowledge/communities/C030 - lib.rs (C030)|lib.rs (C030)]] — 75 cross-community edges
- [[docs/knowledge/communities/C032 - Pool|Pool]] — 10 cross-community edges
- [[docs/knowledge/communities/C051 - Rpc|Rpc]] — 7 cross-community edges
- [[docs/knowledge/communities/C163 - Opportunity|Opportunity]] — 5 cross-community edges
- [[docs/knowledge/communities/C104 - CycleIndex|CycleIndex]] — 4 cross-community edges
- [[docs/knowledge/communities/C124 - lib.rs (C124)|lib.rs (C124)]] — 4 cross-community edges
- [[docs/knowledge/communities/C078 - pending.rs|pending.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C096 - decode_check.rs|decode_check.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 2 cross-community edges
- [[docs/knowledge/communities/C247 - lib.rs (C247)|lib.rs (C247)]] — 2 cross-community edges
- [[docs/knowledge/communities/C092 - event_line|event_line]] — 2 cross-community edges
- [[docs/knowledge/communities/C158 - Ratio|Ratio]] — 1 cross-community edges
