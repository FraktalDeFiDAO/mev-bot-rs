---
graphify-generated: true
graphify-community: "38"
graphify-node-count: 46
graphify-internal-edges: 78
tags:
  - graphify/community
---

# ManualClock

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 20 nodes
- `crates/jev-advisor/src/resilience/clock.rs` — 13 nodes
- `crates/jev-advisor/tests/common/manual.rs` — 13 nodes

## Nodes (46)

- **ManualClock** — `crates/jev-advisor/tests/common/manual.rs:L22`
- **Inner** — `crates/jev-advisor/tests/common/manual.rs:L12`
- **clock.rs** — `crates/jev-advisor/src/resilience/clock.rs:L1`
- **Duration** — `graph`
- **TokioClock** — `crates/jev-advisor/src/resilience/clock.rs:L26`
- **.sleep()** — `crates/jev-advisor/tests/common/manual.rs:L115`
- **Clock** — `crates/jev-advisor/src/resilience/clock.rs:L19`
- **SleepPurpose** — `crates/jev-advisor/src/resilience/clock.rs:L10`
- **.sleep()** — `crates/jev-advisor/src/resilience/clock.rs:L48`
- **Deregister** — `crates/jev-advisor/tests/common/manual.rs:L99`
- **manual.rs** — `crates/jev-advisor/tests/common/manual.rs:L1`
- **.earliest()** — `crates/jev-advisor/tests/common/manual.rs:L58`
- **.new()** — `crates/jev-advisor/src/resilience/clock.rs:L62`
- **.pending()** — `crates/jev-advisor/tests/common/manual.rs:L70`
- **.requested()** — `crates/jev-advisor/tests/common/manual.rs:L47`
- **SeededRng** — `crates/jev-advisor/src/resilience/clock.rs:L59`
- **.default()** — `crates/jev-advisor/src/resilience/clock.rs:L39`
- **.drive()** — `crates/jev-advisor/tests/common/manual.rs:L80`
- **.drop()** — `crates/jev-advisor/tests/common/manual.rs:L101`
- **.new()** — `crates/jev-advisor/src/resilience/clock.rs:L31`
- **.now()** — `crates/jev-advisor/src/resilience/clock.rs:L45`
- **Duration** — `graph`
- **JitterSource** — `crates/jev-advisor/src/resilience/clock.rs:L54`
- **Self** — `graph`
- **Send** — `graph`
- **.advance()** — `crates/jev-advisor/tests/common/manual.rs:L37`
- **.new()** — `crates/jev-advisor/tests/common/manual.rs:L25`
- **.now()** — `crates/jev-advisor/tests/common/manual.rs:L111`
- **Arc** — `graph`
- **Future** — `graph`
- **Future** — `graph`
- **Output** — `graph`
- **splitmix_reference_vector()** — `crates/jev-advisor/src/resilience/clock.rs:L82`
- **Vec** — `graph`
- **.next_u64()** — `crates/jev-advisor/src/resilience/clock.rs:L68`
- **Default** — `graph`
- **Drop** — `graph`
- **F** — `graph`
- **Instant** — `graph`
- **Mutex** — `graph`
- **Notify** — `graph`
- **Option** — `graph`
- **Output** — `graph`
- **Self** — `graph`
- **Send** — `graph`
- **Sync** — `graph`

## Related communities

- [[docs/knowledge/communities/C118 - jev-advisor.md|jev-advisor.md]] — 1 cross-community edges
- [[docs/knowledge/communities/C149 - Sender|Sender]] — 1 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C031 - resilience.rs|resilience.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C054 - transport.rs|transport.rs]] — 1 cross-community edges
