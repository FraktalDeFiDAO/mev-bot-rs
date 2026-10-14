# JEV-LIQ-007: feature contract, provenance and leakage guards

Crate `mev-liq-features` (pure leaf: `mev-liq-state` types + alloy primitives; no I/O, clock or
network). Spec: `docs/copper_jev_liquidation_architecture.md` sections 0 (inv. 5, 8), 6, 9, 13.

**Bands and `hf_bps` are advisory hints, never eligibility, profit or safety evidence.**
Eligibility stays `BorrowerSnapshot::eligible()` + REVM. A `below_1.00` band does not mean
liquidatable.

## Fields (`copper_liq_features_v1`, canonical order)

| Field | Values | Source / rule |
|---|---|---|
| `schema_version` | `copper_liq_features_v1` | constant, part of the digest |
| `chain` | arbitrum_one, unknown | chain id 42161 |
| `protocol` | aave_v3, unknown | caller-supplied |
| `health_factor_band` | no_debt, below_1.00, 1.00_to_1.02, 1.02_to_1.05, 1.05_to_1.10, 1.10_to_1.25, 1.25_to_2.00, above_2.00, unknown | exact WAD edges, lower-inclusive; debt 0 => no_debt; invalid snapshot => unknown |
| `debt_usd_band` | zero, under_1k, 1k_to_10k, 10k_to_100k, 100k_to_500k, 500k_to_1m, above_1m, unknown | `total_debt_base` vs `usd * base_currency_unit` (checked mul; zero unit/overflow => unknown) |
| `collateral_class`, `debt_class` | stablecoin, volatile_eth_correlated, volatile_btc_correlated, other_volatile, mixed, unknown | `policies/liq_token_classes_v1.toml`; sides use nonzero reserve balances; any unlisted asset or empty reserve list => unknown; differing classes => mixed |
| `oracle_volatility_band` | low <25, medium <100, high <300, extreme >=300 bps, unknown | (max-min)*1e4/min over the 1h window ending at as-of; needs >=3 points, newest <=30 min old; negative/zero/>2^184 price, same-block conflict => unknown; points after as-of ignored |
| `hf_trend` | falling, stable, rising, unknown | previous canonical vs current canonical; stable within +-50 bps; interval 2s..600s, previous block strictly earlier and not after as-of; zero-debt => unknown |
| `liquidity_to_debt_band` | low <1x, medium <5x, high, unknown | route liquidity (base units, no route identity); older than 60s, after as-of, or zero debt => unknown |
| `historical_route_success_band` | low <25%, medium, high >=75%, unknown | last <=20 outcomes with `recorded_at <= as_of`; <5 samples => unknown |
| `last_account_recheck_age_band` | under_30s, 30s_to_2m, 2m_to_10m, 10m_to_1h, stale, unknown | scheduler monotonic clock; >=1h => stale; last-check later than now => unknown |
| `snapshot_quality` | verified_canonical, provisional, stale, invalid | verified only for Canonical + Valid + digest intact + age <=120s |

`hf_bps(hf_wad) = floor(hf_wad * 10000 / 1e18)` in U512, exact for every U256 (informational,
not serialized).

## Determinism and as-of rules

- `build_features(&FeatureInput, &AsOf) -> Result<FeatureV1, FeatureError>` is pure. Input
  ordering is irrelevant (series are sorted).
- A current snapshot with block or observation time after the as-of point is an error. Future
  oracle points, simulation outcomes and liquidity are ignored. `previous_canonical(key,
  before_block)` (migration 3) enforces `block_number < before_block` in SQL.
- Canonical JSON: fixed field order, compact, string values only. Digest =
  `keccak256("mev-liq-features/digest/v1\0" || schema || \0 || json)`.
- `RequestKey { feature_hash, model, prompt_hash }` (+ `id()`), model validated `[A-Za-z0-9._-]{1,64}`.

## Redaction

`FeatureV1` is not `Serialize`; fields are closed enums, so no address, amount, URL, key or route
is representable. `RedactedFeatures::new` requires `verified_canonical` (provisional/stale/invalid
are refused) and scans the JSON with `forbidden_pattern` (hex run >= 40, `0x`, `://`, bearer,
authorization, private key, mnemonic, secret, api key, `sk-`, rpc_url, PEM). It is the only
serializable type and is what HTTP code (JEV-LIQ-009) must send. The AccountKey correlation stays
local in the queue.

## Migration 3 (`snapshot_history`)

Per account, canonical + valid snapshots only (written canonical via `upsert_cas`, or promoted by
`checkpoint_canonical`): `(chain_id, pool, borrower, block_number)` PK with hash, observation time,
hf, debt. Retention: newest 32 per account. Reorg (`invalidate_from_reorg`) deletes rows above the
ancestor or at its height on another hash. Down: `DROP TABLE snapshot_history`. `SCHEMA_VERSION = 3`.

## Known gaps

- Reserve balances are not populated by the Aave rechecker yet, so collateral/debt classes are
  `unknown` in production until a task reads `getUserReserveData`.
- No oracle-price history table exists in `mev-liq-state` (only the latest price); the caller
  supplies the series. `debt_usd_band` relies on the single-block `getUserAccountData` for oracle
  consistency.
- Thresholds are first-cut design choices; calibrate in JEV-LIQ-016/017 replay and rebaseline.
- Token registry addresses come from `config/arbitrum.toml`; not re-fetched on-chain here.
