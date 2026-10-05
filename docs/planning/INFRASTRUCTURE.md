# Infrastructure & Cost Plan

Researched 2026-10-03. Prices change often (Hetzner raised prices on 2026-06-15; Oracle cut its free
tier in 2026; OVH announced an H2 2026 price change) — **re-check before ordering.**

## Key finding: we can't afford our own L2 node yet, and that's OK

| Node | Disk needed | RAM | Notes |
|---|---|---|---|
| Arbitrum One (Nitro, pruned) | ~2.3–2.4 TB snapshot | 16 GB+ | Plus growth headroom |
| Base (reth, pruned, v2 snapshot) | ~1 TB pruned (3.2 TB archive snapshot) | 32 GB min, 64 GB rec. | Base recommends 8+ cores, local NVMe |
| Optimism (op-reth, pruned) | Smaller than Base | 16–32 GB | Cheapest OP Stack option |

A server that fits any of these costs **~$77–260/month** — more than our entire capital in 1–3
months. So until profitable we run **node-less** using the free data sources below.

## Where the sequencers are

The Arbitrum and Base sequencers are believed to run in **US East (AWS us-east / Virginia area)**.
Industry measurements show a Virginia node has roughly a 70 ms edge over Frankfurt for Arbitrum.
→ **Host the bot in US East (Ashburn / Virginia / New Jersey).** Europe-only providers like
Hetzner dedicated are a poor fit despite low prices.
Verify by measuring ping/WS latency to the sequencer endpoints from candidate VPSs in Phase 0.

## Stage 1 (now → profitable): ~$0–10/month

### Bot host (VPS)

