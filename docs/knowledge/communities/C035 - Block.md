---
graphify-generated: true
graphify-community: "35"
graphify-node-count: 47
graphify-internal-edges: 130
tags:
  - graphify/community
---

# Block

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-risk/src/ledger.rs` — 38 nodes
- `graph` — 8 nodes
- `crates/mev-risk/src/lib.rs` — 1 node

## Nodes (47)

- **Block** — `crates/mev-risk/src/lib.rs:L26`
- **Ledger** — `crates/mev-risk/src/ledger.rs:L96`
- **Reservation** — `crates/mev-risk/src/ledger.rs:L99`
- **ledger.rs** — `crates/mev-risk/src/ledger.rs:L1`
- **Result** — `graph`
- **.reserve_with_tip()** — `crates/mev-risk/src/ledger.rs:L408`
- **.save()** — `crates/mev-risk/src/ledger.rs:L105`
- **.attach()** — `crates/mev-risk/src/ledger.rs:L548`
- **.roll()** — `crates/mev-risk/src/ledger.rs:L118`
- **.settle()** — `crates/mev-risk/src/ledger.rs:L597`
- **signed_tip_liabilities_and_charges_survive_restart()** — `crates/mev-risk/src/ledger.rs:L770`
- **.persistent_existing()** — `crates/mev-risk/src/ledger.rs:L268`
- **.set_native_price()** — `crates/mev-risk/src/ledger.rs:L528`
- **native_cost()** — `crates/mev-risk/src/ledger.rs:L626`
- **pending_liability_survives_restart_and_corruption_blocks_sends()** — `crates/mev-risk/src/ledger.rs:L838`
- **.default()** — `crates/mev-risk/src/ledger.rs:L35`
- **.initialize()** — `crates/mev-risk/src/ledger.rs:L136`
- **.open()** — `crates/mev-risk/src/ledger.rs:L272`
- **.persistent()** — `crates/mev-risk/src/ledger.rs:L264`
- **.reserve()** — `crates/mev-risk/src/ledger.rs:L362`
- **.unresolved()** — `crates/mev-risk/src/ledger.rs:L449`
- **admission_touches_no_disk_and_the_single_durable_write_is_attach()** — `crates/mev-risk/src/ledger.rs:L721`
- **TipCaps** — `crates/mev-risk/src/ledger.rs:L29`
- **.check_tip()** — `crates/mev-risk/src/ledger.rs:L366`
- **.require_identity()** — `crates/mev-risk/src/ledger.rs:L237`
- **.cancel_before_broadcast()** — `crates/mev-risk/src/ledger.rs:L587`
- **.cancel_unsigned()** — `crates/mev-risk/src/ledger.rs:L573`
- **.check()** — `crates/mev-risk/src/ledger.rs:L310`
- **.receipt_cost()** — `crates/mev-risk/src/ledger.rs:L541`
- **.verify_terms()** — `crates/mev-risk/src/ledger.rs:L510`
- **migration_requires_complete_receipts_and_refuses_overwrite()** — `crates/mev-risk/src/ledger.rs:L647`
- **PathBuf** — `graph`
- **pending_tips_cross_midnight_and_settle_to_a_conservative_charge()** — `crates/mev-risk/src/ledger.rs:L807`
- **Self** — `graph`
- **signed_priority_fee_cannot_exceed_the_reserved_tip_bound()** — `crates/mev-risk/src/ledger.rs:L753`
- **.add_arb_cost()** — `crates/mev-risk/src/ledger.rs:L494`
- **.nonce_window()** — `crates/mev-risk/src/ledger.rs:L249`
- **.record_legacy()** — `crates/mev-risk/src/ledger.rs:L478`
- **live_open_cannot_create_an_empty_budget()** — `crates/mev-risk/src/ledger.rs:L708`
- **.active()** — `crates/mev-risk/src/ledger.rs:L503`
- **.arb_spent()** — `crates/mev-risk/src/ledger.rs:L473`
- **.spent()** — `crates/mev-risk/src/ledger.rs:L469`
- **Arc** — `graph`
- **Default** — `graph`
- **Display** — `graph`
- **Error** — `graph`
- **Mutex** — `graph`

## Related communities

- [[docs/knowledge/communities/C193 - Seed|Seed]] — 14 cross-community edges
- [[docs/knowledge/communities/C204 - cost_microusd|cost_microusd]] — 11 cross-community edges
- [[docs/knowledge/communities/C043 - RiskGuard|RiskGuard]] — 8 cross-community edges
- [[docs/knowledge/communities/C147 - prepare|prepare]] — 3 cross-community edges
- [[docs/knowledge/communities/C190 - now_secs|now_secs]] — 3 cross-community edges
- [[docs/knowledge/communities/C235 - money.rs|money.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C162 - assemble|assemble]] — 2 cross-community edges
- [[docs/knowledge/communities/C069 - Item|Item]] — 2 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C149 - Sender|Sender]] — 1 cross-community edges
- [[docs/knowledge/communities/C169 - .new|.new]] — 1 cross-community edges
- [[docs/knowledge/communities/C088 - bid.rs|bid.rs]] — 1 cross-community edges
