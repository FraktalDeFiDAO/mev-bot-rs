---
graphify-generated: true
graphify-community: "7"
graphify-node-count: 74
graphify-internal-edges: 167
tags:
  - graphify/community
---

# ScheduleOutput

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-sched/src/types.rs` — 25 nodes
- `graph` — 23 nodes
- `crates/mev-liq-sched/src/policy.rs` — 15 nodes
- `crates/mev-liq-sched/src/advisory.rs` — 9 nodes
- `crates/mev-liq-sched/src/lib.rs` — 1 node
- `crates/mev-liq-sched/tests/props.rs` — 1 node

## Nodes (74)

- **ScheduleOutput** — `crates/mev-liq-sched/src/types.rs:L247`
- **PolicyConfig** — `crates/mev-liq-sched/src/types.rs:L284`
- **types.rs** — `crates/mev-liq-sched/src/types.rs:L1`
- **would_be_schedule()** — `crates/mev-liq-sched/src/advisory.rs:L366`
- **OptionalDecision** — `crates/mev-liq-sched/src/types.rs:L110`
- **ScheduleInput** — `crates/mev-liq-sched/src/types.rs:L150`
- **ScheduleError** — `crates/mev-liq-sched/src/types.rs:L269`
- **policy.rs** — `crates/mev-liq-sched/src/policy.rs:L1`
- **schedule_inner()** — `crates/mev-liq-sched/src/policy.rs:L99`
- **Result** — `graph`
- **.run()** — `crates/mev-liq-sched/src/advisory.rs:L588`
- **evaluate()** — `crates/mev-liq-sched/src/policy.rs:L436`
- **schedule_with_advisory()** — `crates/mev-liq-sched/src/policy.rs:L91`
- **Serialize** — `graph`
- **schedule()** — `crates/mev-liq-sched/src/policy.rs:L80`
- **.schedule()** — `crates/mev-liq-sched/src/advisory.rs:L615`
- **.schedule()** — `crates/mev-liq-sched/src/advisory.rs:L642`
- **.validate()** — `crates/mev-liq-sched/src/advisory.rs:L180`
- **Tier** — `crates/mev-liq-sched/src/types.rs:L23`
- **UrgentEvent** — `crates/mev-liq-sched/src/types.rs:L57`
- **.schedule()** — `crates/mev-liq-sched/src/policy.rs:L27`
- **Acc** — `crates/mev-liq-sched/src/policy.rs:L47`
- **AccountState** — `crates/mev-liq-sched/src/types.rs:L66`
- **PoolState** — `crates/mev-liq-sched/src/types.rs:L91`
- **ScheduleDecision** — `crates/mev-liq-sched/src/types.rs:L171`
- **ser_key()** — `crates/mev-liq-sched/src/types.rs:L10`
- **ser_reason()** — `crates/mev-liq-sched/src/types.rs:L17`
- **.new()** — `crates/mev-liq-sched/src/types.rs:L78`
- **DeadlineMiss** — `crates/mev-liq-sched/src/types.rs:L192`
- **Ev** — `crates/mev-liq-sched/src/policy.rs:L41`
- **RankingPolicy** — `crates/mev-liq-sched/src/policy.rs:L16`
- **StaleRejection** — `crates/mev-liq-sched/src/types.rs:L218`
- **TierCounts** — `crates/mev-liq-sched/src/types.rs:L228`
- **.validate()** — `crates/mev-liq-sched/src/types.rs:L335`
- **BaselinePolicy** — `crates/mev-liq-sched/src/policy.rs:L22`
- **Option** — `graph`
- **Option** — `graph`
- **pick()** — `crates/mev-liq-sched/src/policy.rs:L401`
- **Reason** — `crates/mev-liq-sched/src/policy.rs:L33`
- **ReasonCode** — `crates/mev-liq-sched/src/types.rs:L32`
- **Result** — `graph`
- **Result** — `graph`
- **SourceVersion** — `crates/mev-liq-sched/src/types.rs:L162`
- **sweep_capacity()** — `crates/mev-liq-sched/src/policy.rs:L384`
- **valid_answer()** — `crates/mev-liq-sched/src/advisory.rs:L225`
- **.fmt()** — `crates/mev-liq-sched/src/advisory.rs:L49`
- **.fmt()** — `crates/mev-liq-sched/src/advisory.rs:L529`
- **.fmt()** — `crates/mev-liq-sched/src/types.rs:L274`
- **.from_stored()** — `crates/mev-liq-sched/src/types.rs:L131`
- **CoverageViolation** — `crates/mev-liq-sched/src/types.rs:L202`
- **Error** — `graph`
- **ExportView** — `graph`
- **newer()** — `crates/mev-liq-sched/src/policy.rs:L55`
- **Ok** — `graph`
- **Self** — `graph`
- **Vec** — `graph`
- **Vec** — `graph`
- **.add()** — `crates/mev-liq-sched/src/types.rs:L236`
- **.default()** — `crates/mev-liq-sched/src/types.rs:L312`
- **Address** — `graph`
- **B256** — `graph`
- **BonusFn** — `graph`
- **Formatter** — `graph`
- **lib.rs** — `crates/mev-liq-sched/src/lib.rs:L1`
- **ReplayAdvisoryPolicy<F>** — `crates/mev-liq-sched/src/advisory.rs:L638`
- **S** — `graph`
- **share_ceil()** — `crates/mev-liq-sched/src/policy.rs:L393`
- **small_cfg()** — `crates/mev-liq-sched/tests/props.rs:L160`
- **Default** — `graph`
- **Display** — `graph`
- **Formatter** — `graph`
- **String** — `graph`
- **U256** — `graph`
- **WatchTier** — `graph`

## Related communities

- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 42 cross-community edges
- [[docs/knowledge/communities/C042 - advisory.rs (C042)|advisory.rs (C042)]] — 15 cross-community edges
- [[docs/knowledge/communities/C040 - sched.rs|sched.rs]] — 10 cross-community edges
- [[docs/knowledge/communities/C029 - ShadowSchedule|ShadowSchedule]] — 9 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 8 cross-community edges
- [[docs/knowledge/communities/C071 - replay_with_policy|replay_with_policy]] — 8 cross-community edges
- [[docs/knowledge/communities/C179 - World|World]] — 5 cross-community edges
- [[docs/knowledge/communities/C060 - mod.rs (C060)|mod.rs (C060)]] — 5 cross-community edges
- [[docs/knowledge/communities/C012 - ChainPoint|ChainPoint]] — 4 cross-community edges
- [[docs/knowledge/communities/C150 - lib.rs (C150)|lib.rs (C150)]] — 4 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 4 cross-community edges
- [[docs/knowledge/communities/C142 - advisory_store.rs|advisory_store.rs]] — 2 cross-community edges
