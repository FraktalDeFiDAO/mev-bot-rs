---
graphify-generated: true
graphify-community: "101"
graphify-node-count: 22
graphify-internal-edges: 43
tags:
  - graphify/community
---

# repo.rs

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `crates/mev-liq-state/tests/repo.rs` — 18 nodes
- `graph` — 4 nodes

## Nodes (22)

- **repo.rs** — `crates/mev-liq-state/tests/repo.rs:L1`
- **snap()** — `crates/mev-liq-state/tests/repo.rs:L17`
- **repo()** — `crates/mev-liq-state/tests/repo.rs:L44`
- **any_u256()** — `crates/mev-liq-state/tests/repo.rs:L455`
- **key()** — `crates/mev-liq-state/tests/repo.rs:L9`
- **reorg_invalidates_provisional_and_orphaned_rows_keeps_ancestor_canon()** — `crates/mev-liq-state/tests/repo.rs:L241`
- **cas_rejects_stale_and_advances_monotonically()** — `crates/mev-liq-state/tests/repo.rs:L96`
- **list_expired_boundaries()** — `crates/mev-liq-state/tests/repo.rs:L299`
- **mark_dirty_by_asset_and_account()** — `crates/mev-liq-state/tests/repo.rs:L316`
- **max_u256_round_trips_through_storage()** — `crates/mev-liq-state/tests/repo.rs:L73`
- **provisional_is_never_promoted_without_checkpoint()** — `crates/mev-liq-state/tests/repo.rs:L197`
- **tampered_digest_is_rejected()** — `crates/mev-liq-state/tests/repo.rs:L128`
- **concurrent_upserts_exactly_one_wins()** — `crates/mev-liq-state/tests/repo.rs:L139`
- **digest_is_deterministic_and_sensitive_to_every_digested_field()** — `crates/mev-liq-state/tests/repo.rs:L377`
- **eligibility_is_exact_at_the_wad_boundary()** — `crates/mev-liq-state/tests/repo.rs:L51`
- **migrations_roll_back_and_reapply()** — `crates/mev-liq-state/tests/repo.rs:L362`
- **state_survives_restart()** — `crates/mev-liq-state/tests/repo.rs:L172`
- **zero_debt_max_hf_is_not_eligible()** — `crates/mev-liq-state/tests/repo.rs:L62`
- **SnapshotRepository** — `graph`
- **TempDir** — `graph`
- **U256** — `graph`
- **Value** — `graph`

## Related communities

- [[docs/knowledge/communities/C018 - BorrowerSnapshot|BorrowerSnapshot]] — 1 cross-community edges
- [[docs/knowledge/communities/C019 - RepoError|RepoError]] — 1 cross-community edges
- [[docs/knowledge/communities/C233 - trend_with|trend_with]] — 1 cross-community edges
- [[docs/knowledge/communities/C093 - Strategy|Strategy]] — 1 cross-community edges
