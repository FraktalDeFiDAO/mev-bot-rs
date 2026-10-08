# One crate per protocol

Owner requirement (2026-10-08): isolate each protocol in its own crate. This is the target structure and the staged
path to it. The bot trades live, so every stage is behaviour-preserving, covered by tests that compare old and new
results, and deployed on its own.

## Where protocol knowledge lives

| Concern | Where | Per-protocol? |
|---|---|---|
| Lending liquidation adapters (runtime: RPC, indexing, planning) | `mev-lending-<name>` (9 crates) behind the `mev-lending` trait | yes |
| Decoding: events, calldata, identified contracts | `mev-protocol-<name>` (10 crates) behind the `Protocol` trait, collected by `mev-decode` | **yes (stage 1, done)** |
| Contract bindings | `mev-bindings` (one file, every protocol) | no |
| Pool math (price, swap output, tick walk) | `mev-dex` (`Pool`, matches on `PoolKind`) | no |
| Applying logs to pool state | `mev-state` (`apply_log`, matches on `PoolKind`) | no |
| Factory lookups, pool and tick loading | `mev-discovery` (`lib.rs`, `ticks.rs`, `v4.rs`) | no |
| Venue identity | `mev-types::PoolKind`, matched in 7 crates | no |

## Target

```
mev-protocol            core: Protocol trait, Class, EventKind, SwapIntent, LiquidationIntent, PoolRef, PriceUpdate, ABI helpers
mev-protocol-<name>     one per protocol, pure (no RPC, no engine, no state):
                          events (topic0, class, price), calldata decoders, identified contracts,
                          later: pool math, log -> state transition, factory/pool discovery calls
mev-decode              registry over the protocol crates + the `mev-decode` CLI; keeps its public API
mev-lending-<name>      runtime adapters (RPC, indexing, planning); depend on the matching mev-protocol-<name>
```

A protocol crate knows nothing about the others. Adding a protocol means one new crate plus one line in the
`mev-decode` registry (and, from stage 3, one line in the venue registry). Nothing outside the registries matches on a
protocol name.

`mev-protocol-*` is the pure, dependency-light layer other programs can reuse on its own; `mev-lending-*` stays the heavy
runtime layer and will depend on it.

### The trait

```rust
pub trait Protocol: Sync {
    fn id(&self) -> &'static str;                         // "uniswap_v3"
    fn events(&self) -> Vec<EventKind>;                   // topic0, name, class
    fn price_update(&self, kind: &EventKind, data: &[u8]) -> Option<PriceUpdate> { None }
    fn decode_call(&self, to: Address, input: &[u8]) -> Option<Decoded> { None }
    fn contracts(&self) -> Vec<Contract> { vec![] }       // identified destinations, decoded or not
}
```

## Stages

| Stage | Scope | Risk to trading | State |
|---|---|---|---|
| 1 | Decoding: `mev-protocol` core, ten `mev-protocol-*` crates, `mev-decode` as a registry, new router and liquidation-call decoders | none (not on the trading path; the old API is kept) | **done** (DECISIONS #94) |
| 2 | Event bindings: `mev-bindings` split per protocol; protocol crates own their ABI; `mev-bindings` re-exports | low (types move, not behaviour) | planned |
| 3 | Pool math: `Pool::amount_out`, `state_after`, tick walk move into per-venue crates behind a `Venue` trait; `mev-dex` dispatches. Old and new compared on randomised pools (property tests) before the switch | **medium: pricing is on the hot path** | planned |
| 4 | Log application: `mev-state::apply_log` and V3 Mint/Burn/V4 liquidity move into the venue crates; the tentative/rollback machinery stays in `mev-state` | **medium** | planned |
| 5 | Discovery: factory lookups and pool/tick loading per venue; `mev-discovery` becomes a loop over the registry | low-medium | planned |
| 6 | `PoolKind` stops being matched outside the venue registry (`mev-search`, `mev-engine`, `mev-analytics`) | medium | planned |

Rules for stages 3-6: no stage changes numeric results (a differential test runs both implementations over the same
recorded pools and logs and requires identical output); no stage is deployed together with another; each is shadow-checked
against the live node comparison counters before and after.

## Protocol crates (stage 1)

| Crate | Covers |
|---|---|
| `mev-protocol-uniswap-v2` | V2 swap, sync, mint, burn events (also Sushi V2 and Camelot V2 pairs) |
| `mev-protocol-uniswap-v3` | V3 swap, mint, burn, position-manager liquidity events (Sushi V3 and Algebra share the topics) |
| `mev-protocol-uniswap-v4` | V4 swap and modify-liquidity events |
| `mev-protocol-uniswap-router` | calldata of SwapRouter02, SwapRouter v1 and the Universal Router: exact-in swaps (single and multi-hop, first hop), V2-style swaps, `multicall`, Universal Router V2/V3 exact-in commands |
| `mev-protocol-pancake-v3` | Pancake swap event (different layout) |
| `mev-protocol-balancer` | Vault swap event |
| `mev-protocol-aave-v3` | `LiquidationCall` event; `liquidationCall` calldata |
| `mev-protocol-morpho-blue` | `Liquidate` event; `liquidate` calldata (with market id) |
| `mev-protocol-compound-v3` | `AbsorbCollateral`/`AbsorbDebt` events; `absorb` calldata on the four configured comets |
| `mev-protocol-unidentified` | the two unidentified swap contracts A and B, kept apart until they are identified |

Counts: 1 core + 10 protocol crates. Camelot V3 (Algebra) emits the same `Swap` shape as Uniswap V3 and shares its topic;
it gets its own crate when its math and discovery move in stages 3-5.

## Not decoded yet (stage 1 follow-ups)

Exact-output swaps, V4 swaps through the Universal Router, 1inch / Odos / ParaSwap / KyberSwap / 0x aggregator routes,
the Camelot and Sushi routers, ERC-4337 `handleOps` unwrapping, position-manager `mint` / `increaseLiquidity` /
`decreaseLiquidity` calldata, V2 `addLiquidity` / `removeLiquidity`, Fluid and the inactive lending protocols. The ranked
list with measured volume is `docs/design/watched-protocols.md` and the coverage report.
