---
graphify-generated: true
graphify-community: "11"
graphify-node-count: 62
graphify-internal-edges: 139
tags:
  - graphify/community
---

# gate.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-risk/src/gate.rs` — 40 nodes
- `graph` — 14 nodes
- `crates/mev-risk/src/persist.rs` — 8 nodes

## Nodes (62)

- **gate.rs** — `crates/mev-risk/src/gate.rs:L1`
- **.new()** — `crates/mev-risk/src/gate.rs:L314`
- **Store** — `crates/mev-risk/src/persist.rs:L15`
- **.record()** — `crates/mev-risk/src/gate.rs:L259`
- **cfg()** — `crates/mev-risk/src/gate.rs:L351`
- **.check()** — `crates/mev-risk/src/gate.rs:L171`
- **RouteGate<K>** — `crates/mev-risk/src/gate.rs:L119`
- **GateConfig** — `crates/mev-risk/src/gate.rs:L20`
- **RouteGate** — `crates/mev-risk/src/gate.rs:L100`
- **.with_persistence()** — `crates/mev-risk/src/gate.rs:L132`
- **Cooldowns<K>** — `crates/mev-risk/src/gate.rs:L313`
- **K** — `graph`
- **.load_checked()** — `crates/mev-risk/src/persist.rs:L60`
- **.load()** — `crates/mev-risk/src/persist.rs:L49`
- **.save_checked()** — `crates/mev-risk/src/persist.rs:L82`
- **a_route_that_keeps_losing_is_backed_off_then_released()** — `crates/mev-risk/src/gate.rs:L464`
- **a_win_resets_the_losing_streak()** — `crates/mev-risk/src/gate.rs:L564`
- **negative_ev_routes_are_skipped_and_big_wins_still_go()** — `crates/mev-risk/src/gate.rs:L482`
- **the_global_record_survives_a_restart()** — `crates/mev-risk/src/gate.rs:L427`
- **.forget()** — `crates/mev-risk/src/gate.rs:L286`
- **.new()** — `crates/mev-risk/src/persist.rs:L22`
- **.save()** — `crates/mev-risk/src/gate.rs:L151`
- **a_new_route_gets_its_free_tries()** — `crates/mev-risk/src/gate.rs:L453`
- **a_new_route_inherits_the_global_record_after_its_tries()** — `crates/mev-risk/src/gate.rs:L515`
- **Cooldowns** — `crates/mev-risk/src/gate.rs:L309`
- **cooldowns_expire_and_prune()** — `crates/mev-risk/src/gate.rs:L356`
- **disabled_gate_never_blocks()** — `crates/mev-risk/src/gate.rs:L551`
- **no_wins_anywhere_stops_all_sending_but_a_probe_each_period()** — `crates/mev-risk/src/gate.rs:L370`
- **old_results_age_out_so_a_dead_route_is_retried_from_scratch()** — `crates/mev-risk/src/gate.rs:L538`
- **Self** — `graph`
- **T** — `graph`
- **the_global_stop_is_off_at_zero_and_ages_out()** — `crates/mev-risk/src/gate.rs:L401`
- **winning_routes_are_trusted()** — `crates/mev-risk/src/gate.rs:L499`
- **.default()** — `crates/mev-risk/src/gate.rs:L44`
- **.mark()** — `crates/mev-risk/src/gate.rs:L320`
- **.new()** — `crates/mev-risk/src/gate.rs:L120`
- **.prior_ppm()** — `crates/mev-risk/src/gate.rs:L163`
- **.record_global()** — `crates/mev-risk/src/gate.rs:L274`
- **.save()** — `crates/mev-risk/src/persist.rs:L76`
- **HashMap** — `graph`
- **Option** — `graph`
- **Verdict** — `crates/mev-risk/src/gate.rs:L61`
- **.active()** — `crates/mev-risk/src/gate.rs:L325`
- **.healthy()** — `crates/mev-risk/src/persist.rs:L56`
- **.is_empty()** — `crates/mev-risk/src/gate.rs:L301`
- **.is_empty()** — `crates/mev-risk/src/gate.rs:L340`
- **.prune()** — `crates/mev-risk/src/gate.rs:L332`
- **PathBuf** — `graph`
- **PathBuf** — `graph`
- **Result** — `graph`
- **Stats** — `crates/mev-risk/src/gate.rs:L90`
- **.allows()** — `crates/mev-risk/src/gate.rs:L84`
- **.len()** — `crates/mev-risk/src/gate.rs:L297`
- **.len()** — `crates/mev-risk/src/gate.rs:L336`
- **Arc** — `graph`
- **AtomicBool** — `graph`
- **Default** — `graph`
- **File** — `graph`
- **Option** — `graph`
- **persist.rs** — `crates/mev-risk/src/persist.rs:L1`
- **SavedGate** — `crates/mev-risk/src/gate.rs:L112`
- **Self** — `graph`

## Related communities

- [[docs/knowledge/communities/C014 - Engine|Engine]] — 2 cross-community edges
- [[docs/knowledge/communities/C193 - Seed|Seed]] — 1 cross-community edges
