---
graphify-generated: true
graphify-community: "4"
graphify-node-count: 86
graphify-internal-edges: 195
tags:
  - graphify/community
---

# mod.rs (C004)

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/copper-jev-advisor/tests/common/mod.rs` — 42 nodes
- `graph` — 33 nodes
- `crates/copper-jev-advisor/tests/e2e.rs` — 11 nodes

## Nodes (86)

- **mod.rs** — `crates/copper-jev-advisor/tests/common/mod.rs:L1`
- **e2e.rs** — `crates/copper-jev-advisor/tests/e2e.rs:L1`
- **init_logs()** — `crates/copper-jev-advisor/tests/common/mod.rs:L455`
- **start()** — `crates/copper-jev-advisor/tests/e2e.rs:L16`
- **client_cfg()** — `crates/copper-jev-advisor/tests/common/mod.rs:L383`
- **Env** — `crates/copper-jev-advisor/tests/common/mod.rs:L285`
- **Vec** — `graph`
- **handle()** — `crates/copper-jev-advisor/tests/common/mod.rs:L105`
- **items_flow_export_to_shadow_decision_to_ack()** — `crates/copper-jev-advisor/tests/e2e.rs:L40`
- **TestServer** — `crates/copper-jev-advisor/tests/common/mod.rs:L63`
- **wait_until()** — `crates/copper-jev-advisor/tests/common/mod.rs:L395`
- **assert_logs_clean()** — `crates/copper-jev-advisor/tests/common/mod.rs:L498`
- **RootRecorder** — `crates/copper-jev-advisor/tests/common/mod.rs:L430`
- **.new()** — `crates/copper-jev-advisor/tests/common/mod.rs:L292`
- **a_superseded_export_is_skipped_without_a_request()** — `crates/copper-jev-advisor/tests/e2e.rs:L322`
- **CaptureWriter** — `crates/copper-jev-advisor/tests/common/mod.rs:L410`
- **done()** — `crates/copper-jev-advisor/tests/e2e.rs:L24`
- **Script** — `crates/copper-jev-advisor/tests/common/mod.rs:L37`
- **Capture** — `crates/copper-jev-advisor/tests/common/mod.rs:L408`
- **crash_without_shutdown_is_replayed_after_lease_expiry()** — `crates/copper-jev-advisor/tests/e2e.rs:L252`
- **rate_limit_trips_the_breaker_and_the_worker_recovers()** — `crates/copper-jev-advisor/tests/e2e.rs:L114`
- **Recorded** — `crates/copper-jev-advisor/tests/common/mod.rs:L49`
- **shutdown_mid_flight_releases_leases_and_restart_finishes_exactly_once()** — `crates/copper-jev-advisor/tests/e2e.rs:L192`
- **tampered_export_is_dead_lettered_and_never_sent()** — `crates/copper-jev-advisor/tests/e2e.rs:L294`
- **unreachable_api_degrades_without_crashing_and_keeps_rows()** — `crates/copper-jev-advisor/tests/e2e.rs:L153`
- **.raw_args()** — `crates/copper-jev-advisor/tests/common/mod.rs:L340`
- **ok()** — `crates/copper-jev-advisor/tests/common/mod.rs:L183`
- **.on_new_span()** — `crates/copper-jev-advisor/tests/common/mod.rs:L436`
- **.spawn()** — `crates/copper-jev-advisor/tests/common/mod.rs:L70`
- **Arc** — `graph`
- **decision_rows()** — `crates/copper-jev-advisor/tests/e2e.rs:L28`
- **features_n()** — `crates/copper-jev-advisor/tests/common/mod.rs:L245`
- **Mutex** — `graph`
- **.export_raw()** — `crates/copper-jev-advisor/tests/common/mod.rs:L314`
- **.export()** — `crates/copper-jev-advisor/tests/common/mod.rs:L310`
- **.key_file()** — `crates/copper-jev-advisor/tests/common/mod.rs:L332`
- **account()** — `crates/copper-jev-advisor/tests/common/mod.rs:L209`
- **http()** — `crates/copper-jev-advisor/tests/common/mod.rs:L164`
- **logs()** — `crates/copper-jev-advisor/tests/common/mod.rs:L475`
- **snapshot()** — `crates/copper-jev-advisor/tests/common/mod.rs:L217`
- **SocketAddr** — `graph`
- **spans()** — `crates/copper-jev-advisor/tests/common/mod.rs:L480`
- **String** — `graph`
- **.advisor_path()** — `crates/copper-jev-advisor/tests/common/mod.rs:L306`
- **.config()** — `crates/copper-jev-advisor/tests/common/mod.rs:L373`
- **.recorded()** — `crates/copper-jev-advisor/tests/common/mod.rs:L96`
- **.snapshot_path()** — `crates/copper-jev-advisor/tests/common/mod.rs:L302`
- **.write()** — `crates/copper-jev-advisor/tests/common/mod.rs:L413`
- **closed_port_addr()** — `crates/copper-jev-advisor/tests/common/mod.rs:L196`
- **PathBuf** — `graph`
- **pool()** — `crates/copper-jev-advisor/tests/common/mod.rs:L205`
- **too_many()** — `crates/copper-jev-advisor/tests/common/mod.rs:L191`
- **valid_body()** — `crates/copper-jev-advisor/tests/common/mod.rs:L178`
- **.flush()** — `crates/copper-jev-advisor/tests/common/mod.rs:L417`
- **.header()** — `crates/copper-jev-advisor/tests/common/mod.rs:L55`
- **.make_writer()** — `crates/copper-jev-advisor/tests/common/mod.rs:L424`
- **Arc** — `graph`
- **CancellationToken** — `graph`
- **dir_of()** — `crates/copper-jev-advisor/tests/common/mod.rs:L514`
- **Duration** — `graph`
- **Path** — `graph`
- **Result** — `graph`
- **S** — `graph`
- **Self** — `graph`
- **.hits()** — `crates/copper-jev-advisor/tests/common/mod.rs:L100`
- **Address** — `graph`
- **AtomicUsize** — `graph`
- **Attributes** — `graph`
- **B256** — `graph`
- **Box** — `graph`
- **Config** — `graph`
- **Context** — `graph`
- **Env** — `graph`
- **FnMut** — `graph`
- **Handle** — `graph`
- **Id** — `graph`
- **Layer** — `graph`
- **longest_hex_run()** — `crates/copper-jev-advisor/tests/common/mod.rs:L485`
- **MakeWriter** — `graph`
- **Option** — `graph`
- **SnapshotRepository** — `graph`
- **String** — `graph`
- **TcpStream** — `graph`
- **TempDir** — `graph`
- **Vec** — `graph`
- **Write** — `graph`

## Related communities

- [[docs/knowledge/communities/C015 - Worker|Worker]] — 2 cross-community edges
- [[docs/knowledge/communities/C065 - RedactedFeatures|RedactedFeatures]] — 2 cross-community edges
- [[docs/knowledge/communities/C118 - jev-advisor.md|jev-advisor.md]] — 1 cross-community edges
- [[docs/knowledge/communities/C129 - config.rs (C129)|config.rs (C129)]] — 1 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C231 - check|check]] — 1 cross-community edges
- [[docs/knowledge/communities/C032 - Pool|Pool]] — 1 cross-community edges
- [[docs/knowledge/communities/C041 - TransportError|TransportError]] — 1 cross-community edges
