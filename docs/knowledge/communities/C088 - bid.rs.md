---
graphify-generated: true
graphify-community: "88"
graphify-node-count: 25
graphify-internal-edges: 52
tags:
  - graphify/community
---

# bid.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-risk/src/bid.rs` — 22 nodes
- `graph` — 2 nodes
- `crates/mev-exec/src/lib.rs` — 1 node

## Nodes (25)

- **bid.rs** — `crates/mev-risk/src/bid.rs:L1`
- **.decide()** — `crates/mev-risk/src/bid.rs:L137`
- **input()** — `crates/mev-risk/src/bid.rs:L330`
- **.decide()** — `crates/mev-risk/src/bid.rs:L118`
- **BidPolicy** — `crates/mev-risk/src/bid.rs:L19`
- **Option** — `graph`
- **.integer()** — `crates/mev-risk/src/bid.rs:L99`
- **Decision** — `crates/mev-risk/src/bid.rs:L72`
- **policy()** — `crates/mev-risk/src/bid.rs:L319`
- **Bid** — `crates/mev-risk/src/bid.rs:L50`
- **bid_never_makes_expected_profit_negative_or_below_floor()** — `crates/mev-risk/src/bid.rs:L354`
- **BidInput** — `crates/mev-risk/src/bid.rs:L35`
- **hard_per_tx_cap_binds_on_large_profits()** — `crates/mev-risk/src/bid.rs:L371`
- **IntegerBidPolicy** — `crates/mev-risk/src/bid.rs:L88`
- **outbids_competition_by_the_margin_and_sets_consistent_fee_fields()** — `crates/mev-risk/src/bid.rs:L386`
- **Skip** — `crates/mev-risk/src/bid.rs:L62`
- **.from_bid()** — `crates/mev-exec/src/lib.rs:L460`
- **IntegerBidInput** — `crates/mev-risk/src/bid.rs:L79`
- **invalid_inputs_skip()** — `crates/mev-risk/src/bid.rs:L402`
- **route_competition()** — `crates/mev-risk/src/bid.rs:L220`
- **strongest()** — `crates/mev-risk/src/bid.rs:L206`
- **a_route_with_history_is_priced_by_its_own_winners()** — `crates/mev-risk/src/bid.rs:L293`
- **Result** — `graph`
- **strongest_takes_the_higher_estimate()** — `crates/mev-risk/src/bid.rs:L425`
- **zero_profit_never_bids()** — `crates/mev-risk/src/bid.rs:L341`

## Related communities

- [[docs/knowledge/communities/C198 - TipBook|TipBook]] — 4 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C169 - .new|.new]] — 1 cross-community edges
- [[docs/knowledge/communities/C171 - Fees|Fees]] — 1 cross-community edges
- [[docs/knowledge/communities/C035 - Block|Block]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
- [[docs/knowledge/communities/C093 - Strategy|Strategy]] — 1 cross-community edges
