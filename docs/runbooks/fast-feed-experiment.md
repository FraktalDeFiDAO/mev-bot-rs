# Runbook: Experiment 0, does the paid Fast Feed lead the free feed?

**Question:** how much earlier does the Fast Feed deliver each transaction than the standard sequencer feed we use today?
**Why:** 0 wins in 155 races; the winners reach the feed in the same message as us, in the block after the trigger, and 10 of the 24 races
with tip data were lost on timing despite a higher tip. The standard feed is per block; the Fast Feed publishes each transaction as soon as
the Sequencer has ordered and executed it. If it leads by about a round (125 ms) or more, the sequencer-first rebuild is justified; if not,
we save weeks. **Cost:** one ticket, at least 17 USDG for a 24 h round.

## Documentation (read these first)

| What | Link |
|---|---|
| What the Fast Feed publishes, message fields, pricing, who it is for | https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/fast-feed |
| **How to get access** (API key, USDG, `Tickets` contract, endpoint, rules) | https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/use-fast-feed |
| How PGA orders transactions (rounds, priority fee, boost) | https://docs.arbitrum.io/how-arbitrum-works/priority-gas-auction/pga |
| The free standard feed (format, `22207817` block offset) | https://docs.arbitrum.io/run-a-node/sequencer/read-sequencer-feed |
| Doc index (append `.md` to any docs URL for markdown) | https://docs.arbitrum.io/llms.txt |
| `Tickets` contract (proxy) on Arbiscan | https://arbiscan.io/address/0x451DD62C26b753d07d6104aaC6865AaE73a6A598 |
| USDG token on Arbiscan | https://arbiscan.io/address/0x004B506865409877C9fA29bfb1ebA929984B9bbC |

Everything we rely on is recorded with its source in `docs/arbitrum-facts.md` (F1-F8). Verified live on 2026-10-07: the contract exists, `token()`
is the USDG address above (6 decimals), round 14, price 17 USDG. **Not in the docs:** where to acquire USDG (swap into it on an Arbitrum One DEX; check
liquidity yourself) and refunds.

## When you can buy (verified on chain 2026-10-07/08)

- Rounds are 24 h (`roundDuration()` = 86400; **DAO-settable**, so scripts read it each time). Round 14 started 2026-10-07 17:00 UTC.
- **Grandfather period:** for the first half of every round (`grandfatherPeriodFraction()` = 128 of 256) only the previous round's subscribers may
  buy, up to the number they held. A **first-time buyer can buy only in the second half**: today **05:00-17:00 UTC (2026-10-08)**.
- A ticket bought in round N is active for round N+1 only. Bought today, it is live **2026-10-08 17:00 UTC to 2026-10-09 17:00 UTC**.
- To keep the subscription you must renew each round, and renewing in the *first* half of the next round is guaranteed (grandfather right);
  waiting for the second half risks the round selling out (target 100, max 200 tickets).
- Tickets are tracked **per buyer address** (`thisRoundTicketCount`, `grandfatherCount`). The price floats between roughly half and double of the
  previous round's, set by demand (EIP-4844 style, `priceUpdateFraction` 144), never below the 17 USDG floor.

## What you need

