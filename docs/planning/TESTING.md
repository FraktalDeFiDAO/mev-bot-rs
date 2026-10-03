# Testing

Every layer is tested; each tier catches what the one below can't. Run the gate before every commit.

| Tier | What | Where | Run | Network |
|---|---|---|---|---|
| Unit | Every crate's logic: pool math, state/log application, search, risk, config validation, metrics, calldata encoding (ABI round-trips), lending math per protocol, runner scheduling (scripted adapters), sequencer decoding, oracle matching, index cache/migration | `crates/*/src` `#[cfg(test)]` | `cargo test --workspace` | none |
| Property | Invariants over random inputs (proptest, 256 cases each): DEX output monotonic/bounded/fee-reducing; trade sizing beats a brute-force grid; identical pools never arb; loan caps respected; Aave/Morpho/Comet liquidation sizing never overpays/overseizes, bonus ≥ 0; risk guard never exceeds the gas cap; sequencer decoder never panics | `mod props` in dex, search, risk, sequencer, lending-* | `cargo test --workspace` | none |
| Contract unit + fuzz + invariant | Mocked vault/token/protocol: owner-only for every entry point, callbacks reject any caller/args, bad routes revert, allowlist (targets and spenders), liquidation profit accounting; invariant over 128k random calls: executor never holds tokens or ETH | `contracts/test/ArbExecutorUnit.t.sol` | `forge test --match-path test/ArbExecutorUnit.t.sol` | none |
| Contract fork | Real protocols on live Arbitrum state: arbitrage via Uni/Sushi V2, Camelot V2, Uni V3, PancakeSwap V3, Algebra; Aave liquidation; Comet absorb + buyCollateral; Morpho liquidation; Fluid liquidation with native-ETH collateral; security cases (unauthorized callbacks, token-drain attempt, ETH outside liquidation) | `contracts/test/ArbExecutor.t.sol` | `cd contracts && forge test` | Arbitrum RPC |
| End-to-end | The real `mev-bot` binary in **live** mode against an anvil fork: deploy executor, start bot, create a price gap, assert detect → simulate → send → land → profit at the receiver. Also exercises stale-nonce recovery | `scripts/test-e2e.sh` | `scripts/test-e2e.sh` | archive-capable RPC for the fork |
| Live shadow | Full stack against mainnet without sending: discovery, adapters, sweeps, sequencer triggers, simulations | `scripts/run.sh shadow` | — | live |

## Gate

```bash
scripts/check.sh          # fmt + clippy -D warnings + all Rust tests (unit + property)
scripts/check.sh --all    # + Foundry suites (fork tests need an RPC: ARBITRUM_FORK_URL)
scripts/test-e2e.sh       # E2E (uses ARBITRUM_HTTP_URL from .env for the fork, else arb1)
```

## E2E safety

The anvil fork keeps Arbitrum's state but runs with **chain id 31337**, and the harness strips the
sequencer broadcast endpoint and feed from the config — a transaction signed on the fork can never
be valid on mainnet or reach a mainnet node. It uses anvil's public dev key only.

## Bugs these tests caught

- Stale nonce after the wallet is used outside the bot → every trade rejected (E2E). Fixed: resync + retry.
- Executor never got prank-owner in tests when `new` was inside constructor args (Foundry prank ordering).
- Wrong 1e36 oracle-price fixture in Morpho math (unit test).
- Chainlink reports arrive via per-operator forwarders, so matching `tx.to` never fired (live run + unit test).
