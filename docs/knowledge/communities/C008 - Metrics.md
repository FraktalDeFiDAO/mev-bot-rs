---
graphify-generated: true
graphify-community: "8"
graphify-node-count: 71
graphify-internal-edges: 136
tags:
  - graphify/community
---

# Metrics

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `graph` — 33 nodes
- `crates/mev-metrics/src/jsonl.rs` — 21 nodes
- `crates/mev-metrics/src/server.rs` — 17 nodes

## Nodes (71)

- **Metrics** — `crates/mev-metrics/src/jsonl.rs:L74`
- **server.rs** — `crates/mev-metrics/src/server.rs:L1`
- **jsonl.rs** — `crates/mev-metrics/src/jsonl.rs:L1`
- **.new()** — `crates/mev-metrics/src/jsonl.rs:L88`
- **.start()** — `crates/mev-metrics/src/server.rs:L75`
- **ServerHandle** — `crates/mev-metrics/src/server.rs:L67`
- **writer()** — `crates/mev-metrics/src/jsonl.rs:L173`
- **ListenAddrError** — `crates/mev-metrics/src/server.rs:L24`
- **.flush()** — `crates/mev-metrics/src/jsonl.rs:L127`
- **writes_jsonl_events_counters_and_heartbeat()** — `crates/mev-metrics/src/jsonl.rs:L289`
- **.event()** — `crates/mev-metrics/src/jsonl.rs:L141`
- **.local_addr()** — `crates/mev-metrics/src/server.rs:L100`
- **addr_from_env()** — `crates/mev-metrics/src/server.rs:L56`
- **open()** — `crates/mev-metrics/src/jsonl.rs:L236`
- **parse_listen_addr()** — `crates/mev-metrics/src/server.rs:L43`
- **SocketAddr** — `graph`
- **get()** — `crates/mev-metrics/src/server.rs:L180`
- **Msg** — `crates/mev-metrics/src/jsonl.rs:L66`
- **serve()** — `crates/mev-metrics/src/server.rs:L126`
- **.update()** — `crates/mev-metrics/src/jsonl.rs:L153`
- **a_full_queue_drops_events_without_blocking_the_caller()** — `crates/mev-metrics/src/jsonl.rs:L320`
- **a_stalled_client_does_not_hold_the_listener_past_the_deadline()** — `crates/mev-metrics/src/server.rs:L230`
- **Counters** — `crates/mev-metrics/src/jsonl.rs:L23`
- **dropping_the_handle_stops_the_listener()** — `crates/mev-metrics/src/server.rs:L241`
- **event_files_never_contain_rpc_keys()** — `crates/mev-metrics/src/jsonl.rs:L272`
- **events_are_stamped_with_the_mode()** — `crates/mev-metrics/src/jsonl.rs:L249`
- **Result** — `graph`
- **Result** — `graph`
- **serves_metrics_and_rejects_everything_else()** — `crates/mev-metrics/src/server.rs:L211`
- **.fmt()** — `crates/mev-metrics/src/server.rs:L30`
- **.registry()** — `crates/mev-metrics/src/jsonl.rs:L109`
- **.serve_prometheus()** — `crates/mev-metrics/src/jsonl.rs:L114`
- **.snapshot()** — `crates/mev-metrics/src/jsonl.rs:L166`
- **Arc** — `graph`
- **events_of_a_new_day_go_to_a_new_file()** — `crates/mev-metrics/src/jsonl.rs:L348`
- **Registry** — `graph`
- **respond()** — `crates/mev-metrics/src/server.rs:L116`
- **.drop()** — `crates/mev-metrics/src/server.rs:L106`
- **.heartbeat()** — `crates/mev-metrics/src/jsonl.rs:L159`
- **Arc** — `graph`
- **AtomicU64** — `graph`
- **BufWriter** — `graph`
- **File** — `graph`
- **Option** — `graph`
- **PathBuf** — `graph`
- **Registry** — `graph`
- **String** — `graph`
- **String** — `graph`
- **SyncSender** — `graph`
- **TcpStream** — `graph`
- **.dropped()** — `crates/mev-metrics/src/jsonl.rs:L122`
- **.set_live()** — `crates/mev-metrics/src/jsonl.rs:L135`
- **AtomicBool** — `graph`
- **AtomicBool** — `graph`
- **Display** — `graph`
- **Drop** — `graph`
- **env_is_off_by_default()** — `crates/mev-metrics/src/server.rs:L206`
- **Error** — `graph`
- **FnOnce** — `graph`
- **Formatter** — `graph`
- **IpAddr** — `graph`
- **JoinHandle** — `graph`
- **loopback_only_unless_overridden()** — `crates/mev-metrics/src/server.rs:L190`
- **Mutex** — `graph`
- **Option** — `graph`
- **Path** — `graph`
- **Receiver** — `graph`
- **Self** — `graph`
- **Self** — `graph`
- **SocketAddr** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C075 - Exposition|Exposition]] — 2 cross-community edges
- [[docs/knowledge/communities/C014 - Engine|Engine]] — 1 cross-community edges
- [[docs/knowledge/communities/C153 - mod.rs (C153)|mod.rs (C153)]] — 1 cross-community edges
- [[docs/knowledge/communities/C016 - client.rs|client.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C186 - main (C186)|main (C186)]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
- [[docs/knowledge/communities/C007 - ScheduleOutput|ScheduleOutput]] — 1 cross-community edges