- A **separate wallet** (not the bot's hot wallet) with **≥ 40-50 USDG** (price 17, may double) and a little ETH on Arbitrum One for gas.
- Foundry (`cast`, `forge`) and `openssl`.
- **A keystore account, not a raw key.** `cast wallet import ff-owner --interactive` (paste the key once; it is stored encrypted in `~/.foundry/keystores`).
  The scripts take only `FF_*_ACCOUNT=<name>`. Raw keys are refused on purpose: Foundry 1.8 has no environment variable for them, so every use would
  put the key on a command line, visible to anything that can list processes. (An earlier draft of these scripts claimed otherwise; that was wrong and is fixed.)

## Path A: one ticket for the experiment (simplest)

```bash
scripts/fast-feed-ticket.sh status                       # round, price, and WHEN A FIRST-TIME BUYER CAN BUY
scripts/fast-feed-ticket.sh keygen                       # API key -> ~/.config/mev-bot/fast-feed.key (0600); prints its hash
export FF_BUYER_ACCOUNT=ff-owner
scripts/fast-feed-ticket.sh status <wallet-address>      # USDG + ETH balances
scripts/fast-feed-ticket.sh buy --tickets 1 --deposit-usdg 40          # DRY RUN: refuses with a clear message before the window opens
scripts/fast-feed-ticket.sh buy --tickets 1 --deposit-usdg 40 --yes    # typed confirmation, then approve + deposit + purchase
```

## Path B: a vault that renews by itself and accepts deposits in ETH / USDC (v2)

`contracts/src/FastFeedTicketVault.sol` (+ `UsdgConverter.sol`) is the subscriber. It **holds the money itself** (the Tickets contract is a DAO-governed
upgradeable proxy that has been upgraded once; the vault deposits exactly `price x tickets` into it just before buying and takes back any leftover).

- **Renewal:** `renew()` is callable by **anyone**, but can only buy the owner-configured number of tickets for the owner-configured key hash, at no
  more than the price cap, once per round, within `renewals`, `budget` and `expiry`. It buys what the vault can afford (a partial seat beats losing it) and,
  inside the grandfather window, only what the allowance permits (the Tickets contract reverts the whole purchase if you ask for more).
- **Deposits:** send **USDG, USDC or ETH** to the vault address like to any wallet. A keeper (anyone) calls `convert(token, amount)`; ETH goes ETH -> WETH ->
  USDC (Uniswap V3) -> USDG (the hooked Uniswap V4 pool, the only deep USDG liquidity). The contract computes the minimum USDG itself (Chainlink ETH/USD,
  fresh and sequencer-up; USDC valued 1:1) and reverts unless it gets at least that: a keeper can only sandwich inside the bound (10 bps USDC, 100 bps ETH),
  never steal. Any other token needs a route from the owner (`route`).
- **Only the owner** can fund, configure, whitelist routes, withdraw, rotate the key, `exit()` or sweep. The keeper account holds gas only.

```bash
# one-time: keystore accounts (the key is typed once, stored encrypted; raw keys are refused)
cast wallet import ff-owner --interactive
cast wallet import ff-keeper --interactive           # a SEPARATE account with ~0.002 ETH of gas and nothing else
export FF_OWNER_ACCOUNT=ff-owner

scripts/fast-feed-vault.sh deploy --yes               # you deploy it (dry run without --yes); note the address
scripts/fast-feed-vault.sh setup <vault> --tickets 1 --max-price 40 --renewals 7 --budget 150 --expiry-days 30 --deposit 50   # dry run; add --yes
scripts/fast-feed-vault.sh route <vault> both --yes   # whitelist USDC and ETH deposits for conversion (dry run first)

export FF_KEEPER_ACCOUNT=ff-keeper FF_PASSWORD_FILE=~/.config/mev-bot/keeper.pw    # file mode 600 with the keystore password
scripts/fast-feed-vault.sh keeper <vault> --loop 300  # converts deposits when USDG is short, simulates, then renews (use systemd/cron in production)

scripts/fast-feed-vault.sh status <vault>             # anyone: balances, renewals/budget left, ready to renew
scripts/fast-feed-vault.sh rotate-key <vault> <new-key-file> --yes   # planned key change, used from the next renewal
scripts/fast-feed-vault.sh withdraw <vault> 20 --yes  # take USDG back
scripts/fast-feed-vault.sh exit <vault> --yes         # stop automation and return everything in USDG; other tokens: sweep / sweepEth
```
**Monitoring:** set `FF_VAULT` (and `FF_KEEPER_ADDRESS`) for `scripts/watchdog.sh`: it alerts when the next round has no ticket and < 2 h remain,
when USDG cannot cover the next ticket, when renewals or budget run low, and when the keeper's gas is low. `scripts/report.sh` (with `FF_VAULT`) prints
ticket spend and conversion drag.

**Limits you should know.** The worst-case spend is `min(budget, renewals x tickets x max price)`. A leaked API key cannot be revoked mid-round (rotate and wait
out the round). USDG has no price feed and is assumed = $1 (a depeg below $1 makes conversions revert; above $1 they return more). The USDG pool's hook is a
135-byte proxy we cannot read: the bound protects every conversion, and `renew()` never depends on a conversion succeeding. **The contracts are unaudited and
undeployed; 39 fork tests (21 ticket logic, 18 converter) plus fuzz and an end-to-end fork run pass. Start with a canary:** deploy, send 20 USDG, one renewal,
confirm the `TicketsPurchased` event names the vault as buyer, then `exit`.

## Measure (after the ticket is live)

From the host you care about (the lead is a difference between two feeds seen from the same place; the Ohio host is the most representative):

```bash
export ARBITRUM_FAST_FEED_KEY=$(cat ~/.config/mev-bot/fast-feed.key)
cargo run --release -p mev-analytics --bin mev-feedlead -- config/arbitrum.toml 3600 data/feedlead
```

It prints a progress line every 30 s and a verdict at the end. Output in `data/feedlead/`:
`lead.jsonl` (every paired transaction), `summary.json`, and `fast_feed_samples.jsonl` (the first 300 raw Fast Feed messages: the fixtures for the
Phase 2 decoder; contains no secrets).

## Decision rule

- **Premise holds:** the Fast Feed's median lead is **≥ 100 ms** and it delivered first for **> 50 %** of transactions. Then Phase 2+ (Fast Feed receipt logs driving state) is worth building, and a continuous ticket (≈ $510/month per connection) becomes a cost to weigh against shadow results.
- **Premise fails:** otherwise the Fast Feed alone does not explain the timing losses; do not buy tickets continuously; revisit the diagnosis.
- Also read `fast_only` / `standard_only` in `summary.json` (a Fast Feed that misses transactions is a correctness problem, see the gap counters).

## Safety

- The API key is a bearer secret: keep the file at mode 600, never commit it, never paste it into chats or logs (the bot redacts URL keys, not this).
- Everything that moves money is a dry run unless `--yes` and still asks for a typed word. Signing uses keystore accounts only (no raw keys anywhere).
- The vault's keeper account holds gas only; if its key leaks, the worst outcome is someone calling `renew()` early, which the owner configured anyway.
- Unused deposit stays in your Tickets balance (path A: `scripts/fast-feed-ticket.sh withdraw <usdg>`) or the vault's (path B: `scripts/fast-feed-vault.sh withdraw`).
- `mev-feedlead` is read-only: it opens two WebSocket connections and writes files under the output directory.

## What was verified without a ticket

`cargo test -p mev-ingest` includes a mock Fast Feed server that requires `Authorization: Bearer <key>` and sends **binary** frames: the client decodes
them, counts a sequence gap, hands out raw samples, and receives nothing with a wrong key. (An earlier version of the client ignored binary frames and
would have connected and silently dropped every message.) The pairing/lead statistics are unit-tested (`crates/mev-ingest/src/lead.rs`).
Not verified until a ticket exists: the exact receipt/log JSON the real server sends (open question 1 in `docs/arbitrum-facts.md`).