| Option | Specs | Cost | Verdict |
|---|---|---|---|
| **Oracle Cloud Always Free, region `us-ashburn-1`** | Ampere ARM; officially 2 OCPU / 12 GB in 2026 (some still get 4/24), 200 GB block storage | **$0** | **First choice.** Pick Ashburn as home region at signup (can't change later). Capacity in Ashburn is often short — retry or use fallback |
| Hetzner Cloud, Ashburn (ASH) | Shared/dedicated vCPU, 4–8 GB | ~$5–15 (raised in 2026, check) | Fallback. Good network, US East |
| Vultr / DigitalOcean, NJ or NYC | 2 vCPU, 4 GB | ~$12–24 | Fallback #2 |

The bot itself is light (Rust, in-memory state); 2 cores / 4–12 GB is enough at this stage.
Build release binaries in CI or locally and ship them — don't compile on the free VM.

### Data sources (all free)

| Source | What we use it for | Limit |
|---|---|---|
| **Arbitrum sequencer feed** `wss://arb1.arbitrum.io/feed` | Real-time tx stream: oracle updates, swaps, liquidatable events | Free, unmetered — our biggest free edge |
| **Base Flashblocks stream** `wss://mainnet.flashblocks.base.org/ws` | Sequencer pre-confirmed blocks every ~200 ms | Free, official — Base equivalent of the Arbitrum feed |
| **dRPC free** | Primary reads / log subscriptions | ~210M CU per 30 days |
| **Alchemy free** | Secondary reads, `eth_call`, WS logs | 30M CU/month, 300 CU/s |
| **Chainstack free** | Tertiary / failover | 3M request units/month, 25 rps |
| Public endpoints (PublicNode, chain official RPCs) | Last-resort failover, tx broadcast | Rate-limited, best effort |
| **QuickNode** | Skip — no permanent free plan in 2026 (30-day trial only) | — |

`mev-rpc` spreads load across these, tracks each provider's remaining quota, and fails over
automatically. To stay inside free quotas:
- Use **WebSocket log subscriptions** for state changes; never poll in a loop.
- Build state once at startup (batched `eth_call` via Multicall3), then update it from events.
- Run `eth_call`/simulation locally in revm (`mev-sim`) against cached state instead of remote calls.
- Broadcast txs to several endpoints at once (sending txs is cheap on CU).

## Stage 2: paid RPC (~$50/month) — trigger: free quotas are the bottleneck AND 30-day profit ≥ $150

Upgrade the single most-used provider to its lowest paid tier (dRPC / Alchemy / Chainstack —
compare at the time; flat-rate plans are easiest to budget).

## Stage 3: own node near the sequencer — trigger: 30-day profit ≥ 3× node cost AND shadow logs show latency is losing us trades

Run one node for the most profitable chain only, in US East.

### What "lowest latency" actually means for us

Two different latencies matter, and vendors usually only advertise the first:
1. **RPC response time** (bot → node): fixed by running the node on the same machine or in the
   same data center as the bot. Any dedicated node in the right city gets this to ~1–5 ms.
2. **Distance to the sequencer** (node/bot → sequencer, both for seeing txs and sending them):
   fixed by location only — **US East / Ashburn**, ideally the same cloud region as the sequencer.
   No provider can sell more speed than its data center location allows.

So the fastest setup is **bot + node in the same US East data center**, receiving the free
sequencer feed / Flashblocks directly. Paying more beyond that buys capacity and uptime, not speed.

### Option A: pre-built managed dedicated node (no node ops — "RPC node already built")

| Provider | Chains | Price | Location choice | Notes |
|---|---|---|---|---|
| **Argusnodes** | Base (reth); others on request | Full node **from $199/mo**, archive from $799/mo | 18 cities incl. North America — request Ashburn | Cheapest pre-built dedicated found. Bare metal, unlimited RPS, live p50/p99 latency dashboard, monthly billing, no lock-in, crypto accepted |
| **Allnodes** | Arbitrum, Base, OP | Arbitrum full: **$320/mo** (Advanced), $640 (Enterprise) | Not stated — ask | Fixed price, unlimited requests |
| **Chainstack Dedicated** | Arbitrum, Base, OP + many | Compute from **$0.50/hr (~$365/mo) + storage**; needs Pro plan or higher | **You choose the cloud provider and region** — can pick AWS US East | Best for colocating with an AWS-hosted sequencer. Also offers Warp txs ($0.15/tx via bloXroute) for faster sending |
| GetBlock Dedicated | Arbitrum, Base, OP | $600–1,000+/mo | — | Pricier for the same thing |
| RpcFast dedicated cluster | Arbitrum | from $2,500/mo + servers | — | Enterprise; overkill |

### Option B: low-latency add-ons (no node)

| Service | Price | What it buys |
|---|---|---|
| Chainstack **Unlimited Node** (shared) | from $149/mo | Unlimited requests at fixed RPS — removes quota worries, not a latency upgrade |
| bloXroute **Base Flashblocks** stream | $250/mo ($300 parsed, $500 statediff) | Faster delivery of the same data as the free Flashblocks stream — only worth it if shadow logs show we're late |
| bloXroute **Base Speed Boost** | $1,000/mo | Faster tx routing to the Base sequencer. Late-stage only |
| bloXroute | — | **No Arbitrum products** — on Arbitrum the paid speed tools were Timeboost's express lane auction (being replaced by per-tx priority-fee auctions plus a paid fast feed; DECISIONS #52) |

### Option C: rent bare metal and run the node ourselves (cheapest per unit of speed)

| Option | Approx. price | Fits |
|---|---|---|
| OVHcloud **Rise** (US, pick East/Vint Hill) | from ~$77/mo, NVMe | Optimism / Base pruned (check RAM ≥ 32 GB & ≥ 2 TB NVMe) |
| OVHcloud **Advance-1** (US) | ~$204/mo + setup fee | Arbitrum / Base with headroom |
| Hetzner **AX42** (EU only) | €97.30/mo (new 2026 price) | Cheap, but EU → latency penalty. Only for non-latency-critical analytics/archive |
| Hetzner **AX102** (EU only) | €257.30/mo | Same caveat |

### Which options let us run our bot on the same machine as the node

| Type | Providers | Node included | Root/SSH (run our bot on it) |
|---|---|---|---|
| **Node + server in one** | **RedSwitches** dedicated Arbitrum / Base node — from **$199/mo**, pre-synced from snapshot, full root + KVM + IPMI. US East: Manassas VA (peers at Equinix Ashburn) or Edison NJ | ✅ | ✅ |
| Server only (we install node) | OVH Rise / Advance (US East), Hetzner AX (EU only), Oracle free VPS (too small for a node) | ❌ | ✅ |
| Node endpoint only | Argusnodes, Allnodes, Chainstack Dedicated, GetBlock, RpcFast | ✅ | ❌ — we'd need a separate VPS in the same city/region |

RedSwitches is the only option found that matches "pre-built node + server we build on directly".
Verify before ordering: exact quote for Manassas, real CPU model (single-core speed matters),
that NVMe size has growth headroom, and whether a 1-month term is available.

### Recommendation for Stage 3

| Chain | Pick | Monthly |
|---|---|---|
| Arbitrum or Base, bot on the same box | **RedSwitches dedicated node in Manassas VA** — node pre-synced, we get root and run `mev-bot` beside it | from $199 |
| Base, fully managed | Argusnodes full node in a US East city + a small VPS in the same city for the bot | from $199 + VPS |
| Arbitrum | **Chainstack Dedicated on AWS US East** (closest to the sequencer, managed). Self-run on OVH Advance-1 (~$204 + setup) if we want the cheapest option and can do node ops | ~$365+ / ~$204 |
| Either, cheapest | Self-run OVH Rise in US East (Option C) | from ~$77 |

Before paying: ask each vendor for a trial or a 1-month term, run `mev-bot` in shadow mode against it,
and compare our measured time-to-see-tx and time-to-inclusion with the free setup. Keep it only if
the extra captured profit exceeds its cost.

Notes:
- Hetzner dedicated servers are **not available in the US** (only cloud is), so they're out for
  latency-sensitive nodes.
- Prefer providers with **no or low setup fees** — OVH Advance charges a setup fee equal to one month.
- Use snapshots (Arbitrum, Base v2) to sync in hours instead of weeks.

## Monthly cost summary

| Stage | Setup | Monthly |
|---|---|---|
| 1 — research, shadow, early live | Free Oracle VPS (Ashburn) + free RPC tiers + Arbitrum feed | **$0** (fallback VPS ~$5–15) |
| 2 — proven profit | + one paid RPC tier | ~$50 |
| 3 — scaling | + one US East dedicated node (self-run $77–204, managed $199–365+) | ~$80–365 |

## Sources

- [Arbitrum: run a full node](https://docs.arbitrum.io/run-arbitrum-node/run-full-node) — snapshot sizes
- [Base: node performance tuning](https://docs.base.org/base-chain/node-operators/performance-tuning) — hardware recommendations
- [Base v2 snapshot size reduction](https://www.bitget.com/news/detail/12560605537603)
- [Hetzner price adjustment, June 2026](https://docs.hetzner.com/general/infrastructure-and-availability/price-adjustment/)
- [Oracle Cloud free tier changes 2026](https://terminalbytes.com/oracle-cloud-free-tier-changes-2026/)
- [OVHcloud US dedicated server prices](https://us.ovhcloud.com/bare-metal/prices/)
- [Chainstack: best free-tier RPCs 2026](https://chainstack.com/best-free-tier-rpcs-web3-builders-2026/)
- [RedSwitches dedicated Arbitrum node](https://www.redswitches.com/dedicated-arbitrum-rpc-node-servers/) · [Base node](https://www.redswitches.com/dedicated-base-rpc-node-servers/) · [US data centers](https://www.redswitches.com/dedicated-servers/usa/)
- [Argusnodes Base dedicated node](https://argusnodes.com/chains/base)
- [Allnodes Arbitrum hosting](https://www.allnodes.com/arb/host)
- [Chainstack dedicated nodes](https://docs.chainstack.com/docs/dedicated-node) · [Chainstack trader nodes](https://chainstack.com/trader-nodes/)
- [bloXroute pricing](https://bloxroute.com/pricing/)
- [Dwellir: Arbitrum RPC providers 2026](https://www.dwellir.com/blog/best-arbitrum-rpc-providers)
- [RpcFast dedicated Arbitrum](https://rpcfast.com/dedicated-arbitrum-nodes)
- [Base Flashblocks app integration](https://docs.base.org/base-chain/flashblocks/app-integration)
- [L2 node location & latency](https://chainscorelabs.com/blog/layer-2-wars-arbitrum-optimism-base-and-beyond/l2-node-infrastructure-and-operation/why-your-l2-nodes-geographic-location-is-a-competitive-advantage)

## Endpoint capability matrix (measured 2026-10-03/04)

Keys live only in `.env` (`ARBITRUM_HTTP_URL`, `_2`.. `_9`, same for `WS`). Roles follow capabilities:

| Endpoint | eth_call | Wide getLogs | Archive | WS | Role |
|---|---|---|---|---|---|
| Alchemy (free, key 1 & 2) | ✅ | ❌ 10 blocks | ✅ | ✅ | Paid reads (rotated), broadcast, WS + failover, E2E fork source |
| Chainstack (free) | ✅ | ❌ ~100 blocks (10k → 403) | ❌ (-32002) | ✅ | Paid reads (rotated), broadcast, WS failover. Not in sweeps: ~3M RU/month quota vs ~4.7M/month sweep load |
| arb1.arbitrum.io (public) | ✅ | ✅ complete 2M-block ranges | ✅ | — | History scans (rate-limits this IP; backoff), sweep rotation, fork tests |
| PublicNode (public) | ✅ | ⚠️ silently truncates | ❌ | ✅ | Sweep rotation (rate-limits: -32005) |
| dRPC (public) | ✅ | ❌ 10k range, truncates | — | — | Sweep rotation |
| BlastAPI (public) | ✅ | ❌ 10 blocks / truncates | — | — | Sweep rotation |
| MeowRPC (public) | ❌ "eth_call not supported" | — | — | — | Unused |
| Arbitrum sequencer feed | — | — | — | ✅ feed | Real-time txs (~0.9 s ahead), free, unmetered |
