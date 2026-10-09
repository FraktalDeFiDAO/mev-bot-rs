# Cargo Workspace Layout

Every component of the bot is its own crate, so the repo works as a set of **building blocks**:
any crate can be reused by another bot, tool, or strategy without pulling in the rest.
Crates depend only "downward" in the layers below, so each one can be tested in isolation and
swapped without touching others.

**Building-block rules**
- Pure logic crates (`mev-types`, `mev-dex`, `mev-state`, `mev-search`, `mev-risk`) do no I/O.
- Anything that talks to the network (`mev-rpc`, `mev-ingest`, `mev-discovery`, `mev-sim`, `mev-exec`)
  is its own crate with a small public API.
- `mev-engine` composes the blocks into a runnable bot; `mev-bot` is only CLI, logging, config and secrets.
- Every crate must build alone: `for c in crates/*; do cargo check -p $(basename $c); done`.

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
    ├── mev-atlas/            # L1  (optional Atlas/SVR solver protocol: auction feed, EIP-712 signing)
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
    ├── mev-engine/           # app layer: reusable bot runtime
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
| app | `mev-engine` | Reusable runtime: bootstraps the blocks for one chain and runs event → search → sim → risk → send → receipt. Embeddable by any binary | all L0–L5 |
| app | `mev-bot` | CLI binary: parses args, loads config + secrets, runs `mev-engine` in `shadow` / `live` mode | mev-config, mev-engine |

## Rules

- Dependencies only point to lower layers; no cycles. Apps (`mev-bot`, `mev-analytics`) are the
  only crates allowed to depend on everything.
- Shared dependency versions live in `[workspace.dependencies]` in the root `Cargo.toml`.
- Each crate has its own unit tests; fork/integration tests live in the crate that owns the behavior
  (e.g. `mev-sim` tests replay real historical txs).
- Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` — latency matters.
- Target both `x86_64` and `aarch64` (the free Oracle Cloud VPS is ARM).

## Liquidation building blocks (added 2026-10-03)

| Layer | Crate | Responsibility |
|---|---|---|
| L1 | `mev-sequencer` | Arbitrum sequencer feed client: Nitro L2 message decoding → alloy `TxEnvelope` stream (~0.9 s ahead of RPC receipts) |
| L0 | `mev-decode` | Registry over the `mev-protocol-*` crates and the `mev-decode` CLI; standalone decoding, usable alone (`mev-decode` CLI) or as a library: swaps from calldata, swap/liquidity/liquidation/state event table, prices from logs, identified contracts. No RPC/engine/state dependencies |
| L0 | `mev-protocol` | Core types for protocol crates: `Protocol` trait, `EventKind`, `SwapIntent`, `LiquidationIntent`, `PoolRef`, `PriceUpdate`, ABI word helpers |
| L0 | `mev-protocol-uniswap-v2` | V2-style pair events (Uniswap, Sushi, Camelot V2): swap, sync, mint, burn |
| L0 | `mev-protocol-uniswap-v3` | V3-style pool events (Uniswap, Sushi, Algebra topic) and position-manager liquidity events |
| L0 | `mev-protocol-uniswap-v4` | V4 PoolManager swap and modify-liquidity events |
| L0 | `mev-protocol-uniswap-router` | Calldata of SwapRouter02, SwapRouter v1 and the Universal Router: exact-in V2/V3 swaps, multicall, commands |
| L0 | `mev-protocol-pancake-v3` | PancakeSwap V3 swap event |
| L0 | `mev-protocol-balancer` | Balancer V2 Vault swap event |
| L0 | `mev-protocol-aave-v3` | Aave V3 `LiquidationCall` event and `liquidationCall` calldata |
| L0 | `mev-protocol-morpho-blue` | Morpho Blue `Liquidate` event and `liquidate` calldata (with market id) |
| L0 | `mev-protocol-compound-v3` | Compound III absorb events and `absorb` calldata on the four configured comets |
| L0 | `mev-protocol-fluid` | Fluid vaults: `LogLiquidate` / `LogAbsorb` events and `liquidate` calldata (by price range, so a vault, not an account) |
| L0 | `mev-protocol-unidentified` | The two unidentified swap contracts (A, B) read from landed transactions |
| L2 | `mev-outcomes` | Chain-side ground truth: sampled `eth_getBlockReceipts` → `chaintx-DATE.jsonl` (status, gas, block position, swap/liquidity/liquidation events) joined to the feed recording on the tx hash; coverage audit of the decoders |
| L1 | `mev-oracle` | Resolve price sources (Chainlink proxies, Aave CAPO adapters, Morpho oracles) to aggregators; `FeedWatcher` matches feed txs incl. Chainlink forwarder `forward(aggregator, …)` |
| L1 | `mev-lending` | **The `LiquidationAdapter` trait** + `Plan`/`ProtocolCall`/`Health`/`AccountRef` — every venue implements it |
| L2 | `mev-lending-index` | Shared event-scan account index (newest-first backfill, forward scan, range shrinking, throttle, JSON cache + migration) |
| L2 | `mev-lending-aave-v3` | Aave v3 adapter (Live) + pure Aave math |
| L2 | `mev-lending-compound-v3` | Compound III adapter (Live): absorb + buyCollateral, storefront purchases; pure Comet math |
| L2 | `mev-lending-morpho` | Morpho Blue adapter (Live); pure Morpho math |
| L2 | `mev-lending-fluid` | Fluid vaults adapter (Live, resolver-driven); native-ETH collateral via executor wrapping |
| L2 | `mev-lending-silo` | Silo v2 — Inactive (≈ $0.09 total debt on Arbitrum); verified addresses + plan |
| L2 | `mev-lending-dolomite`, `mev-lending-gearbox` | AuditPending — verified facts + go-live checklists |
| L2 | `mev-lending-radiant`, `mev-lending-spark` | Inactive (halted after exploits / no SparkLend on Arbitrum) |
| L4 | `mev-liquidate` | Protocol-agnostic runner: indexing, sweeps, sequencer-triggered at-risk bursts, backoff, planning |
| app | `mev-engine` | Runtime composing arbitrage + liquidation runner; one executor path for both |
| app | `mev-analytics` | Library + `mev-analyze` (event rates, per-pair edges) and `mev-scan` (search + on-chain sim of every route) |

Adding a venue = one new `mev-lending-<name>` crate implementing the trait + a config section +
allowlisting its targets (`scripts/allow-targets.sh`). No contract change: the executor runs any
owner-allowlisted protocol call with exact, reset approvals.
