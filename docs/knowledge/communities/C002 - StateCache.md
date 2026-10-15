---
graphify-generated: true
graphify-community: "2"
graphify-node-count: 97
graphify-internal-edges: 249
tags:
  - graphify/community
---

# StateCache

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-localsim/src/lib.rs` — 43 nodes
- `graph` — 40 nodes
- `crates/mev-localsim/src/diff.rs` — 9 nodes
- `crates/mev-localsim/src/bin/mev-diff-probe.rs` — 5 nodes

## Nodes (97)

- **StateCache** — `crates/mev-localsim/src/lib.rs:L48`
- **lib.rs** — `crates/mev-localsim/src/lib.rs:L1`
- **execute()** — `crates/mev-localsim/src/lib.rs:L214`
- **Overlay** — `crates/mev-localsim/src/lib.rs:L293`
- **.push()** — `crates/mev-localsim/src/lib.rs:L353`
- **fetch_address()** — `crates/mev-localsim/src/lib.rs:L433`
- **Key** — `crates/mev-localsim/src/lib.rs:L35`
- **SnapshotDb** — `crates/mev-localsim/src/lib.rs:L116`
- **Address** — `graph`
- **Change** — `crates/mev-localsim/src/diff.rs:L43`
- **diff.rs** — `crates/mev-localsim/src/diff.rs:L1`
- **Pending** — `crates/mev-localsim/src/lib.rs:L336`
- **Vec** — `graph`
- **.basic()** — `crates/mev-localsim/src/lib.rs:L126`
- **.insert_account()** — `crates/mev-localsim/src/lib.rs:L81`
- **pending_txs_chain_through_the_overlay_and_rebuild_on_block()** — `crates/mev-localsim/src/lib.rs:L697`
- **.apply()** — `crates/mev-localsim/src/lib.rs:L476`
- **.new()** — `crates/mev-localsim/src/lib.rs:L66`
- **Outcome** — `crates/mev-localsim/src/lib.rs:L198`
- **Result** — `graph`
- **U256** — `graph`
- **.insert_fetched()** — `crates/mev-localsim/src/lib.rs:L491`
- **.on_block()** — `crates/mev-localsim/src/lib.rs:L376`
- **.storage()** — `crates/mev-localsim/src/lib.rs:L161`
- **AccountState** — `crates/mev-localsim/src/diff.rs:L18`
- **B256** — `graph`
- **fill()** — `crates/mev-localsim/src/lib.rs:L409`
- **simulate()** — `crates/mev-localsim/src/lib.rs:L209`
- **TxDiff** — `crates/mev-localsim/src/diff.rs:L35`
- **.apply_block()** — `crates/mev-localsim/src/lib.rs:L568`
- **changes()** — `crates/mev-localsim/src/diff.rs:L52`
- **fetch_block_diff()** — `crates/mev-localsim/src/diff.rs:L96`
- **main()** — `crates/mev-localsim/src/bin/mev-diff-probe.rs:L31`
- **report()** — `crates/mev-localsim/src/bin/mev-diff-probe.rs:L109`
- **signed()** — `crates/mev-localsim/src/lib.rs:L606`
- **.apply_change()** — `crates/mev-localsim/src/lib.rs:L508`
- **.code_by_hash()** — `crates/mev-localsim/src/lib.rs:L149`
- **.insert_slot()** — `crates/mev-localsim/src/lib.rs:L106`
- **.is_empty()** — `crates/mev-localsim/src/lib.rs:L394`
- **dedup()** — `crates/mev-localsim/src/lib.rs:L399`
- **executes_from_cache_and_reports_misses()** — `crates/mev-localsim/src/lib.rs:L623`
- **Fetched** — `crates/mev-localsim/src/lib.rs:L468`
- **mev-diff-probe.rs** — `crates/mev-localsim/src/bin/mev-diff-probe.rs:L1`
- **StateDiff** — `crates/mev-localsim/src/diff.rs:L27`
- **touches()** — `crates/mev-localsim/src/lib.rs:L580`
- **TxEnvelope** — `graph`
- **.apply_changes()** — `crates/mev-localsim/src/lib.rs:L542`
- **.block_hash()** — `crates/mev-localsim/src/lib.rs:L184`
- **.commit()** — `crates/mev-localsim/src/lib.rs:L303`
- **Infallible** — `graph`
- **late_fill_replays_newer_block_changes()** — `crates/mev-localsim/src/lib.rs:L653`
- **Option** — `graph`
- **provider()** — `crates/mev-localsim/src/bin/mev-diff-probe.rs:L26`
- **Stats** — `crates/mev-localsim/src/bin/mev-diff-probe.rs:L15`
- **tx_hashes()** — `crates/mev-localsim/src/diff.rs:L91`
- **.stale()** — `crates/mev-localsim/src/lib.rs:L110`
- **AccountInfo** — `graph`
- **B256** — `graph`
- **BTreeMap** — `graph`
- **Bytecode** — `graph`
- **Entry** — `crates/mev-localsim/src/lib.rs:L41`
- **HashSet** — `graph`
- **is_system()** — `crates/mev-localsim/src/lib.rs:L190`
- **.is_empty()** — `crates/mev-localsim/src/lib.rs:L77`
- **.is_empty()** — `crates/mev-localsim/src/lib.rs:L328`
- **Address** — `graph`
- **Bytes** — `graph`
- **Bytes** — `graph`
- **diff_mode_semantics()** — `crates/mev-localsim/src/diff.rs:L111`
- **DynProvider** — `graph`
- **EvmState** — `graph`
- **Fetched** — `graph`
- **HashMap** — `graph`
- **Logs** — `graph`
- **Option** — `graph`
- **Result** — `graph`
- **same_logs()** — `crates/mev-localsim/src/lib.rs:L593`
- **U256** — `graph`
- **Vec** — `graph`
- **VecDeque** — `graph`
- **.len()** — `crates/mev-localsim/src/lib.rs:L73`
- **.len()** — `crates/mev-localsim/src/lib.rs:L390`
- **.reset()** — `crates/mev-localsim/src/lib.rs:L559`
- **Arc** — `graph`
- **Database** — `graph`
- **Duration** — `graph`
- **Duration** — `graph`
- **DynProvider** — `graph`
- **DynProvider** — `graph`
- **HashSet** — `graph`
- **Instant** — `graph`
- **Mutex** — `graph`
- **Result** — `graph`
- **Self** — `graph`
- **String** — `graph`
- **T** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C000 - liquidation.rs|liquidation.rs]] — 1 cross-community edges
