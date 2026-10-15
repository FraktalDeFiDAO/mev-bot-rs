---
graphify-generated: true
graphify-community: "18"
graphify-node-count: 53
graphify-internal-edges: 98
tags:
  - graphify/community
---

# BorrowerSnapshot

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 18 nodes
- `crates/mev-liq-state/src/domain.rs` — 14 nodes
- `crates/mev-liq-state/src/coalesce.rs` — 11 nodes
- `crates/mev-lending-aave-v3/src/recheck.rs` — 7 nodes
- `crates/mev-liq-sched/tests/advisory.rs` — 1 node
- `crates/mev-liq-state/src/lib.rs` — 1 node
- `crates/seqscope-cli/src/main.rs` — 1 node

## Nodes (53)

- **BorrowerSnapshot** — `crates/mev-liq-state/src/domain.rs:L125`
- **DirtyReason** — `crates/mev-liq-state/src/domain.rs:L53`
- **Coalescer** — `crates/mev-liq-state/src/coalesce.rs:L23`
- **domain.rs** — `crates/mev-liq-state/src/domain.rs:L1`
- **.recheck_many()** — `crates/mev-lending-aave-v3/src/recheck.rs:L103`
- **AaveAccountRechecker** — `crates/mev-lending-aave-v3/src/recheck.rs:L29`
- **.recheck()** — `crates/mev-lending-aave-v3/src/recheck.rs:L47`
- **.snapshot()** — `crates/mev-lending-aave-v3/src/recheck.rs:L63`
- **ChainRef** — `crates/mev-liq-state/src/domain.rs:L38`
- **ReserveBalance** — `crates/mev-liq-state/src/domain.rs:L110`
- **.push_asset()** — `crates/mev-liq-state/src/coalesce.rs:L99`
- **.recheck_at_head()** — `crates/mev-lending-aave-v3/src/recheck.rs:L157`
- **coalesce.rs** — `crates/mev-liq-state/src/coalesce.rs:L1`
- **later()** — `crates/mev-liq-state/src/coalesce.rs:L32`
- **.compute_digest()** — `crates/mev-liq-state/src/domain.rs:L155`
- **.push_account()** — `crates/mev-liq-state/src/coalesce.rs:L58`
- **Address** — `graph`
- **Slot** — `crates/mev-liq-state/src/coalesce.rs:L14`
- **Address** — `graph`
- **B256** — `graph`
- **now_ns()** — `crates/mev-lending-aave-v3/src/recheck.rs:L37`
- **Option** — `graph`
- **.from_code()** — `crates/mev-liq-state/src/domain.rs:L27`
- **.from_code()** — `crates/mev-liq-state/src/domain.rs:L92`
- **.into_effects()** — `crates/mev-liq-state/src/coalesce.rs:L118`
- **.seal()** — `crates/mev-liq-state/src/domain.rs:L185`
- **Option** — `graph`
- **reason()** — `crates/mev-liq-sched/tests/advisory.rs:L562`
- **Result** — `graph`
- **Self** — `graph`
- **.code()** — `crates/mev-liq-state/src/domain.rs:L65`
- **.digest_matches()** — `crates/mev-liq-state/src/domain.rs:L190`
- **.new()** — `crates/mev-liq-state/src/coalesce.rs:L47`
- **a_broken_edit_keeps_the_previous_rules_and_a_fixed_one_reloads()** — `crates/seqscope-cli/src/main.rs:L961`
- **Address** — `graph`
- **BTreeMap** — `graph`
- **lib.rs** — `crates/mev-liq-state/src/lib.rs:L1`
- **recheck.rs** — `crates/mev-lending-aave-v3/src/recheck.rs:L1`
- **SnapshotStatus** — `crates/mev-liq-state/src/domain.rs:L117`
- **U256** — `graph`
- **.eligible()** — `crates/mev-liq-state/src/domain.rs:L147`
- **.is_empty()** — `crates/mev-liq-state/src/coalesce.rs:L112`
- **.len()** — `crates/mev-liq-state/src/coalesce.rs:L108`
- **.overflowed()** — `crates/mev-liq-state/src/coalesce.rs:L104`
- **.urgency()** — `crates/mev-liq-state/src/domain.rs:L81`
- **DynProvider** — `graph`
- **getUserAccountDataReturn** — `graph`
- **Option** — `graph`
- **Self** — `graph`
- **String** — `graph`
- **Vec** — `graph`
- **Vec** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 17 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 12 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 8 cross-community edges
- [[docs/knowledge/communities/C006 - Result|Result]] — 5 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 5 cross-community edges
- [[docs/knowledge/communities/C048 - FeatureV1|FeatureV1]] — 4 cross-community edges
- [[docs/knowledge/communities/C233 - trend_with|trend_with]] — 3 cross-community edges
- [[docs/knowledge/communities/C040 - sched.rs|sched.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C044 - liq_shadow.rs|liq_shadow.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C143 - addr_of|addr_of]] — 2 cross-community edges
- [[docs/knowledge/communities/C067 - features.rs (C067)|features.rs (C067)]] — 2 cross-community edges
- [[docs/knowledge/communities/C071 - replay_with_policy|replay_with_policy]] — 2 cross-community edges
