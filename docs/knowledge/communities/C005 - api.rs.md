---
graphify-generated: true
graphify-community: "5"
graphify-node-count: 80
graphify-internal-edges: 179
tags:
  - graphify/community
---

# api.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/seqscope-server/tests/api.rs` — 34 nodes
- `graph` — 26 nodes
- `crates/seqscope-core/tests/corpus.rs` — 18 nodes
- `crates/seqscope-core/src/tx.rs` — 2 nodes

## Nodes (80)

- **api.rs** — `crates/seqscope-server/tests/api.rs:L1`
- **.new()** — `crates/seqscope-server/tests/api.rs:L32`
- **H** — `crates/seqscope-server/tests/api.rs:L25`
- **corpus.rs** — `crates/seqscope-core/tests/corpus.rs:L1`
- **Tx** — `crates/seqscope-core/src/tx.rs:L9`
- **.get()** — `crates/seqscope-server/tests/api.rs:L96`
- **next_json()** — `crates/seqscope-server/tests/api.rs:L517`
- **.call()** — `crates/seqscope-server/tests/api.rs:L63`
- **.post()** — `crates/seqscope-server/tests/api.rs:L99`
- **corpus()** — `crates/seqscope-core/tests/corpus.rs:L18`
- **all_rows()** — `crates/seqscope-core/tests/corpus.rs:L70`
- **op()** — `crates/seqscope-server/tests/api.rs:L753`
- **.feed_corpus()** — `crates/seqscope-server/tests/api.rs:L108`
- **corpus()** — `crates/seqscope-server/tests/api.rs:L160`
- **engine()** — `crates/seqscope-core/tests/corpus.rs:L57`
- **rules_can_be_validated_saved_and_are_never_half_applied()** — `crates/seqscope-server/tests/api.rs:L315`
- **StreamExt** — `graph`
- **Value** — `graph`
- **.put()** — `crates/seqscope-server/tests/api.rs:L103`
- **a_probe_tries_a_rule_on_history_and_then_on_the_live_stream_without_saving()** — `crates/seqscope-server/tests/api.rs:L571`
- **guards_refuse_foreign_hosts_cross_origin_and_unmarked_changes()** — `crates/seqscope-server/tests/api.rs:L202`
- **rows_sql_stats_and_gaps_read_what_was_stored()** — `crates/seqscope-server/tests/api.rs:L455`
- **rules_are_added_switched_changed_and_removed_without_touching_text()** — `crates/seqscope-server/tests/api.rs:L762`
- **serve_on_local_port()** — `crates/seqscope-server/tests/api.rs:L503`
- **summary()** — `crates/seqscope-core/tests/corpus.rs:L286`
- **Truth** — `crates/seqscope-core/tests/corpus.rs:L29`
- **edits_made_to_the_file_are_picked_up_and_a_broken_edit_is_reported_not_applied()** — `crates/seqscope-server/tests/api.rs:L387`
- **StatusCode** — `graph`
- **the_live_socket_streams_stored_rows_and_status()** — `crates/seqscope-server/tests/api.rs:L543`
- **.wait_rows()** — `crates/seqscope-server/tests/api.rs:L137`
- **a_dry_run_counts_what_a_rule_would_match_over_recent_traffic()** — `crates/seqscope-server/tests/api.rs:L410`
- **Option** — `graph`
- **simultaneous_edits_all_land()** — `crates/seqscope-server/tests/api.rs:L954`
- **starter_sets_are_listed_in_words_and_added_once()** — `crates/seqscope-server/tests/api.rs:L873`
- **the_ui_and_health_are_served()** — `crates/seqscope-server/tests/api.rs:L175`
- **Vec** — `graph`
- **.from_sequenced()** — `crates/seqscope-core/src/tx.rs:L32`
- **a_guided_edit_keeps_the_notes_at_the_top_of_the_file_and_refuses_a_broken_file()** — `crates/seqscope-server/tests/api.rs:L912`
- **a_socket_from_another_site_is_refused()** — `crates/seqscope-server/tests/api.rs:L627`
- **any_rule_can_be_explained_in_plain_english_and_a_bad_one_says_why()** — `crates/seqscope-server/tests/api.rs:L686`
- **damaged_calldata_never_panics_or_hangs()** — `crates/seqscope-core/tests/corpus.rs:L336`
- **every_transaction_produces_at_least_one_call_and_every_row_is_well_formed()** — `crates/seqscope-core/tests/corpus.rs:L91`
- **processing_the_corpus_twice_gives_identical_rows()** — `crates/seqscope-core/tests/corpus.rs:L165`
- **rule()** — `crates/seqscope-core/tests/corpus.rs:L46`
- **String** — `graph`
- **the_catalogue_carries_names_for_pickers()** — `crates/seqscope-server/tests/api.rs:L719`
- **the_catalogue_describes_fields_and_protocols()** — `crates/seqscope-server/tests/api.rs:L293`
- **TruthEvent** — `crates/seqscope-core/tests/corpus.rs:L34`
- **.settled_rows()** — `crates/seqscope-server/tests/api.rs:L116`
- **a_replayed_recording_drives_the_whole_pipeline_with_no_network()** — `crates/seqscope-server/tests/api.rs:L645`
- **decoded_liquidity_changes_match_a_liquidity_event_on_chain()** — `crates/seqscope-core/tests/corpus.rs:L239`
- **decoded_swaps_name_a_pool_the_transaction_really_swapped_in()** — `crates/seqscope-core/tests/corpus.rs:L186`
- **destinations_that_wrap_calls_survive_arbitrary_bytes()** — `crates/seqscope-core/tests/corpus.rs:L376`
- **new_rule()** — `crates/seqscope-server/tests/api.rs:L757`
- **the_corpus_is_what_it_claims_to_be()** — `crates/seqscope-core/tests/corpus.rs:L81`
- **what_the_decoder_reads_has_not_changed_by_accident()** — `crates/seqscope-core/tests/corpus.rs:L304`
- **.drop()** — `crates/seqscope-server/tests/api.rs:L152`
- **Address** — `graph`
- **Arc** — `graph`
- **B256** — `graph`
- **BTreeMap** — `graph`
- **Bytes** — `graph`
- **Drop** — `graph`
- **Duration** — `graph`
- **Error** — `graph`
- **HashMap** — `graph`
- **Message** — `graph`
- **Method** — `graph`
- **Mode** — `graph`
- **next()** — `crates/seqscope-core/tests/corpus.rs:L328`
- **Option** — `graph`
- **Option** — `graph`
- **PathBuf** — `graph`
- **pick()** — `crates/seqscope-core/tests/corpus.rs:L417`
- **Result** — `graph`
- **Self** — `graph`
- **serving_on_a_public_address_is_refused_unless_asked_for()** — `crates/seqscope-server/tests/api.rs:L273`
- **U256** — `graph`
- **Unpin** — `graph`
- **Vec** — `graph`

