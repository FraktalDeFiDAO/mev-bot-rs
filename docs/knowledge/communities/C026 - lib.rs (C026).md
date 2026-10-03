---
graphify-generated: true
graphify-community: "26"
graphify-node-count: 53
graphify-internal-edges: 121
tags:
  - graphify/community
---

# lib.rs (C026)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liquidate/src/lib.rs` — 31 nodes
- `graph` — 17 nodes
- `crates/mev-lending/src/lib.rs` — 4 nodes
- `crates/mev-lending-aave-v3/src/lib.rs` — 1 node

## Nodes (53)

- **Runner** — `crates/mev-liquidate/src/lib.rs:L63`
- **Fake** — `crates/mev-liquidate/src/lib.rs:L287`
- **lib.rs** — `crates/mev-liquidate/src/lib.rs:L1`
- **.new()** — `crates/mev-liquidate/src/lib.rs:L72`
- **Address** — `graph`
- **Health** — `crates/mev-lending/src/lib.rs:L46`
- **RunnerEvent** — `crates/mev-liquidate/src/lib.rs:L45`
- **Slot** — `crates/mev-liquidate/src/lib.rs:L56`
- **Vec** — `graph`
- **.health()** — `crates/mev-liquidate/src/lib.rs:L311`
- **.plan()** — `crates/mev-liquidate/src/lib.rs:L327`
- **.poll()** — `crates/mev-liquidate/src/lib.rs:L168`
- **LiquidationAdapter** — `crates/mev-lending/src/lib.rs:L97`
- **.run()** — `crates/mev-liquidate/src/lib.rs:L222`
- **.sweep()** — `crates/mev-liquidate/src/lib.rs:L142`
- **cfg()** — `crates/mev-liquidate/src/lib.rs:L355`
- **.on_feed_tx()** — `crates/mev-liquidate/src/lib.rs:L209`
- **feed_tx_to_watched_contract_starts_a_burst()** — `crates/mev-liquidate/src/lib.rs:L423`
- **Option** — `graph`
- **RunnerConfig** — `crates/mev-liquidate/src/lib.rs:L31`
- **sequenced()** — `crates/mev-liquidate/src/lib.rs:L447`
- **.accounts()** — `crates/mev-liquidate/src/lib.rs:L304`
- **.assets()** — `crates/mev-liquidate/src/lib.rs:L104`
- **.assets()** — `crates/mev-liquidate/src/lib.rs:L350`
- **.price_feeds()** — `crates/mev-liquidate/src/lib.rs:L344`
- **.targets()** — `crates/mev-liquidate/src/lib.rs:L112`
- **.targets()** — `crates/mev-liquidate/src/lib.rs:L347`
- **.worth_watching()** — `crates/mev-liquidate/src/lib.rs:L124`
- **AdapterStatus** — `crates/mev-lending/src/lib.rs:L20`
- **healthy_accounts_are_not_at_risk_and_dust_is_ignored()** — `crates/mev-liquidate/src/lib.rs:L408`
- **recv_opt()** — `crates/mev-liquidate/src/lib.rs:L272`
- **sweep_then_poll_plans_liquidatable_account_once()** — `crates/mev-liquidate/src/lib.rs:L386`
- **.index()** — `crates/mev-liquidate/src/lib.rs:L301`
- **.status()** — `crates/mev-liquidate/src/lib.rs:L298`
- **Box** — `graph`
- **non_live_adapters_are_skipped()** — `crates/mev-liquidate/src/lib.rs:L378`
- **Result** — `graph`
- **UnboundedSender** — `graph`
- **.id()** — `crates/mev-liquidate/src/lib.rs:L295`
- **.index_all()** — `crates/mev-liquidate/src/lib.rs:L128`
- **.status()** — `crates/mev-lending-aave-v3/src/lib.rs:L246`
- **HashMap** — `graph`
- **Instant** — `graph`
- **Sync** — `graph`
- **UnboundedReceiver** — `graph`
- **.liquidatable()** — `crates/mev-lending/src/lib.rs:L55`
- **Arc** — `graph`
- **Duration** — `graph`
- **Mutex** — `graph`
- **ProtocolId** — `graph`
- **Self** — `graph`
- **Send** — `graph`
- **T** — `graph`

## Related communities

- [[docs/knowledge/communities/C025 - lib.rs (C025)|lib.rs (C025)]] — 15 cross-community edges
- [[docs/knowledge/communities/C003 - Infrastructure & Cost Plan|Infrastructure & Cost Plan]] — 4 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 3 cross-community edges
- [[docs/knowledge/communities/C028 - lib.rs (C028)|lib.rs (C028)]] — 2 cross-community edges
