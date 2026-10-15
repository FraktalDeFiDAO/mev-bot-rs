---
graphify-generated: true
graphify-community: "100"
graphify-node-count: 22
graphify-internal-edges: 57
tags:
  - graphify/community
---

# build_features

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-features/tests/features.rs` — 17 nodes
- `graph` — 4 nodes
- `crates/mev-liq-features/src/features.rs` — 1 node

## Nodes (22)

- **build_features()** — `crates/mev-liq-features/src/features.rs:L347`
- **spec_input()** — `crates/mev-liq-features/tests/features.rs:L84`
- **reg()** — `crates/mev-liq-features/tests/features.rs:L119`
- **as_of()** — `crates/mev-liq-features/tests/features.rs:L21`
- **data_recorded_after_the_as_of_point_cannot_change_features()** — `crates/mev-liq-features/tests/features.rs:L837`
- **vol_with()** — `crates/mev-liq-features/tests/features.rs:L338`
- **quality_of()** — `crates/mev-liq-features/tests/features.rs:L644`
- **redacted_features_refuse_non_verified_quality()** — `crates/mev-liq-features/tests/features.rs:L1053`
- **simulation_history_bands_use_event_time_only()** — `crates/mev-liq-features/tests/features.rs:L590`
- **digest_changes_iff_a_feature_changes()** — `crates/mev-liq-features/tests/features.rs:L762`
- **invalid_snapshot_hides_account_derived_bands()** — `crates/mev-liq-features/tests/features.rs:L680`
- **recheck_age_uses_monotonic_clock_and_labels_anomalies()** — `crates/mev-liq-features/tests/features.rs:L620`
- **request_key_dedups_and_separates()** — `crates/mev-liq-features/tests/features.rs:L801`
- **same_input_gives_identical_bytes_and_digest()** — `crates/mev-liq-features/tests/features.rs:L751`
- **sims()** — `crates/mev-liq-features/tests/features.rs:L580`
- **spec_example_is_reproduced_byte_for_byte()** — `crates/mev-liq-features/tests/features.rs:L124`
- **stale_or_missing_route_liquidity_is_unknown()** — `crates/mev-liq-features/tests/features.rs:L549`
- **unknown_and_mixed_class_fallbacks()** — `crates/mev-liq-features/tests/features.rs:L487`
- **Vec** — `graph`
- **OracleVolatilityBand** — `graph`
- **Result** — `graph`
- **SnapshotQuality** — `graph`

## Related communities

- [[docs/knowledge/communities/C067 - features.rs (C067)|features.rs (C067)]] — 30 cross-community edges
- [[docs/knowledge/communities/C048 - FeatureV1|FeatureV1]] — 18 cross-community edges
- [[docs/knowledge/communities/C233 - trend_with|trend_with]] — 4 cross-community edges
- [[docs/knowledge/communities/C134 - bands.rs|bands.rs]] — 2 cross-community edges
- [[docs/knowledge/communities/C141 - ClassRegistry|ClassRegistry]] — 2 cross-community edges
- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C009 - audit.rs|audit.rs]] — 1 cross-community edges
