---
graphify-generated: true
graphify-community: "58"
graphify-node-count: 36
graphify-internal-edges: 112
tags:
  - graphify/community
---

# exact.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-dex/src/exact.rs` — 32 nodes
- `graph` — 4 nodes

## Nodes (36)

- **exact.rs** — `crates/mev-dex/src/exact.rs:L1`
- **U256** — `graph`
- **Option** — `graph`
- **.swap_exact_in()** — `crates/mev-dex/src/exact.rs:L323`
- **swap_step_exact_in()** — `crates/mev-dex/src/exact.rs:L209`
- **sqrt_ratio_at_tick()** — `crates/mev-dex/src/exact.rs:L107`
- **mul_div_up()** — `crates/mev-dex/src/exact.rs:L30`
- **mul_div()** — `crates/mev-dex/src/exact.rs:L22`
- **pool()** — `crates/mev-dex/src/exact.rs:L569`
- **amount0_delta()** — `crates/mev-dex/src/exact.rs:L135`
- **amount1_delta()** — `crates/mev-dex/src/exact.rs:L149`
- **narrow()** — `crates/mev-dex/src/exact.rs:L14`
- **next_price_from_amount0_up_add()** — `crates/mev-dex/src/exact.rs:L159`
- **next_sqrt_price_from_input()** — `crates/mev-dex/src/exact.rs:L184`
- **tick_at_sqrt_ratio()** — `crates/mev-dex/src/exact.rs:L267`
- **div_up()** — `crates/mev-dex/src/exact.rs:L43`
- **max_sqrt_ratio()** — `crates/mev-dex/src/exact.rs:L55`
- **next_price_from_amount1_down_add()** — `crates/mev-dex/src/exact.rs:L172`
- **steps_stop_at_bitmap_word_boundaries_like_the_contract()** — `crates/mev-dex/src/exact.rs:L641`
- **V3Exact** — `crates/mev-dex/src/exact.rs:L286`
- **v2_amount_out()** — `crates/mev-dex/src/exact.rs:L61`
- **a_price_outside_the_known_tick_window_is_not_quoted()** — `crates/mev-dex/src/exact.rs:L532`
- **a_swap_inside_one_band_matches_the_closed_form()** — `crates/mev-dex/src/exact.rs:L593`
- **crossing_a_tick_changes_liquidity_and_spends_exactly_the_input()** — `crates/mev-dex/src/exact.rs:L607`
- **max_sqrt()** — `crates/mev-dex/src/exact.rs:L474`
- **running_past_the_loaded_window_is_reported_not_guessed()** — `crates/mev-dex/src/exact.rs:L631`
- **Step** — `crates/mev-dex/src/exact.rs:L201`
- **swap_step_conserves_the_input_and_never_overpays()** — `crates/mev-dex/src/exact.rs:L552`
- **Swapped** — `crates/mev-dex/src/exact.rs:L310`
- **u()** — `crates/mev-dex/src/exact.rs:L482`
- **v2_matches_the_library_and_reverts_on_empty_inputs()** — `crates/mev-dex/src/exact.rs:L510`
- **Next** — `crates/mev-dex/src/exact.rs:L300`
- **tick_lookup_inverts_the_forward_function()** — `crates/mev-dex/src/exact.rs:L583`
- **Vec** — `graph`
- **tick_math_matches_the_contract_at_known_points()** — `crates/mev-dex/src/exact.rs:L487`
- **U512** — `graph`

## Related communities

- [[docs/knowledge/communities/C001 - lib.rs (C001)|lib.rs (C001)]] — 1 cross-community edges
