# Arbitrum bot contracts and opportunities — 2026-10-06

The strongest new findings are **Atlas/SVR auction participation**, **Morpho flash-loan funding**, and **routing beyond our current two- and three-pool arbitrage search**. These are engineering opportunities to measure, not established profitable strategies.

Research used Arbiscan transaction pages, read-only Arbitrum One RPC, Sourcify source matches, and first-party protocol documentation. No contracts were deployed and no transactions were submitted. Selected RPC evidence is saved in [arbitrum-bot-evidence-2026-10-06.json](arbitrum-bot-evidence-2026-10-06.json). Additional historical arbitrage candidates are in [the discovery notes](arbitrum-bot-candidates-2026-10-06.md).

## Contracts worth studying

Addresses are on Arbitrum One, chain ID 42161. “Source not recovered” means the explorer code page was inaccessible and Sourcify returned no match; it does **not** establish that the contract is unverified on Arbiscan. Contract addresses also do not necessarily represent independent operators.

| Contract | Evidence and timing | What to learn | Source access |
| --- | --- | --- | --- |
| [0x5A35e7396664B2024B29433eb189F1F72B41Fdba](https://arbiscan.io/address/0x5A35e7396664B2024B29433eb189F1F72B41Fdba#code) | Successful cyclic swaps in August/September 2026; current bytecode checked October 6 | Five-pool paths, compact calldata, direct transfers between pools, Uniswap v4 interaction | Sourcify exact match for ERC1967 proxy; current implementation `0xB874a74ECD2DDb001F15A7C2aA56646B1D2AC1DA` source not recovered |
| [0x3a43a868ee1829ad89f79c8ecf67cc7476469f55](https://arbiscan.io/address/0x3a43a868ee1829ad89f79c8ecf67cc7476469f55#code) | Actual solver and Aave event liquidator in an October 3, 2026 Atlas transaction | Auction bid, oracle update plus liquidation inside one transaction, solver attribution | Solver source not recovered; enclosing Atlas and DAppControl source matches available |
| [0xff6c17ce9b1b19c9028980ce49e5ef818e80d151](https://arbiscan.io/address/0xff6c17ce9b1b19c9028980ce49e5ef818e80d151#code) | Successful Aave liquidation October 4, 2026 | Morpho-funded WETH/WETH liquidation without a collateral swap | Source not recovered |
| [0xe2c2bce4714bcd6ad6cce0af5ae2150b3c4a231e](https://arbiscan.io/address/0xe2c2bce4714bcd6ad6cce0af5ae2150b3c4a231e#code) | Five successful transactions containing Aave liquidations October 4, 2026 | Large combined operations, mixed funding and swaps; account for the entire transaction | Source not recovered |
| [0x0dc76a8cdc8194e2ed4810e729cea63d4b864104](https://arbiscan.io/address/0x0dc76a8cdc8194e2ed4810e729cea63d4b864104#code) | Aave event liquidator October 4, 2026; outer transaction goes to `0xb03d0c72c76055cad15297f779ee77f90f9d9b9e` | Distinguish the actual liquidation caller from its outer execution contract | Source not recovered; current deployed code confirmed |
| [0xf238d4353246948ecdc3f3252ae939fe5234e145](https://arbiscan.io/address/0xf238d4353246948ecdc3f3252ae939fe5234e145#code) | Successful arbitrage November 27, 2024 | Balancer-funded WETH → UNI → WETH path through GMX Vault; historical venue reference | Source not recovered; historical strategy does not establish current opportunity |
| [0x00000000c868e634590a981dd3bd007dca5c2e43](https://arbiscan.io/address/0x00000000c868e634590a981dd3bd007dca5c2e43#code) | Successful v4/v3 arbitrage March 4, 2025 | ANT/USDC route, 76-byte calldata, explicit USDC payout; historical v4 reference | Source not recovered; historical reference |
| [0x0000b0ca00000017d543c800be4257629544fb51](https://arbiscan.io/address/0x0000b0ca00000017d543c800be4257629544fb51#code) | Successful two-pool arbitrage May 3, 2024 | Simple WETH/USDT route and explicit profit transfer; baseline for gas comparison | Source not recovered; historical reference |

Historical rows and their transaction evidence were independently researched in [the discovery notes](arbitrum-bot-candidates-2026-10-06.md). Sources for the recent rows are detailed below.

## 1. The “0x8ad1” competitor is Atlas, not a single bot

Our existing [decision log, entry 40](../planning/DECISIONS.md) attributes fast Aave liquidations to `0x8ad1aE9D…`. First-party deployment documentation identifies the full address, `0x8ad1aE9D97C79aA68A0a151E83ff3942f68F86C1`, as **Atlas v1.6.4**. The control contract is `0xe15BBa987C002ecc3586e81244517877D294d291`. [Chainlink contract references](https://docs.chain.link/data-feeds/svr-feeds/searcher-onboarding-atlas), [FastLane deployments](https://docs.atlasevm.com/atlas/contracts/deployments/arbitrum-one/).

Decoded [transaction `0xcb9879…61ac1`](https://arbiscan.io/tx/0xcb987939b824e0e56ab7480919deede4885e65c884b55e494d1ffe7890361ac1) using Atlas's matched ABI and the official RPC:

| Field | Observed value |
| --- | --- |
| Block / transaction position | 511414322 / 1 |
| Date | October 3, 2026 UTC |
| Outer recipient | Atlas `0x8ad1ae9d97c79aa68a0a151e83ff3942f68f86c1` |
| Number of solver operations | 1 |
| Solver contract | `0x3a43a868ee1829ad89f79c8ecf67cc7476469f55` |
| Solver operation signer | `0x271381c423d07bfab0479af2be110262e8469d58` |
| DApp control | `0xe15bba987c002ecc3586e81244517877d294d291` |
| Bid token / amount | Native ETH / 71,193,699,776,909 wei |
| Aave event liquidator | Same solver contract |
| Debt repaid / collateral seized | 3.157471 USDT0 / 0.00003965 WBTC |
| Receipt status | Success |

Atlas source has a Sourcify match named `Atlas`, and the control source has a match named `ChainlinkOevDAppControl`. [Atlas matched source and ABI](https://sourcify.dev/server/v2/contract/42161/0x8ad1ae9d97c79aa68a0a151e83ff3942f68f86c1?fields=compilation,abi), [control matched source](https://sourcify.dev/server/v2/contract/42161/0xe15bba987c002ecc3586e81244517877d294d291?fields=compilation,sources).

Chainlink's SVR flow bundles an oracle update and solver execution atomically. Searchers receive auctions over a dedicated WebSocket and submit signed solutions; the oracle network submits the transaction. Participation requires an Atlas-compatible solver and bonded funds. The documentation recommends 0.1 ETH bonding on Arbitrum and describes permissionless reputation tiers. [SVR onboarding](https://docs.chain.link/data-feeds/svr-feeds/searcher-onboarding-atlas).

**Implication:** a transaction already containing the oracle update and liquidation cannot be beaten by sending a separate liquidation after seeing that update. Some losses attributed to an unusually fast bot are auction-flow losses. This sample does not prove that every historical `0x8ad1` transaction had the same solver or mechanism; decode those individually.

**Repo gap:** the oracle module recognizes some SVR wrapper shapes, but no auction client, SolverOperation submission path, or Atlas solver entry point was found in `crates/`, `config/`, or our executor. `ArbExecutor` starts trades only for its immutable owner. An integration needs a deliberately authenticated solver interface/adapter, not an unrestricted new caller.

**Next measurement:** classify historical Aave liquidation receipts into Atlas/SVR and direct transactions, decoding solver identities. Measure debt, residual after bids and gas, and accessible auction volume before committing capital to an integration.

## 2. Morpho is an observed alternative to our flash-loan funding

In [transaction `0x39749a…ceeb1`](https://arbiscan.io/tx/0x39749a6d4451983e8b9e39134e0c626dc0e85e0b8cd1e473d43a18941ddceeb1), `0xff6c…d151` receives **0.000896724977931720 WETH** from Morpho `0x6c247b1F6182318877311737BaC0844bAa518F5e`, repays an Aave WETH debt, receives WETH collateral, and returns the same borrowed quantity to Morpho. The Aave event reports 0.000896635351973013 WETH debt and 0.000936983942811797 WETH collateral. The receipt used 427,426 gas. These figures are observed transfers, not a whole-strategy profit calculation. The sender is also the solver-operation signer in the Atlas sample; these two executors are linked by their use of that signer.

Morpho documents `flashLoan` and the authenticated `onMorphoFlashLoan` callback, followed by repayment through `transferFrom`. [Morpho flash-loan flow](https://docs.morpho.org/learn/concepts/flashloans/).

Read-only token balances sampled on October 6:

| Token | Morpho balance | Balancer Vault balance |
| --- | ---: | ---: |
| WETH | 179.4611 | 94.7254 |
| USDC | 1,011,296.583557 | 10,945.642419 |
| USDT0 | 176,410.997174 | 50,815.116763 |
| USDC.e | 0 | 168,989.140812 |

These are changing contract balances, not reserved liquidity or guaranteed borrow sizes. Exact raw readings are saved in the evidence JSON. The token-specific difference matters: Morpho was much deeper for native USDC, while Balancer held USDC.e.

**Repo gap:** our executor currently has Balancer funding, V3 flash swaps, and Aave funding, with no `onMorphoFlashLoan` entry point. A Morpho funding option is worth comparing for USDC liquidations constrained by Balancer capacity. First fork-replay representative liquidations and measure successful borrow size, full transaction gas, and repayment behavior. Check the deployed protocol's fee rules before relying on cost savings.

## 3. Longer routes and calldata efficiency

[August 3 transaction `0x96c754…43990`](https://arbiscan.io/tx/0x96c754157b2777fd32dc3c73c23bd9ab843482b92418da68e7fec513d7a43990) sent to `0x5A35…Fdba` traverses this five-pool cycle:

`WETH → USDC → USDC.e → WBTC → tBTC → WETH`

Transfers connect the pools directly, including PancakeSwap v3's WBTC/tBTC pool. Raw calldata is **312 bytes**, confirmed through RPC. A five-hop call to our existing `executeFlashSwap` ABI would be **1,092 bytes**: selector + three argument heads + array length + five six-word hop tuples. This is a structural size comparison, not a measured gas saving; Arbitrum compresses transaction data and the two implementations differ.

Our [two-pool search](../../crates/mev-search/src/lib.rs) and [triangular cycle index](../../crates/mev-search/src/cycles.rs) cannot enumerate that five-pool cycle. The executor already loops over arbitrary hop arrays, so search coverage is one separate limitation. The configured pool kinds lack Uniswap v4; another [successful sample on August 28](https://arbiscan.io/tx/0xe75d5444da786a2ea45e1e741396f71a9489ac38db5025e9c972695a21814671) includes the v4 PoolManager.

Another [historical v4/v3 sample](https://arbiscan.io/tx/0x17fc8afdfff290ad67604cdc6a9a2acf8ab24a9351b7e18aa8a8148f0992303f) from `0x00000000c868e634590a981dd3bd007dca5c2e43` combines ANT/USDC pools using 76 bytes of calldata. Its receipt used 224,632 gas. This is a useful comparison case, not proof that its encoding causes low gas or that ANT opportunities remain available.

The `0x5A35` proxy is an exact source match in Sourcify. Its current ERC1967 storage slot resolves to `0xB874a74ECD2DDb001F15A7C2aA56646B1D2AC1DA`. That implementation source was not recovered, so packed field meanings, callback checks, and supported route types remain unresolved. [Matched proxy](https://sourcify.dev/server/v2/contract/42161/0x5A35e7396664B2024B29433eb189F1F72B41Fdba?fields=compilation,proxyResolution).

**Next measurement:** build a dataset of successful four- and five-pool routes and v4-containing routes, replay them at historical state, then rank incremental net opportunity versus additional search/simulation work. For calldata changes, benchmark our present ABI against a compact encoding on identical routes. Benchmark direct pool-to-pool settlement separately. Keep existing callback authentication and minimum-profit protection in either design.

## 4. Successful execution can leave almost no return

Two `0x5A35` samples show why transaction count is a poor success metric:

| Transaction | Observed WETH/ETH residual or payout | Transaction fee, ETH | Residual minus transaction fee, ETH |
| --- | ---: | ---: | ---: |
| [August 3](https://arbiscan.io/tx/0x96c754157b2777fd32dc3c73c23bd9ab843482b92418da68e7fec513d7a43990) | 0.000013491712415497 WETH retained | 0.000013360848488 | 0.000000130863927497 |
| [September 4](https://arbiscan.io/tx/0x387d3813e890dbb097726e449d244e7f9c936e413747a639fe3168d4c5f79053) | 0.000060353741455878 ETH paid to sender | 0.000052328034292 | 0.000008025707163878 |

The arithmetic excludes infrastructure costs, failed attempts, other fees outside the transaction, and inventory valuation. The August trade's margin is particularly small. Both samples precede the repo's observed September ordering-policy change, so their zero priority fees do not establish a useful bidding policy today. Arbitrum's current PGA documentation describes ordering by effective priority fee with arrival-time tie breaks. [PGA mechanics](https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/pga).

For `0xe2c2…231e`, one recent transaction used 8,378,660 gas and emitted 539 logs. Its one Aave liquidation cannot explain the complete operation's return; it includes many other transfers and calls. Treat whole-transaction accounting as mandatory before inferring an advantage from such bundles. [Sample receipt](https://arbiscan.io/tx/0x5b945fe1548ef515e758e7868579c85fcee713bb1ff08f8ffc455120f7a014f8).

## Research boundaries and follow-up order

The Aave scan covers blocks **511174214–512211014**, corresponding to **2026-10-03 02:40:47 through 2026-10-06 09:57:04 UTC**. It found eight LiquidationCall events in eight successful transactions. This is approximately 79 hours, not an exact 72-hour window: block counts were used to choose the start, then timestamps were checked. Only Aave v3 was scanned for this recent dataset; it is not a census of every Arbitrum bot.

Arbiscan blocked direct code pages. Source matches were recovered for Atlas, its control contract, and the `0x5A35` proxy; most executor logic remains unavailable. We also found exact matched `FlashArbitrage` source at `0xA53a6eBF1aD2DF94645e3b9544a66A78fc7055e5`, but it is a prefunded two-router template with a sampled starting-balance revert. It was excluded from the competitive shortlist. A verified name alone is weak evidence.

Recommended order:

1. Correct competitor attribution and measure SVR versus direct liquidation volume and returns.
2. Replay Morpho funding against our existing sources for representative liquidation sizes.
3. Measure four/five-hop and v4 route opportunity before expanding adapters and search.
4. Benchmark compact calldata and pool-to-pool settlement on identical profitable routes.

This research identifies concrete hypotheses and public examples. It does not establish a profitable edge, reconstruct competitors' off-chain search algorithms, or justify copying unavailable contract logic.
