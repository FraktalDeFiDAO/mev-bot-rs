---
graphify-generated: true
graphify-community: "41"
graphify-node-count: 43
graphify-internal-edges: 79
tags:
  - graphify/community
---

# TransportError

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 19 nodes
- `crates/jev-advisor/src/client.rs` — 14 nodes
- `crates/jev-advisor/src/error.rs` — 8 nodes
- `crates/jev-advisor/src/response.rs` — 1 node
- `crates/jev-advisor/tests/transport.rs` — 1 node

## Nodes (43)

- **TransportError** — `crates/jev-advisor/src/error.rs:L59`
- **ClientConfig** — `crates/jev-advisor/src/client.rs:L23`
- **client.rs** — `crates/jev-advisor/src/client.rs:L1`
- **TypeSafeClient** — `crates/jev-advisor/src/client.rs:L141`
- **.exchange()** — `crates/jev-advisor/src/client.rs:L198`
- **.new()** — `crates/jev-advisor/src/client.rs:L160`
- **error.rs** — `crates/jev-advisor/src/error.rs:L1`
- **.send()** — `crates/jev-advisor/src/client.rs:L304`
- **validate_endpoint()** — `crates/jev-advisor/src/client.rs:L80`
- **.new()** — `crates/jev-advisor/src/client.rs:L46`
- **parse_retry_after()** — `crates/jev-advisor/src/client.rs:L115`
- **cfg_err()** — `crates/jev-advisor/src/client.rs:L74`
- **Result** — `graph`
- **sanitize()** — `crates/jev-advisor/src/error.rs:L124`
- **schema()** — `crates/jev-advisor/src/response.rs:L261`
- **String** — `graph`
- **Transport** — `crates/jev-advisor/src/client.rs:L133`
- **.fmt()** — `crates/jev-advisor/src/client.rs:L151`
- **.retry_after()** — `crates/jev-advisor/src/error.rs:L115`
- **Duration** — `graph`
- **map_reqwest()** — `crates/jev-advisor/src/client.rs:L285`
- **String** — `graph`
- **.class()** — `crates/jev-advisor/src/error.rs:L106`
- **.loopback()** — `crates/jev-advisor/src/client.rs:L62`
- **CancellationToken** — `graph`
- **Duration** — `graph`
- **Error** — `graph`
- **ErrorClass** — `crates/jev-advisor/src/error.rs:L96`
- **Option** — `graph`
- **retry_after_parser()** — `crates/jev-advisor/tests/transport.rs:L193`
- **RetryableKind** — `crates/jev-advisor/src/error.rs:L17`
- **Self** — `graph`
- **TimeoutKind** — `crates/jev-advisor/src/error.rs:L9`
- **Url** — `graph`
- **Vec** — `graph`
- **Client** — `graph`
- **Debug** — `graph`
- **Formatter** — `graph`
- **HeaderValue** — `graph`
- **Option** — `graph`
- **Send** — `graph`
- **Sync** — `graph`
- **SystemTime** — `graph`

## Related communities

- [[docs/knowledge/communities/C049 - response.rs|response.rs]] — 13 cross-community edges
- [[docs/knowledge/communities/C054 - transport.rs|transport.rs]] — 7 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 6 cross-community edges
- [[docs/knowledge/communities/C154 - Secret|Secret]] — 3 cross-community edges
- [[docs/knowledge/communities/C001 - lib.rs (C001)|lib.rs (C001)]] — 2 cross-community edges
- [[docs/knowledge/communities/C139 - MetricsInner|MetricsInner]] — 2 cross-community edges
- [[docs/knowledge/communities/C015 - Worker|Worker]] — 2 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 2 cross-community edges
- [[docs/knowledge/communities/C118 - jev-advisor.md|jev-advisor.md]] — 1 cross-community edges
- [[docs/knowledge/communities/C129 - config.rs (C129)|config.rs (C129)]] — 1 cross-community edges
- [[docs/knowledge/communities/C031 - resilience.rs|resilience.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C004 - mod.rs (C004)|mod.rs (C004)]] — 1 cross-community edges
