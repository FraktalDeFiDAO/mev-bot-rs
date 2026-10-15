---
graphify-generated: true
graphify-community: "20"
graphify-node-count: 52
graphify-internal-edges: 99
tags:
  - graphify/community
---

# Auction

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 20 nodes
- `crates/mev-atlas/src/sign.rs` — 14 nodes
- `crates/mev-atlas/src/lib.rs` — 12 nodes
- `crates/mev-atlas/src/client.rs` — 6 nodes

## Nodes (52)

- **Auction** — `crates/mev-atlas/src/lib.rs:L37`
- **sign.rs** — `crates/mev-atlas/src/sign.rs:L1`
- **lib.rs** — `crates/mev-atlas/src/lib.rs:L1`
- **sign()** — `crates/mev-atlas/src/sign.rs:L72`
- **handle()** — `crates/mev-atlas/src/client.rs:L92`
- **RawOp** — `crates/mev-atlas/src/lib.rs:L65`
- **run()** — `crates/mev-atlas/src/client.rs:L34`
- **digest()** — `crates/mev-atlas/src/sign.rs:L46`
- **op()** — `crates/mev-atlas/src/sign.rs:L126`
- **OpParams** — `crates/mev-atlas/src/sign.rs:L92`
- **parse_notification()** — `crates/mev-atlas/src/lib.rs:L93`
- **SignedSolverOp** — `crates/mev-atlas/src/sign.rs:L51`
- **client.rs** — `crates/mev-atlas/src/client.rs:L1`
- **Event** — `crates/mev-atlas/src/client.rs:L22`
- **Hints** — `crates/mev-atlas/src/lib.rs:L27`
- **RawHints** — `crates/mev-atlas/src/lib.rs:L56`
- **SolverOperation** — `graph`
- **Address** — `graph`
- **build()** — `crates/mev-atlas/src/sign.rs:L101`
- **Bytes** — `graph`
- **json_uses_hex_quantities()** — `crates/mev-atlas/src/sign.rs:L193`
- **signature_recovers_to_the_bonded_signer()** — `crates/mev-atlas/src/sign.rs:L151`
- **String** — `graph`
- **subscribe_frame()** — `crates/mev-atlas/src/lib.rs:L86`
- **U256** — `graph`
- **.to_json()** — `crates/mev-atlas/src/sign.rs:L58`
- **domain()** — `crates/mev-atlas/src/sign.rs:L35`
- **notification()** — `crates/mev-atlas/src/lib.rs:L139`
- **Option** — `graph`
- **parses_an_auction_notification()** — `crates/mev-atlas/src/lib.rs:L162`
- **submit_frame()** — `crates/mev-atlas/src/lib.rs:L127`
- **Address** — `graph`
- **auctions_are_forwarded()** — `crates/mev-atlas/src/client.rs:L148`
- **B256** — `graph`
- **build_follows_the_auction_rules()** — `crates/mev-atlas/src/sign.rs:L170`
- **Bytes** — `graph`
- **replies_to_submissions_become_events()** — `crates/mev-atlas/src/client.rs:L123`
- **String** — `graph`
- **subscribe_frame_matches_the_spec()** — `crates/mev-atlas/src/lib.rs:L189`
- **UnboundedSender** — `graph`
- **B256** — `graph`
- **eip712_type_matches_atlas_solver_typehash()** — `crates/mev-atlas/src/sign.rs:L144`
- **Eip712Domain** — `graph`
- **ignores_non_notifications_and_rejects_malformed_ones()** — `crates/mev-atlas/src/lib.rs:L178`
- **Option** — `graph`
- **PrivateKeySigner** — `graph`
- **refuses_to_sign_for_a_different_from_address()** — `crates/mev-atlas/src/sign.rs:L164`
- **Result** — `graph`
- **Result** — `graph`
- **U256** — `graph`
- **UnboundedReceiver** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C109 - Hub|Hub]] — 2 cross-community edges
- [[docs/knowledge/communities/C047 - lib.rs (C047)|lib.rs (C047)]] — 1 cross-community edges
