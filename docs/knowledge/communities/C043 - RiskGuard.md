---
graphify-generated: true
graphify-community: "43"
graphify-node-count: 41
graphify-internal-edges: 84
tags:
  - graphify/community
---

# RiskGuard

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-risk/src/lib.rs` — 34 nodes
- `graph` — 7 nodes

## Nodes (41)

- **RiskGuard** — `crates/mev-risk/src/lib.rs:L70`
- **lib.rs** — `crates/mev-risk/src/lib.rs:L1`
- **guard()** — `crates/mev-risk/src/lib.rs:L311`
- **.mark_sent()** — `crates/mev-risk/src/lib.rs:L237`
- **.record()** — `crates/mev-risk/src/lib.rs:L258`
- **.new()** — `crates/mev-risk/src/lib.rs:L86`
- **backruns_share_a_bounded_pending_allowance()** — `crates/mev-risk/src/lib.rs:L354`
- **PathBuf** — `graph`
- **.record_lost_race()** — `crates/mev-risk/src/lib.rs:L263`
- **.with_persistence()** — `crates/mev-risk/src/lib.rs:L100`
- **caps_and_failure_streak_survive_a_restart()** — `crates/mev-risk/src/lib.rs:L322`
- **concurrent_sends_up_to_the_limit()** — `crates/mev-risk/src/lib.rs:L436`
- **Self** — `graph`
- **.record_inner()** — `crates/mev-risk/src/lib.rs:L268`
- **.unresolved()** — `crates/mev-risk/src/lib.rs:L219`
- **.with_existing_persistence()** — `crates/mev-risk/src/lib.rs:L105`
- **.with_max_in_flight()** — `crates/mev-risk/src/lib.rs:L122`
- **arbitrage_has_its_own_slice_of_the_gas_budget()** — `crates/mev-risk/src/lib.rs:L388`
- **estimates_do_not_expand_approved_budgets()** — `crates/mev-risk/src/lib.rs:L468`
- **lost_races_cost_gas_but_never_pause()** — `crates/mev-risk/src/lib.rs:L373`
- **RiskLimits** — `crates/mev-risk/src/lib.rs:L16`
- **.nonce_window()** — `crates/mev-risk/src/lib.rs:L223`
- **.record_arb_gas()** — `crates/mev-risk/src/lib.rs:L252`
- **.with_tip_caps()** — `crates/mev-risk/src/lib.rs:L117`
- **corrupt_state_file_blocks_admission()** — `crates/mev-risk/src/lib.rs:L345`
- **daily_gas_cap_rolls_over()** — `crates/mev-risk/src/lib.rs:L418`
- **one_tx_in_flight()** — `crates/mev-risk/src/lib.rs:L429`
- **pauses_after_failure_streak_and_resets_on_success()** — `crates/mev-risk/src/lib.rs:L408`
- **.kill_file()** — `crates/mev-risk/src/lib.rs:L83`
- **.record_profit()** — `crates/mev-risk/src/lib.rs:L249`
- **loan_cap()** — `crates/mev-risk/src/lib.rs:L285`
- **.arb_spent_today_usd()** — `crates/mev-risk/src/lib.rs:L227`
- **.settled_spend()** — `crates/mev-risk/src/lib.rs:L231`
- **.spent_today_usd()** — `crates/mev-risk/src/lib.rs:L278`
- **Address** — `graph`
- **blocks_below_min_profit()** — `crates/mev-risk/src/lib.rs:L400`
- **loan_cap_takes_the_tighter_limit()** — `crates/mev-risk/src/lib.rs:L304`
- **Option** — `graph`
- **Pending** — `graph`
- **Vec** — `graph`
- **VecDeque** — `graph`

## Related communities

- [[docs/knowledge/communities/C204 - cost_microusd|cost_microusd]] — 14 cross-community edges
- [[docs/knowledge/communities/C035 - Block|Block]] — 8 cross-community edges
- [[docs/knowledge/communities/C288 - guard|guard]] — 2 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C169 - .new|.new]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
