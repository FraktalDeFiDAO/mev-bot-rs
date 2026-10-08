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

## Path B: a vault that renews by itself (for continuous use)

`contracts/src/FastFeedTicketVault.sol` is the subscriber. It holds the USDG deposit and buys tickets **as itself**, so the renewal priority belongs to
its address. `renew()` is callable by **anyone** but can only buy the owner-configured number of tickets for the owner-configured key hash, at no
more than the owner's price cap, once per round, while a renewals budget lasts. Only the owner can fund, change settings, or withdraw. So the
unattended **keeper account needs gas ETH only** and cannot touch the money.

```bash
export FF_OWNER_ACCOUNT=ff-owner
scripts/fast-feed-vault.sh deploy --yes                    # you deploy it (dry run without --yes); note the address
scripts/fast-feed-vault.sh setup <vault> --tickets 1 --max-price 40 --renewals 7 --deposit 150   # dry run; add --yes
# keeper: a SEPARATE keystore account holding ~0.002 ETH of gas, nothing else
cast wallet import ff-keeper --interactive
export FF_KEEPER_ACCOUNT=ff-keeper FF_PASSWORD_FILE=~/.config/mev-bot/keeper.pw   # file mode 600 with the keystore password
scripts/fast-feed-vault.sh keeper <vault> --loop 300       # every 5 min: renew if the contract says it can (use systemd/cron for production)
scripts/fast-feed-vault.sh status <vault>                  # anyone: deposit, renewals left, can renew now
scripts/fast-feed-vault.sh withdraw <vault> 50 --yes       # owner takes unused deposit back
```
The worst case spend is `renewals x tickets x max-price`, and never more than the deposit. Set `--renewals 0` (via `setConfig`) to stop automation.
**The contract is not audited and is not deployed.** It is ~150 lines, no proxy, no upgrade path, no admin backdoor, covered by 11 fork tests against the real
Tickets contract (`forge test --match-contract FastFeedTicketVaultForkTest`) and an end-to-end script run on a fork. Deploy it with a small deposit first.

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
