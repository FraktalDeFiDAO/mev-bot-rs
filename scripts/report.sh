#!/usr/bin/env bash
# Revenue vs. cost: the bot's net PnL from its event logs (production host) next to month-to-date
# AWS spend and the fixed monthly plans. Read-only.
#   scripts/report.sh            # uses ssh mev-ohio and AWS profile mev-bot
# Fixed costs below are monthly USD; edit when plans change.
set -uo pipefail
HOST="${MEV_HOST:-mev-ohio}"
CHAINSTACK_MONTHLY=49

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

echo "== infrastructure (month to date)"
START="$(date -u +%Y-%m-01)"; END="$(date -u -d tomorrow +%Y-%m-%d)"
if OUT=$(aws ce get-cost-and-usage --profile mev-bot --region us-east-1 --time-period "Start=$START,End=$END" \
    --granularity MONTHLY --metrics UnblendedCost --group-by Type=DIMENSION,Key=SERVICE --output json 2>&1); then
  python3 -c '
import sys, json
d = json.loads(sys.stdin.read())
g = sorted(d["ResultsByTime"][0]["Groups"], key=lambda g: -float(g["Metrics"]["UnblendedCost"]["Amount"]))
tot = sum(float(x["Metrics"]["UnblendedCost"]["Amount"]) for x in g)
for x in g[:6]: print("  AWS %-34s %8.2f" % (x["Keys"][0][:34], float(x["Metrics"]["UnblendedCost"]["Amount"])))
print("  AWS total (before credits)%24.2f" % tot)' <<<"$OUT"
else
  echo "  AWS cost unavailable: ${OUT%%$'\n'*} (run: aws login --profile mev-bot)"
fi
echo "  Chainstack plan                                $CHAINSTACK_MONTHLY.00 / month"
