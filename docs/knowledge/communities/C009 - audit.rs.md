---
graphify-generated: true
graphify-community: "9"
graphify-node-count: 69
graphify-internal-edges: 151
tags:
  - graphify/community
---

# audit.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 25 nodes
- `crates/mev-analytics/src/audit.rs` — 20 nodes
- `crates/mev-lending-aave-v3/src/exact.rs` — 12 nodes
- `crates/mev-analytics/src/bin/mev-liq-audit.rs` — 11 nodes
- `crates/mev-lending/src/lib.rs` — 1 node

## Nodes (69)

- **audit.rs** — `crates/mev-analytics/src/audit.rs:L1`
- **BotLog** — `crates/mev-analytics/src/audit.rs:L32`
- **exact.rs** — `crates/mev-lending-aave-v3/src/exact.rs:L1`
- **main()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L47`
- **mev-liq-audit.rs** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L1`
- **decode()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L120`
- **Liquidation** — `crates/mev-analytics/src/audit.rs:L14`
- **.parse()** — `crates/mev-analytics/src/audit.rs:L41`
- **classify()** — `crates/mev-analytics/src/audit.rs:L145`
- **available_collateral()** — `crates/mev-lending-aave-v3/src/exact.rs:L54`
- **get_logs()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L256`
- **open_secs()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L187`
- **Address** — `graph`
- **base()** — `crates/mev-lending-aave-v3/src/exact.rs:L109`
- **load_indexed()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L309`
- **Result** — `graph`
- **addr()** — `crates/mev-analytics/src/audit.rs:L206`
- **block_ts()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L233`
- **DynProvider** — `graph`
- **U256** — `graph`
- **usd()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L174`
- **Address** — `graph`
- **atlas_bundles_are_auctions_not_races()** — `crates/mev-analytics/src/audit.rs:L252`
- **block_at_ts()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L242`
- **first_liquidatable()** — `crates/mev-analytics/src/audit.rs:L170`
- **liq()** — `crates/mev-analytics/src/audit.rs:L210`
- **load_bot_log()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L293`
- **report()** — `crates/mev-analytics/src/bin/mev-liq-audit.rs:L332`
- **t()** — `crates/mev-analytics/src/audit.rs:L200`
- **.touches_near()** — `crates/mev-analytics/src/audit.rs:L75`
- **addresses_in()** — `crates/mev-analytics/src/audit.rs:L102`
- **Inputs** — `crates/mev-lending-aave-v3/src/exact.rs:L41`
- **Payout** — `crates/mev-lending-aave-v3/src/exact.rs:L33`
- **percent_div()** — `crates/mev-lending-aave-v3/src/exact.rs:L22`
- **percent_mul()** — `crates/mev-lending-aave-v3/src/exact.rs:L11`
- **u()** — `crates/mev-lending-aave-v3/src/exact.rs:L105`
- **when_collateral_runs_out_the_repayment_shrinks_to_match()** — `crates/mev-lending-aave-v3/src/exact.rs:L140`
- **a_thousand_dollars_of_debt_pays_out_the_bonus_less_the_protocol_cut()** — `crates/mev-lending-aave-v3/src/exact.rs:L131`
- **classify_orders_reasons()** — `crates/mev-analytics/src/audit.rs:L272`
- **Option** — `graph`
- **Option** — `graph`
- **Option** — `graph`
- **parses_uptime_and_touched_accounts()** — `crates/mev-analytics/src/audit.rs:L226`
- **Verdict** — `crates/mev-analytics/src/audit.rs:L124`
- **.liquidatable()** — `crates/mev-lending/src/lib.rs:L38`
- **.span()** — `crates/mev-analytics/src/audit.rs:L88`
- **.up_at()** — `crates/mev-analytics/src/audit.rs:L69`
- **HashSet** — `graph`
- **Log** — `graph`
- **PathBuf** — `graph`
- **String** — `graph`
- **Vec** — `graph`
- **zero_prices_revert()** — `crates/mev-lending-aave-v3/src/exact.rs:L150`
- **.up_minutes_between()** — `crates/mev-analytics/src/audit.rs:L96`
- **address_scan_ignores_longer_hex()** — `crates/mev-analytics/src/audit.rs:L245`
- **B256** — `graph`
- **BTreeSet** — `graph`
- **F** — `graph`
- **first_liquidatable_finds_the_crossing_block()** — `crates/mev-analytics/src/audit.rs:L301`
- **HashMap** — `graph`
- **HashMap** — `graph`
- **HashSet** — `graph`
- **IntoIterator** — `graph`
- **percent_math_rounds_half_up_like_the_library()** — `crates/mev-lending-aave-v3/src/exact.rs:L123`
- **Self** — `graph`
- **TxHash** — `graph`
- **U256** — `graph`
- **Vec** — `graph`
- **Verdict** — `graph`

## Related communities

- [[docs/knowledge/communities/C000 - liquidation.rs|liquidation.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C100 - build_features|build_features]] — 1 cross-community edges
- [[docs/knowledge/communities/C155 - LocalStats|LocalStats]] — 1 cross-community edges
- [[docs/knowledge/communities/C234 - PresimStats|PresimStats]] — 1 cross-community edges
- [[docs/knowledge/communities/C028 - AccountRef|AccountRef]] — 1 cross-community edges
- [[docs/knowledge/communities/C047 - lib.rs (C047)|lib.rs (C047)]] — 1 cross-community edges
- [[docs/knowledge/communities/C069 - Item|Item]] — 1 cross-community edges
- [[docs/knowledge/communities/C070 - triggers.rs|triggers.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C079 - math.rs (C079)|math.rs (C079)]] — 1 cross-community edges
