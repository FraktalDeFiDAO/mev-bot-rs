#!/usr/bin/env bash
# Revenue vs. cost: the bot's net PnL from its event logs (production host) next to month-to-date
# AWS spend and the fixed monthly plans. Read-only.
#   scripts/report.sh            # uses ssh mev-ohio and AWS profile mev-bot
# Fixed costs below are monthly USD; edit when plans change.
set -uo pipefail
HOST="${MEV_HOST:-mev-ohio}"
CHAINSTACK_MONTHLY=49

echo "== health (scripts/watchdog.sh on $HOST)"
ssh -o ConnectTimeout=15 "$HOST" 'cd src/mev-bot && scripts/watchdog.sh' 2>&1 | head -6

echo "== trading (event logs on $HOST)"
ssh -o ConnectTimeout=15 "$HOST" 'cd src/mev-bot && ls data/arbitrum/events-*.jsonl | tail -3 | xargs python3 -c "
import sys, json, collections
net = collections.Counter(); n = collections.Counter()
for f in sys.argv[1:]:
    day = f.rsplit(\"events-\", 1)[1][:10]
    for l in open(f):
        try: e = json.loads(l)
        except Exception: continue
        d = e.get(\"data\", {})
        if e.get(\"kind\") == \"landed\":
            n[day] += 1
            gas = d.get(\"gas_usd\", 0); prof = d.get(\"profit_usd\", d.get(\"est_profit_usd\", 0)) if d.get(\"success\") else 0
            net[day] += prof - gas
for day in sorted(n): print(\"  %s  landed %3d  net %+.4f USD (profit when won - gas)\" % (day, n[day], net[day]))
"' 2>&1 | head -8

echo "== races (landed arb sends; winner = the tx ahead of ours that touched our pools)"
ssh -o ConnectTimeout=15 "$HOST" 'cd src/mev-bot && ls data/arbitrum/events-*.jsonl | tail -3 | xargs python3 -c "
import sys, json, collections
c = collections.Counter(); late = []
for f in sys.argv[1:]:
    for l in open(f):
        try: e = json.loads(l)
        except Exception: continue
        if e.get(\"kind\") != \"race\": continue
        d = e[\"data\"]
        c[\"races\"] += 1; c[\"won\"] += bool(d.get(\"success\"))
        if d.get(\"winner\"): c[\"winner identified\"] += 1
        ours, win = d.get(\"our_tip_wei\"), d.get(\"winner_tip_wei\")
        if d.get(\"winner\") and ours is not None and win is not None:
            c[\"lost on tip (winner paid >= us)\" if int(win) >= int(ours) else \"lost on timing (we paid more, still behind)\"] += 1
        if d.get(\"blocks_after_trigger\") is not None: late.append(d[\"blocks_after_trigger\"])
for k, v in c.items(): print(\"  %-46s %d\" % (k, v))
if late: print(\"  blocks between trigger and our landing: median %d, max %d\" % (sorted(late)[len(late)//2], max(late)))
"' 2>&1 | head -10

echo "== infrastructure (month to date)"
START="$(date -u +%Y-%m-01)"; END="$(date -u -d tomorrow +%Y-%m-%d)"
if OUT=$(aws ce get-cost-and-usage --profile mev-bot --region us-east-1 --time-period "Start=$START,End=$END" \
    --granularity MONTHLY --metrics UnblendedCost --group-by Type=DIMENSION,Key=RECORD_TYPE --output json 2>&1); then
  python3 -c '
import sys, json
d = json.loads(sys.stdin.read())
g = {x["Keys"][0]: float(x["Metrics"]["UnblendedCost"]["Amount"]) for x in d["ResultsByTime"][0]["Groups"]}
use, cred = g.get("Usage", 0.0), g.get("Credit", 0.0)
print("  AWS usage %8.2f   credits %8.2f   net billed %8.2f   (Cost Explorer lags up to a day)" % (use, cred, use + cred))' <<<"$OUT"
else
  echo "  AWS cost unavailable: ${OUT%%$'\n'*} (run: aws login --profile mev-bot)"
fi
echo "  Chainstack plan                                $CHAINSTACK_MONTHLY.00 / month"
