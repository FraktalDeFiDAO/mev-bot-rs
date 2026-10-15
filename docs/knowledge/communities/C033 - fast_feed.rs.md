---
graphify-generated: true
graphify-community: "33"
graphify-node-count: 47
graphify-internal-edges: 97
tags:
  - graphify/community
---

# fast_feed.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-ingest/src/fast_feed.rs` — 24 nodes
- `graph` — 18 nodes
- `crates/mev-ingest/tests/fast_feed_mock.rs` — 5 nodes

## Nodes (47)

- **fast_feed.rs** — `crates/mev-ingest/src/fast_feed.rs:L1`
- **stream_once()** — `crates/mev-ingest/src/fast_feed.rs:L263`
- **run_with_samples()** — `crates/mev-ingest/src/fast_feed.rs:L230`
- **FastFeedTx** — `crates/mev-ingest/src/fast_feed.rs:L83`
- **FastFeedStats** — `crates/mev-ingest/src/fast_feed.rs:L203`
- **Option** — `graph`
- **run()** — `crates/mev-ingest/src/fast_feed.rs:L220`
- **Backoff** — `crates/mev-ingest/src/fast_feed.rs:L172`
- **decode()** — `crates/mev-ingest/src/fast_feed.rs:L95`
- **frame_text()** — `crates/mev-ingest/src/fast_feed.rs:L117`
- **serve()** — `crates/mev-ingest/tests/fast_feed_mock.rs:L43`
- **String** — `graph`
- **.new()** — `crates/mev-ingest/src/fast_feed.rs:L179`
- **.reset()** — `crates/mev-ingest/src/fast_feed.rs:L196`
- **fast_feed_mock.rs** — `crates/mev-ingest/tests/fast_feed_mock.rs:L1`
- **FeedTransaction** — `crates/mev-ingest/src/fast_feed.rs:L45`
- **GapDetector** — `crates/mev-ingest/src/fast_feed.rs:L143`
- **msg()** — `crates/mev-ingest/src/fast_feed.rs:L343`
- **.next_delay()** — `crates/mev-ingest/src/fast_feed.rs:L187`
- **backoff_doubles_caps_and_resets()** — `crates/mev-ingest/src/fast_feed.rs:L401`
- **num_or_hex()** — `crates/mev-ingest/src/fast_feed.rs:L61`
- **.observe()** — `crates/mev-ingest/src/fast_feed.rs:L148`
- **decodes_a_feed_message_with_numeric_or_hex_positions()** — `crates/mev-ingest/src/fast_feed.rs:L374`
- **Duration** — `graph`
- **FeedMessage** — `crates/mev-ingest/src/fast_feed.rs:L34`
- **json_in_a_binary_frame_is_read_like_text()** — `crates/mev-ingest/src/fast_feed.rs:L354`
- **Result** — `graph`
- **UnboundedSender** — `graph`
- **a_wrong_key_never_yields_messages()** — `crates/mev-ingest/tests/fast_feed_mock.rs:L110`
- **Arc** — `graph`
- **binary_json_frames_with_a_bearer_key_are_decoded_and_sampled()** — `crates/mev-ingest/tests/fast_feed_mock.rs:L71`
- **Continuity** — `crates/mev-ingest/src/fast_feed.rs:L128`
- **gap_detector_flags_skips_and_stale_messages()** — `crates/mev-ingest/src/fast_feed.rs:L386`
- **Instant** — `graph`
- **message()** — `crates/mev-ingest/tests/fast_feed_mock.rs:L31`
- **String** — `graph`
- **Value** — `graph`
- **.reset()** — `crates/mev-ingest/src/fast_feed.rs:L165`
- **AtomicU64** — `graph`
- **B256** — `graph`
- **D** — `graph`
- **Error** — `graph`
- **Message** — `graph`
- **Self** — `graph`
- **TcpListener** — `graph`
- **TxEnvelope** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
