---
graphify-generated: true
graphify-community: "62"
graphify-node-count: 33
graphify-internal-edges: 56
tags:
  - graphify/community
---

# RotatingTransport

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-rpc/src/rotate.rs` — 17 nodes
- `graph` — 16 nodes

## Nodes (33)

- **RotatingTransport** — `crates/mev-rpc/src/rotate.rs:L87`
- **rotate.rs** — `crates/mev-rpc/src/rotate.rs:L1`
- **.http_with_fallbacks()** — `crates/mev-rpc/src/rotate.rs:L123`
- **.call()** — `crates/mev-rpc/src/rotate.rs:L147`
- **.new()** — `crates/mev-rpc/src/rotate.rs:L97`
- **.with_primary()** — `crates/mev-rpc/src/rotate.rs:L103`
- **rate_limited()** — `crates/mev-rpc/src/rotate.rs:L57`
- **.http()** — `crates/mev-rpc/src/rotate.rs:L118`
- **.poll_ready()** — `crates/mev-rpc/src/rotate.rs:L143`
- **attempt_order()** — `crates/mev-rpc/src/rotate.rs:L77`
- **Result** — `graph`
- **Self** — `graph`
- **BoxTransport** — `graph`
- **try_next()** — `crates/mev-rpc/src/rotate.rs:L51`
- **Vec** — `graph`
- **fallbacks_come_after_every_primary()** — `crates/mev-rpc/src/rotate.rs:L222`
- **is_execution_error()** — `crates/mev-rpc/src/rotate.rs:L46`
- **is_rate_limit_payload()** — `crates/mev-rpc/src/rotate.rs:L35`
- **now_ms()** — `crates/mev-rpc/src/rotate.rs:L69`
- **RequestPacket** — `graph`
- **ResponsePacket** — `graph`
- **Service** — `graph`
- **String** — `graph`
- **Arc** — `graph`
- **AtomicU64** — `graph`
- **AtomicUsize** — `graph`
- **Context** — `graph`
- **Error** — `graph`
- **Future** — `graph`
- **Iterator** — `graph`
- **only_execution_errors_stop_the_rotation()** — `crates/mev-rpc/src/rotate.rs:L208`
- **Poll** — `graph`
- **recognizes_rate_limit_payloads()** — `crates/mev-rpc/src/rotate.rs:L185`

## Related communities

- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 1 cross-community edges
- [[docs/knowledge/communities/C069 - Item|Item]] — 1 cross-community edges
