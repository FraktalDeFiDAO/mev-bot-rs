# Cargo Workspace Layout

Every component of the bot is its own crate, so the repo works as a set of **building blocks**:
any crate can be reused by another bot, tool, or strategy without pulling in the rest.
Crates depend only "downward" in the layers below, so each one can be tested in isolation and
swapped without touching others.

**Building-block rules**
- Pure logic crates (`mev-types`, `mev-dex`, `mev-state`, `mev-search`, `mev-risk`) do no I/O.
- Anything that talks to the network (`mev-rpc`, `mev-ingest`, `mev-discovery`, `mev-sim`, `mev-exec`)
  is its own crate with a small public API.
- `mev-bot` only wires crates together; no strategy logic lives in the binary.

```
mev-bot/
├── Cargo.toml                # [workspace] — shared deps & versions pinned here
├── contracts/                # Foundry project (Solidity executor) — not a Rust crate
├── config/                   # per-chain TOML (arbitrum.toml, base.toml, ...)
└── crates/
    ├── mev-types/            # L0
    ├── mev-config/           # L0
    ├── mev-bindings/         # L0
    ├── mev-rpc/              # L1
    ├── mev-ingest/           # L1
    ├── mev-dex/              # L2
    ├── mev-discovery/        # L2
    ├── mev-lending/          # L2
    ├── mev-oracle/           # L2
    ├── mev-state/            # L3
    ├── mev-sim/              # L3
    ├── mev-search/           # L4
    ├── mev-liquidate/        # L4
    ├── mev-risk/             # L4
    ├── mev-exec/             # L5
    ├── mev-metrics/          # L5
    ├── mev-analytics/        # app (Phase 0 research CLI)
    └── mev-bot/              # app (main binary)
```

## Crates

| Layer | Crate | Responsibility | Key deps |
|---|---|---|---|
| L0 | `mev-types` | Shared domain types: `ChainId`, `Token`, `PoolId`, `Opportunity`, `Route`, `Position`, errors | alloy-primitives |
| L0 | `mev-config` | Load/validate per-chain TOML, env-based secrets (keys never in files) | serde, toml |
| L0 | `mev-bindings` | Rust bindings for our executor + external contracts (pools, lenders, oracles) via `sol!` | alloy-sol-types |
| L1 | `mev-rpc` | Provider pool across free/paid endpoints: rotation, failover, per-provider quota & rate-limit tracking, latency scoring | alloy-provider |
| L1 | `mev-ingest` | Arbitrum sequencer feed client, OP Stack flashblocks, WS log subscriptions → normalized event stream | tokio, tokio-tungstenite |
| L2 | `mev-discovery` | Find pools via configured DEX factories, load pool state, token decimals, vault balances (Multicall3) | mev-rpc, mev-dex, mev-state |
| L2 | `mev-dex` | Pool math per DEX type (Uni v2/v3/v4, Aerodrome/Velodrome, Curve, Balancer, Camelot): quote, tick traversal, swap encoding | mev-types |
| L2 | `mev-lending` | Protocol adapters (Aave forks, Compound v3, Morpho, Euler v2, Silo...): health factor, close factor, bonus, liquidation call encoding | mev-types |
| L2 | `mev-oracle` | Chainlink/Redstone/Pyth price tracking; detect pending oracle updates in the feed | mev-types, mev-ingest |
| L3 | `mev-state` | In-memory world state: pool reserves/ticks, lending positions, token metadata; updated from `mev-ingest` | mev-dex, mev-lending |
| L3 | `mev-sim` | Local revm simulation against forked state; returns profit, gas, L1 data fee | revm |
| L4 | `mev-search` | Arb cycle detection (bounded DFS / Bellman-Ford) + optimal input sizing | mev-state, mev-dex |
| L4 | `mev-liquidate` | Liquidation queue by distance-to-liquidation, opportunity builder incl. flash-loan source choice | mev-state, mev-lending, mev-oracle |
| L4 | `mev-risk` | Per-tx & daily gas caps, failure-streak kill switch, token allow/deny lists, honeypot checks | mev-types |
| L5 | `mev-exec` | Build packed calldata, nonce mgmt, gas/priority-fee bidding, multi-endpoint broadcast, receipt tracking | mev-rpc, mev-bindings, mev-risk |
| L5 | `mev-metrics` | PnL ledger, opportunity/shadow-mode logs, Prometheus metrics | mev-types |
| app | `mev-analytics` | Phase 0 CLI: historical liquidation & arb scans, competitor analysis, ranked opportunity table | mev-rpc, mev-dex, mev-lending |
| app | `mev-bot` | Main binary: wires crates together per chain config; modes `shadow` / `live` | all |

## Rules

- Dependencies only point to lower layers; no cycles. Apps (`mev-bot`, `mev-analytics`) are the
  only crates allowed to depend on everything.
- Shared dependency versions live in `[workspace.dependencies]` in the root `Cargo.toml`.
- Each crate has its own unit tests; fork/integration tests live in the crate that owns the behavior
  (e.g. `mev-sim` tests replay real historical txs).
- Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` — latency matters.
- Target both `x86_64` and `aarch64` (the free Oracle Cloud VPS is ARM).
