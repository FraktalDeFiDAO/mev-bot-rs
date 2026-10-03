---
graphify-generated: true
graphify-community: "3"
graphify-node-count: 22
graphify-internal-edges: 52
tags:
  - graphify/community
---

# Infrastructure & Cost Plan

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-sequencer/src/lib.rs` — 17 nodes
- `graph` — 5 nodes

## Nodes (22)

- **lib.rs** — `crates/mev-sequencer/src/lib.rs:L1`
- **decode_l2_message()** — `crates/mev-sequencer/src/lib.rs:L108`
- **parse_broadcast()** — `crates/mev-sequencer/src/lib.rs:L87`
- **signed_msg()** — `crates/mev-sequencer/src/lib.rs:L220`
- **signed_tx()** — `crates/mev-sequencer/src/lib.rs:L204`
- **stream_once()** — `crates/mev-sequencer/src/lib.rs:L155`
- **decodes_nested_batches()** — `crates/mev-sequencer/src/lib.rs:L245`
- **run()** — `crates/mev-sequencer/src/lib.rs:L136`
- **truncated_batch_is_skipped_not_panicking()** — `crates/mev-sequencer/src/lib.rs:L256`
- **TxEnvelope** — `graph`
- **Vec** — `graph`
- **batch()** — `crates/mev-sequencer/src/lib.rs:L226`
- **decodes_signed_tx()** — `crates/mev-sequencer/src/lib.rs:L236`
- **L1IncomingMessage** — `crates/mev-sequencer/src/lib.rs:L75`
- **parses_broadcast_json()** — `crates/mev-sequencer/src/lib.rs:L265`
- **Broadcast** — `crates/mev-sequencer/src/lib.rs:L56`
- **FeedMessage** — `crates/mev-sequencer/src/lib.rs:L63`
- **MessageWithMeta** — `crates/mev-sequencer/src/lib.rs:L69`
- **Header** — `crates/mev-sequencer/src/lib.rs:L82`
- **Result** — `graph`
- **String** — `graph`
- **UnboundedSender** — `graph`

## Related communities

- [[docs/knowledge/communities/C026 - lib.rs (C026)|lib.rs (C026)]] — 4 cross-community edges
