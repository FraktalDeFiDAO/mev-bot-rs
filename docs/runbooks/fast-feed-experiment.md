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

## What you need

- A **separate wallet** (not the bot's hot wallet) with **≥ 40-50 USDG** (price 17, "can rise up to about two times between rounds", deposit for several rounds) and a little ETH on Arbitrum One for three transactions.
- Foundry (`cast`) and `openssl` on the machine you run it from.
- Time: a ticket bought during round N is active for round N+1 only. `scripts/fast-feed-ticket.sh status` prints when that is.

## Steps (you run these; the agent never handles keys or funds)

```bash
scripts/fast-feed-ticket.sh status                      # round, price, when a new ticket becomes active
scripts/fast-feed-ticket.sh keygen                      # API key -> ~/.config/mev-bot/fast-feed.key (0600); prints its hash
export FF_BUYER_PRIVATE_KEY=0x...                       # the separate wallet; only in this shell
scripts/fast-feed-ticket.sh status <wallet-address>     # check USDG + ETH balances
scripts/fast-feed-ticket.sh buy --tickets 1 --deposit-usdg 40          # DRY RUN: prints the plan, sends nothing
scripts/fast-feed-ticket.sh buy --tickets 1 --deposit-usdg 40 --yes    # asks you to type 'buy', then approve + deposit + purchase
unset FF_BUYER_PRIVATE_KEY
```

After the round ends (time printed by `status`), run the measurement for an hour, from the host you care about (the leads are differences between
two feeds seen from the same place, so a workstation works; the Ohio host is the most representative):

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
- `buy` moves real USDG. It is a dry run unless `--yes`, and still asks for a typed confirmation. The paying key is read from `FF_BUYER_PRIVATE_KEY`
  and handed to `cast` through `ETH_PRIVATE_KEY`, so it never appears on a command line.
- Unused deposit stays in your Tickets balance: `scripts/fast-feed-ticket.sh withdraw <usdg>`.
- `mev-feedlead` is read-only: it opens two WebSocket connections and writes files under the output directory.

## What was verified without a ticket

`cargo test -p mev-ingest` includes a mock Fast Feed server that requires `Authorization: Bearer <key>` and sends **binary** frames: the client decodes
them, counts a sequence gap, hands out raw samples, and receives nothing with a wrong key. (An earlier version of the client ignored binary frames and
would have connected and silently dropped every message.) The pairing/lead statistics are unit-tested (`crates/mev-ingest/src/lead.rs`).
Not verified until a ticket exists: the exact receipt/log JSON the real server sends (open question 1 in `docs/arbitrum-facts.md`).
