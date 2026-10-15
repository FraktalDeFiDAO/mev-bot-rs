---
graphify-generated: true
graphify-community: "16"
graphify-node-count: 54
graphify-internal-edges: 132
tags:
  - graphify/community
---

# client.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/jev-advisor/src/resilience/client.rs` — 28 nodes
- `graph` — 23 nodes
- `crates/jev-advisor/src/request.rs` — 2 nodes
- `crates/mev-engine/src/pending.rs` — 1 node

## Nodes (54)

- **client.rs** — `crates/jev-advisor/src/resilience/client.rs:L1`
- **ResilienceConfig** — `crates/jev-advisor/src/resilience/client.rs:L54`
- **AdvisoryRequest** — `crates/jev-advisor/src/request.rs:L42`
- **.call_inner()** — `crates/jev-advisor/src/resilience/client.rs:L273`
- **.lead()** — `crates/jev-advisor/src/resilience/client.rs:L417`
- **.new()** — `crates/jev-advisor/src/resilience/client.rs:L207`
- **State** — `crates/jev-advisor/src/resilience/client.rs:L149`
- **lock()** — `crates/jev-advisor/src/resilience/client.rs:L157`
- **Duration** — `graph`
- **ResilientClient** — `crates/jev-advisor/src/resilience/client.rs:L196`
- **ResilientClient<T, C, J>** — `crates/jev-advisor/src/resilience/client.rs:L206`
- **Role** — `crates/jev-advisor/src/resilience/client.rs:L182`
- **.follow()** — `crates/jev-advisor/src/resilience/client.rs:L353`
- **LeaderGuard** — `crates/jev-advisor/src/resilience/client.rs:L163`
- **Outcome** — `crates/jev-advisor/src/resilience/client.rs:L137`
- **Settled** — `crates/mev-engine/src/pending.rs:L52`
- **.call()** — `crates/jev-advisor/src/resilience/client.rs:L253`
- **.role()** — `crates/jev-advisor/src/resilience/client.rs:L332`
- **.slot()** — `crates/jev-advisor/src/resilience/client.rs:L385`
- **CancellationToken** — `graph`
- **.validate()** — `crates/jev-advisor/src/resilience/client.rs:L83`
- **B256** — `graph`
- **CacheConfig** — `crates/jev-advisor/src/resilience/client.rs:L33`
- **local()** — `crates/jev-advisor/src/resilience/client.rs:L570`
- **.drop()** — `crates/jev-advisor/src/resilience/client.rs:L170`
- **Arc** — `graph`
- **Result** — `graph`
- **Source** — `crates/jev-advisor/src/resilience/client.rs:L125`
- **.default()** — `crates/jev-advisor/src/resilience/client.rs:L69`
- **.snapshot()** — `crates/jev-advisor/src/resilience/client.rs:L237`
- **Followed** — `crates/jev-advisor/src/resilience/client.rs:L189`
- **Mutex** — `graph`
- **Self** — `graph`
- **T** — `graph`
- **.config_changed()** — `crates/jev-advisor/src/resilience/client.rs:L233`
- **.config()** — `crates/jev-advisor/src/resilience/client.rs:L227`
- **.default()** — `crates/jev-advisor/src/resilience/client.rs:L44`
- **C** — `graph`
- **ConfigError** — `graph`
- **Default** — `graph`
- **FallbackReason** — `graph`
- **Flight** — `graph`
- **HashMap** — `graph`
- **InflightPolicy** — `crates/jev-advisor/src/resilience/client.rs:L25`
- **J** — `graph`
- **Option** — `graph`
- **Receiver** — `graph`
- **.body()** — `crates/jev-advisor/src/request.rs:L48`
- **Debug** — `graph`
- **Drop** — `graph`
- **MutexGuard** — `graph`
- **OwnedSemaphorePermit** — `graph`
- **Semaphore** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 6 cross-community edges
- [[docs/knowledge/communities/C065 - RedactedFeatures|RedactedFeatures]] — 6 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 4 cross-community edges
- [[docs/knowledge/communities/C049 - response.rs|response.rs]] — 4 cross-community edges
- [[docs/knowledge/communities/C139 - MetricsInner|MetricsInner]] — 3 cross-community edges
- [[docs/knowledge/communities/C031 - resilience.rs|resilience.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C066 - Breaker|Breaker]] — 3 cross-community edges
- [[docs/knowledge/communities/C085 - quota.rs|quota.rs]] — 3 cross-community edges
- [[docs/knowledge/communities/C116 - cache.rs|cache.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C078 - pending.rs|pending.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C105 - bad|bad]] — 1 cross-community edges
- [[docs/knowledge/communities/C129 - config.rs (C129)|config.rs (C129)]] — 1 cross-community edges
