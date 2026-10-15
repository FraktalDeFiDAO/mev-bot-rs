---
graphify-generated: true
graphify-community: "1"
graphify-node-count: 101
graphify-internal-edges: 266
tags:
  - graphify/community
---

# lib.rs (C001)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/seqscope-server/src/lib.rs` — 58 nodes
- `graph` — 30 nodes
- `scripts/analyze-outcomes.py` — 3 nodes
- `crates/mev-presim/tests/after_live.rs` — 2 nodes
- `scripts/build-corpus-truth.py` — 2 nodes
- `scripts/exchange-view.py` — 2 nodes
- `scripts/learn-packed-table.py` — 2 nodes
- `crates/jev-advisor/src/lib.rs` — 1 node
- `infra/arbitrum-node/arbitrum-prefetch-part-one.py` — 1 node

## Nodes (101)

- **lib.rs** — `crates/seqscope-server/src/lib.rs:L1`
- **AppState** — `crates/seqscope-server/src/lib.rs:L65`
- **Json** — `graph`
- **Arc** — `graph`
- **.new()** — `crates/seqscope-server/src/lib.rs:L129`
- **ApiError** — `crates/seqscope-server/src/lib.rs:L255`
- **String** — `graph`
- **sql()** — `crates/seqscope-server/src/lib.rs:L698`
- **preview()** — `crates/seqscope-server/src/lib.rs:L626`
- **rows()** — `crates/seqscope-server/src/lib.rs:L664`
- **gaps()** — `crates/seqscope-server/src/lib.rs:L741`
- **State** — `graph`
- **stats()** — `crates/seqscope-server/src/lib.rs:L717`
- **guard()** — `crates/seqscope-server/src/lib.rs:L286`
- **put_rules()** — `crates/seqscope-server/src/lib.rs:L438`
- **router()** — `crates/seqscope-server/src/lib.rs:L835`
- **rules_op()** — `crates/seqscope-server/src/lib.rs:L605`
- **serve_on()** — `crates/seqscope-server/src/lib.rs:L899`
- **Value** — `graph`
- **get_rules()** — `crates/seqscope-server/src/lib.rs:L360`
- **live()** — `crates/seqscope-server/src/lib.rs:L770`
- **Response** — `graph`
- **Result** — `graph`
- **validate()** — `crates/seqscope-server/src/lib.rs:L122`
- **.into_response()** — `crates/seqscope-server/src/lib.rs:L258`
- **.save_rules()** — `crates/seqscope-server/src/lib.rs:L191`
- **.apply_op()** — `crates/seqscope-server/src/lib.rs:L553`
- **live_socket()** — `crates/seqscope-server/src/lib.rs:L785`
- **RulesState** — `crates/seqscope-server/src/lib.rs:L59`
- **serve()** — `crates/seqscope-server/src/lib.rs:L858`
- **asset()** — `crates/seqscope-server/src/lib.rs:L316`
- **Config** — `crates/seqscope-server/src/lib.rs:L50`
- **explain()** — `crates/seqscope-server/src/lib.rs:L455`
- **RuleOp** — `crates/seqscope-server/src/lib.rs:L513`
- **send()** — `crates/seqscope-server/src/lib.rs:L778`
- **templates()** — `crates/seqscope-server/src/lib.rs:L477`
- **.feed()** — `crates/seqscope-server/src/lib.rs:L213`
- **bad_request()** — `crates/seqscope-server/src/lib.rs:L263`
- **lib.rs** — `crates/jev-advisor/src/lib.rs:L1`
- **LiveParams** — `crates/seqscope-server/src/lib.rs:L764`
- **NewRule** — `crates/seqscope-server/src/lib.rs:L498`
- **Option** — `graph`
- **validate_rules()** — `crates/seqscope-server/src/lib.rs:L431`
- **.status()** — `crates/seqscope-server/src/lib.rs:L230`
- **IntoResponse** — `graph`
- **row_message()** — `crates/seqscope-server/src/lib.rs:L248`
- **RowsParams** — `crates/seqscope-server/src/lib.rs:L644`
- **TextBody** — `crates/seqscope-server/src/lib.rs:L420`
- **Vec** — `graph`
- **.reload_rules_file()** — `crates/seqscope-server/src/lib.rs:L164`
- **analyze-outcomes.py** — `scripts/analyze-outcomes.py:L1`
- **app_css()** — `crates/seqscope-server/src/lib.rs:L342`
- **app_js()** — `crates/seqscope-server/src/lib.rs:L333`
- **counters_rules()** — `crates/seqscope-server/src/lib.rs:L83`
- **GapParams** — `crates/seqscope-server/src/lib.rs:L737`
- **get_catalogue()** — `crates/seqscope-server/src/lib.rs:L356`
- **header_of()** — `crates/seqscope-server/src/lib.rs:L538`
- **health()** — `crates/seqscope-server/src/lib.rs:L352`
- **index()** — `crates/seqscope-server/src/lib.rs:L324`
- **PathBuf** — `graph`
- **PreviewBody** — `crates/seqscope-server/src/lib.rs:L617`
- **SqlBody** — `crates/seqscope-server/src/lib.rs:L689`
- **WhenBody** — `crates/seqscope-server/src/lib.rs:L450`
- **after_live.rs** — `crates/mev-presim/tests/after_live.rs:L1`
- **build-corpus-truth.py** — `scripts/build-corpus-truth.py:L1`
- **Engine** — `graph`
- **exchange-view.py** — `scripts/exchange-view.py:L1`
- **is_loopback_host()** — `crates/seqscope-server/src/lib.rs:L282`
- **learn-packed-table.py** — `scripts/learn-packed-table.py:L1`
- **now_ms()** — `crates/seqscope-server/src/lib.rs:L658`
- **Request** — `graph`
- **shutdown_signal()** — `crates/seqscope-server/src/lib.rs:L909`
- **Source** — `graph`
- **WebSocket** — `graph`
- **.file_changed()** — `crates/seqscope-server/src/lib.rs:L183`
- **a_second_call_is_verified_on_top_of_the_first()** — `crates/mev-presim/tests/after_live.rs:L15`
- **arbitrum-prefetch-part-one.py** — `infra/arbitrum-node/arbitrum-prefetch-part-one.py:L1`
- **AtomicU64** — `graph`
- **call()** — `scripts/learn-packed-table.py:L24`
- **Display** — `graph`
- **Duration** — `graph`
- **five()** — `crates/seqscope-server/src/lib.rs:L622`
- **Future** — `graph`
- **host_only()** — `crates/seqscope-server/src/lib.rs:L270`
- **Instant** — `graph`
- **lines()** — `scripts/analyze-outcomes.py:L25`
- **lines()** — `scripts/exchange-view.py:L22`
- **Mutex** — `graph`
- **one()** — `crates/seqscope-server/src/lib.rs:L507`
- **Output** — `graph`
- **rpc()** — `scripts/build-corpus-truth.py:L27`
- **RwLock** — `graph`
- **Semaphore** — `graph`
- **SocketAddr** — `graph`
- **StatusCode** — `graph`
- **SystemTime** — `graph`
- **TcpListener** — `graph`
- **thousand()** — `crates/seqscope-server/src/lib.rs:L694`
- **tips()** — `scripts/analyze-outcomes.py:L90`
- **VecDeque** — `graph`
- **WebSocketUpgrade** — `graph`

## Related communities

- [[docs/knowledge/communities/C024 - RuleSet|RuleSet]] — 14 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 8 cross-community edges
- [[docs/knowledge/communities/C005 - api.rs|api.rs]] — 7 cross-community edges
- [[docs/knowledge/communities/C205 - spawn|spawn]] — 2 cross-community edges
- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 2 cross-community edges
- [[docs/knowledge/communities/C109 - Hub|Hub]] — 1 cross-community edges
- [[docs/knowledge/communities/C149 - Sender|Sender]] — 1 cross-community edges
- [[docs/knowledge/communities/C154 - Secret|Secret]] — 1 cross-community edges
- [[docs/knowledge/communities/C167 - catalogue.rs|catalogue.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C184 - arbitrum-snapshot-init.py|arbitrum-snapshot-init.py]] — 1 cross-community edges
- [[docs/knowledge/communities/C198 - TipBook|TipBook]] — 1 cross-community edges
- [[docs/knowledge/communities/C210 - aggregate|aggregate]] — 1 cross-community edges
