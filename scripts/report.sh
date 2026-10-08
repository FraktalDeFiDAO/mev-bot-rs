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

if [[ -n "${FF_VAULT:-}" ]]; then
  echo "== Fast Feed vault (ticket spend and conversion drag, last 30 days)"
  python3 - "${FF_RPC:-https://arb1.arbitrum.io/rpc}" "$FF_VAULT" <<'PY' 2>&1 | head -8
import json, sys, urllib.request
rpc, vault = sys.argv[1], sys.argv[2]
def call(m, p):
    r = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).encode(), {"content-type": "application/json", "user-agent": "report"})
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]
head = int(call("eth_blockNumber", []), 16)
import os
frame = int(os.environ.get("FF_REPORT_LOOKBACK_BLOCKS", 4 * 86400 * 30))  # about 30 days at 4 blocks/s
frm = head - frame
def logs(topic):
    out = []
    for lo in range(frm, head + 1, 9_000_000):
        out += call("eth_getLogs", [{"address": vault, "topics": [topic], "fromBlock": hex(lo), "toBlock": hex(min(head, lo + 8_999_999))}])
    return out
w = lambda d, i: int(d[2 + 64 * i: 2 + 64 * (i + 1)], 16)
renewed_topic = "0x5675981f7d0c04b92b1d6a02125be55a9463468beef1b58ac70c6be74a503407"  # keccak of Renewed(...)
rn = logs(renewed_topic)
paid = sum(w(l["data"], 2) for l in rn); tickets = sum(w(l["data"], 1) for l in rn)
print(f"  ticket renewals {len(rn)}: {tickets} ticket(s), {paid/1e6:.2f} USDG paid (about ${paid/1e6/30:.2f}/day over 30 days)")
converted_topic = "0x80aaefaf59d70d6ed6ecae25018f99e73b43f7dc60af403d422b060c6c1df02c"  # keccak of Converted(...)
cv = logs(converted_topic)
# Converted(token indexed, amountIn, out, minOut, expected): data words 0..3
drag = sum(max(w(l["data"], 3) - w(l["data"], 1), 0) for l in cv); vol = sum(w(l["data"], 3) for l in cv)
print(f"  conversions {len(cv)}: ${vol/1e6:.2f} converted, drag vs oracle value {drag/1e6:.4f} USDG" + (f" ({drag/vol*1e4:.1f} bps)" if vol else ""))
PY
fi

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
