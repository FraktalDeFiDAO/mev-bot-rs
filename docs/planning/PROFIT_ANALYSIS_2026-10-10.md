# Profit analysis, 2026-10-10 (evidence: `events-*.jsonl` on mev-ohio, 2026-10-08 to 10-10)

## Arbitrage: structurally negative at this latency tier
- 192 landed sends, **0 wins**. Spent: gas $25.31 + tips $17.06 = **$42.4 in 3 days** (about $14/day; the daily rate fell after
  `fast_send` was turned off). Revenue: $0.
- Break-even needs a win rate of about cost / (net + cost) = 0.10 / (0.29 + 0.10) = **26%** (median claimed edge $0.29, gas $0.07,
  tip $0.03). Observed 0 of 192: the 95% upper bound on the true win rate is about 3 / 192 = **1.6%**, sixteen times below break-even.
- Races: 192 measured, winner identified in 40: 19 lost on timing (we paid more, still behind), 21 lost on tip.
- Why 192 sends from a 0% record: the per-route gate gives every route `min_attempts = 3` free tries, with about 64 routes that is
  about 192 losing sends before any back-off, and the gate's global record reset on every restart. **Fix in this PR (DECISIONS #132):
  a global stop (30 sends, zero wins -> stop, one probe per hour), persisted across restarts.**

## Liquidations: the only upside, and small and rare so far
- One Aave account (`0x5FeE...`, WETH collateral): bonus about $97, net about $68. **11 backruns landed and all reverted** (about $0.7 of gas, cheap);
  it also appeared in 18 Atlas auctions the bot logged as `atlas_would_bid` (observe-only). Atlas bidding needs a bond of about 0.1 ETH.
- Morpho `liq_no_route` (8 accounts, one showing a $20,789 bonus) is **not an opportunity**: the collateral is Stream's xUSD, Resolv's RLP
  and wstUSR, distressed tokens with no liquid market; refusing them is correct. `liq_implausible` (666 events, bonuses up to $1M) is bad
  price data. `liq_dust` (4,606) is below the profit floor.
- So: roughly one real liquidation candidate in three calm days. A crash day is the upside; nothing in the data shows we win the race to one
  without Atlas.

## Costs
- Chainstack $49/month (about $1.6/day) is the main fixed cost; AWS runs on credits. At about $68 per liquidation, **one captured
  liquidation a month covers Chainstack alone**; nothing else pays for it today.

## Recommendation, in order
1. Merge and deploy #115 (global stop): stops the arbitrage bleed without touching the liquidation path.
2. Check which paths still need the keyed Chainstack endpoints once arbitrage is stopped (presim and `eth_simulateV1` serve arbitrage and
   decoding; liquidation sweeps use public RPCs). If none, the $49/month can be cancelled until there is profit to protect.
3. Decide on the Atlas bond (about 0.1 ETH, about 15% of capital) only if you want a shot at crash-day liquidations; expected value is
   unknown and bounded by the bonus times the solver's share of an auction.
4. Do not buy the Fast Feed ($510/month) on this evidence: the arbitrage win rate would have to rise from under 2% to over 26%.
