---
graphify-generated: true
graphify-community: "26"
graphify-node-count: 82
graphify-internal-edges: 191
tags:
  - graphify/community
---

# lib.rs (C026)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liquidate/src/lib.rs` — 42 nodes
- `graph` — 25 nodes
- `crates/mev-lending/src/lib.rs` — 9 nodes
- `crates/mev-lending-aave-v3/src/lib.rs` — 1 node
- `crates/mev-lending-compound-v3/src/lib.rs` — 1 node
- `crates/mev-lending-dolomite/src/lib.rs` — 1 node
- `crates/mev-lending-gearbox/src/lib.rs` — 1 node
- `crates/mev-lending-radiant/src/lib.rs` — 1 node
- `crates/mev-lending-spark/src/lib.rs` — 1 node

## Nodes (82)

- **AccountRef** — `crates/mev-lending/src/lib.rs:L32`
- **Health** — `crates/mev-lending/src/lib.rs:L46`
- **Plan** — `crates/mev-lending/src/lib.rs:L73`
- **lib.rs** — `crates/mev-liquidate/src/lib.rs:L1`
- **Runner** — `crates/mev-liquidate/src/lib.rs:L69`
- **Fake** — `crates/mev-liquidate/src/lib.rs:L310`
- **.new()** — `crates/mev-liquidate/src/lib.rs:L78`
- **Address** — `graph`
- **Vec** — `graph`
- **Failing** — `crates/mev-liquidate/src/lib.rs:L401`
- **LiquidationAdapter** — `crates/mev-lending/src/lib.rs:L97`
- **AdapterStatus** — `crates/mev-lending/src/lib.rs:L20`
- **Slot** — `crates/mev-liquidate/src/lib.rs:L56`
- **RunnerEvent** — `crates/mev-liquidate/src/lib.rs:L45`
- **.health()** — `crates/mev-liquidate/src/lib.rs:L418`
- **.plan()** — `crates/mev-liquidate/src/lib.rs:L421`
- **.poll()** — `crates/mev-liquidate/src/lib.rs:L191`
- **cfg()** — `crates/mev-liquidate/src/lib.rs:L378`
- **lib.rs** — `crates/mev-lending/src/lib.rs:L1`
- **.plan()** — `crates/mev-liquidate/src/lib.rs:L350`
- **.run()** — `crates/mev-liquidate/src/lib.rs:L245`
- **.sweep()** — `crates/mev-liquidate/src/lib.rs:L165`
- **Option** — `graph`
- **ProtocolCall** — `crates/mev-lending/src/lib.rs:L63`
- **Result** — `graph`
- **.health()** — `crates/mev-liquidate/src/lib.rs:L334`
- **.on_feed_tx()** — `crates/mev-liquidate/src/lib.rs:L232`
- **feed_tx_to_watched_contract_starts_a_burst()** — `crates/mev-liquidate/src/lib.rs:L496`
- **RunnerConfig** — `crates/mev-liquidate/src/lib.rs:L31`
- **sequenced()** — `crates/mev-liquidate/src/lib.rs:L533`
- **.accounts()** — `crates/mev-liquidate/src/lib.rs:L415`
- **.assets()** — `crates/mev-liquidate/src/lib.rs:L112`
- **.assets()** — `crates/mev-liquidate/src/lib.rs:L430`
- **.price_feeds()** — `crates/mev-liquidate/src/lib.rs:L424`
- **.targets()** — `crates/mev-liquidate/src/lib.rs:L120`
- **.targets()** — `crates/mev-liquidate/src/lib.rs:L427`
- **.worth_watching()** — `crates/mev-liquidate/src/lib.rs:L132`
- **healthy_accounts_are_not_at_risk_and_dust_is_ignored()** — `crates/mev-liquidate/src/lib.rs:L481`
- **recv_opt()** — `crates/mev-liquidate/src/lib.rs:L295`
- **sweep_then_poll_plans_liquidatable_account_once()** — `crates/mev-liquidate/src/lib.rs:L459`
- **.accounts()** — `crates/mev-liquidate/src/lib.rs:L327`
- **.assets()** — `crates/mev-liquidate/src/lib.rs:L373`
- **.fmt()** — `crates/mev-lending/src/lib.rs:L40`
- **.index()** — `crates/mev-liquidate/src/lib.rs:L411`
- **.price_feeds()** — `crates/mev-liquidate/src/lib.rs:L367`
- **.status()** — `crates/mev-liquidate/src/lib.rs:L408`
- **.targets()** — `crates/mev-liquidate/src/lib.rs:L370`
- **Address** — `graph`
- **Box** — `graph`
- **index_errors_back_off_instead_of_hammering()** — `crates/mev-liquidate/src/lib.rs:L436`
- **non_live_adapters_are_skipped()** — `crates/mev-liquidate/src/lib.rs:L451`
- **UnboundedSender** — `graph`
- **.id()** — `crates/mev-liquidate/src/lib.rs:L318`
- **.id()** — `crates/mev-liquidate/src/lib.rs:L405`
- **.index_all()** — `crates/mev-liquidate/src/lib.rs:L136`
- **.index()** — `crates/mev-liquidate/src/lib.rs:L324`
- **.status()** — `crates/mev-lending-aave-v3/src/lib.rs:L198`
- **.status()** — `crates/mev-lending-compound-v3/src/lib.rs:L325`
- **.status()** — `crates/mev-lending-dolomite/src/lib.rs:L40`
- **.status()** — `crates/mev-lending-gearbox/src/lib.rs:L40`
- **.status()** — `crates/mev-lending-radiant/src/lib.rs:L32`
- **.status()** — `crates/mev-lending-spark/src/lib.rs:L31`
- **.status()** — `crates/mev-liquidate/src/lib.rs:L321`
- **Arc** — `graph`
- **Duration** — `graph`
- **HashMap** — `graph`
- **Instant** — `graph`
- **Mutex** — `graph`
- **ProtocolId** — `graph`
- **Sync** — `graph`
- **U256** — `graph`
- **UnboundedReceiver** — `graph`
- **.liquidatable()** — `crates/mev-lending/src/lib.rs:L55`
- **Bytes** — `graph`
- **Display** — `graph`
- **Formatter** — `graph`
- **ProtocolId** — `graph`
- **Result** — `graph`
- **Self** — `graph`
- **Send** — `graph`
- **T** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C025 - lib.rs (C025)|lib.rs (C025)]] — 8 cross-community edges
- [[docs/knowledge/communities/C039 - lib.rs (C039)|lib.rs (C039)]] — 8 cross-community edges
- [[docs/knowledge/communities/C040 - lib.rs (C040)|lib.rs (C040)]] — 7 cross-community edges
- [[docs/knowledge/communities/C042 - lib.rs (C042)|lib.rs (C042)]] — 7 cross-community edges
- [[docs/knowledge/communities/C044 - lib.rs (C044)|lib.rs (C044)]] — 7 cross-community edges
- [[docs/knowledge/communities/C046 - lib.rs (C046)|lib.rs (C046)]] — 7 cross-community edges
- [[docs/knowledge/communities/C034 - Engine|Engine]] — 6 cross-community edges
- [[docs/knowledge/communities/C003 - Infrastructure & Cost Plan|Infrastructure & Cost Plan]] — 4 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 2 cross-community edges
- [[docs/knowledge/communities/C028 - lib.rs (C028)|lib.rs (C028)]] — 2 cross-community edges
- [[docs/knowledge/communities/C033 - lib.rs (C033)|lib.rs (C033)]] — 1 cross-community edges
