---
graphify-generated: true
graphify-community: "13"
graphify-node-count: 58
graphify-internal-edges: 161
tags:
  - graphify/community
---

# Runner

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liquidate/src/lib.rs` — 42 nodes
- `graph` — 16 nodes

## Nodes (58)

- **Runner** — `crates/mev-liquidate/src/lib.rs:L124`
- **lib.rs** — `crates/mev-liquidate/src/lib.rs:L1`
- **.new()** — `crates/mev-liquidate/src/lib.rs:L154`
- **.poll()** — `crates/mev-liquidate/src/lib.rs:L382`
- **cfg()** — `crates/mev-liquidate/src/lib.rs:L742`
- **Slot** — `crates/mev-liquidate/src/lib.rs:L96`
- **shadow_for()** — `crates/mev-liquidate/src/lib.rs:L1150`
- **Option** — `graph`
- **RunnerEvent** — `crates/mev-liquidate/src/lib.rs:L62`
- **.sweep_all()** — `crates/mev-liquidate/src/lib.rs:L298`
- **OracleHint** — `crates/mev-liquidate/src/lib.rs:L89`
- **.on_feed_tx()** — `crates/mev-liquidate/src/lib.rs:L445`
- **.run()** — `crates/mev-liquidate/src/lib.rs:L533`
- **.apply_sweep()** — `crates/mev-liquidate/src/lib.rs:L332`
- **.on_hint()** — `crates/mev-liquidate/src/lib.rs:L499`
- **.sweep()** — `crates/mev-liquidate/src/lib.rs:L324`
- **one_successful_adapter_cannot_restore_another_failed_adapter()** — `crates/mev-liquidate/src/lib.rs:L919`
- **.invalidate()** — `crates/mev-liquidate/src/lib.rs:L206`
- **.stamp_plan()** — `crates/mev-liquidate/src/lib.rs:L197`
- **advisory_hook()** — `crates/mev-liquidate/src/lib.rs:L1228`
- **sequenced()** — `crates/mev-liquidate/src/lib.rs:L1125`
- **shadow_schedule_is_off_unless_enabled()** — `crates/mev-liquidate/src/lib.rs:L1215`
- **shadow_schedule_is_recorded_from_the_runner_loop_without_changing_dispatch()** — `crates/mev-liquidate/src/lib.rs:L1178`
- **.with_shadow_schedule()** — `crates/mev-liquidate/src/lib.rs:L231`
- **advisory_off_emits_no_advisory_event()** — `crates/mev-liquidate/src/lib.rs:L1273`
- **advisory_shadow_emits_its_event_and_leaves_the_schedule_event_unchanged()** — `crates/mev-liquidate/src/lib.rs:L1248`
- **feed_tx_to_watched_contract_starts_a_burst()** — `crates/mev-liquidate/src/lib.rs:L1024`
- **real_price_report_triggers_one_backrun_per_cooldown()** — `crates/mev-liquidate/src/lib.rs:L1057`
- **RunnerConfig** — `crates/mev-liquidate/src/lib.rs:L48`
- **.with_hints()** — `crates/mev-liquidate/src/lib.rs:L224`
- **.worth_watching()** — `crates/mev-liquidate/src/lib.rs:L262`
- **auction_hints_predict_once_and_fan_out_to_each_auction()** — `crates/mev-liquidate/src/lib.rs:L1088`
- **burst_polls_only_accounts_the_updated_feed_affects()** — `crates/mev-liquidate/src/lib.rs:L955`
- **healthy_accounts_are_not_at_risk_and_dust_is_ignored()** — `crates/mev-liquidate/src/lib.rs:L1009`
- **recv_feed()** — `crates/mev-liquidate/src/lib.rs:L625`
- **recv_opt()** — `crates/mev-liquidate/src/lib.rs:L632`
- **run_pass()** — `crates/mev-liquidate/src/lib.rs:L1242`
- **sweep_then_poll_plans_liquidatable_account_once()** — `crates/mev-liquidate/src/lib.rs:L970`
- **sweeps_skip_dormant_accounts_except_the_periodic_full_one()** — `crates/mev-liquidate/src/lib.rs:L856`
- **UnboundedSender** — `graph`
- **.index_all()** — `crates/mev-liquidate/src/lib.rs:L266`
- **.restore_confidence()** — `crates/mev-liquidate/src/lib.rs:L216`
- **Box** — `graph`
- **Duration** — `graph`
- **index_errors_back_off_instead_of_hammering()** — `crates/mev-liquidate/src/lib.rs:L896`
- **Instant** — `graph`
- **non_live_adapters_are_skipped()** — `crates/mev-liquidate/src/lib.rs:L911`
- **Self** — `graph`
- **String** — `graph`
- **UnboundedReceiver** — `graph`
- **HashMap** — `graph`
- **Receiver** — `graph`
- **AtomicBool** — `graph`
- **AtomicU64** — `graph`
- **HashSet** — `graph`
- **T** — `graph`
- **TempDir** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C028 - AccountRef|AccountRef]] — 53 cross-community edges
- [[docs/knowledge/communities/C137 - SequencedTx|SequencedTx]] — 5 cross-community edges
- [[docs/knowledge/communities/C029 - ShadowSchedule|ShadowSchedule]] — 5 cross-community edges
- [[docs/knowledge/communities/C109 - Hub|Hub]] — 3 cross-community edges
- [[docs/knowledge/communities/C130 - AdapterStatus|AdapterStatus]] — 3 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 2 cross-community edges
- [[docs/knowledge/communities/C081 - Address|Address]] — 2 cross-community edges
- [[docs/knowledge/communities/C034 - advisory.rs (C034)|advisory.rs (C034)]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