## Related communities

- [[docs/knowledge/communities/C001 - lib.rs (C001)|lib.rs (C001)]] — 7 cross-community edges
- [[docs/knowledge/communities/C024 - RuleSet|RuleSet]] — 4 cross-community edges
- [[docs/knowledge/communities/C136 - Decoded|Decoded]] — 1 cross-community edges
- [[docs/knowledge/communities/C137 - SequencedTx|SequencedTx]] — 1 cross-community edges
- [[docs/knowledge/communities/C149 - Sender|Sender]] — 1 cross-community edges
- [[docs/knowledge/communities/C205 - spawn|spawn]] — 1 cross-community edges
- [[docs/knowledge/communities/C021 - TxRecorder|TxRecorder]] — 1 cross-community edges
- [[docs/knowledge/communities/C023 - lib.rs (C023)|lib.rs (C023)]] — 1 cross-community edges
- [[docs/knowledge/communities/C036 - main.rs (C036)|main.rs (C036)]] — 1 cross-community edges
- [[docs/knowledge/communities/C044 - liq_shadow.rs|liq_shadow.rs]] — 1 cross-community edges
- [[docs/knowledge/communities/C052 - lib.rs (C052)|lib.rs (C052)]] — 1 cross-community edges
- [[docs/knowledge/communities/C069 - Item|Item]] — 1 cross-community edges
