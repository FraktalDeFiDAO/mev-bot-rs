#!/usr/bin/env bash
# Terminal view of the liquidation snapshot pipeline (shadow), from the bot's own log lines:
# the last census (tiers, freshness, lowest health factors), the refresh totals and the last
# schedule pass. Read-only; needs the bot container running with [liquidation_shadow] enabled.
#   scripts/liq-view.sh [WINDOW]     WINDOW = podman logs --since value (default 15m)
set -euo pipefail
CTR=${CTR:-mev-bot-arbitrum}
since=${1:-15m}

logs=$(podman logs --since "$since" "$CTR" 2>&1 | sed 's/\x1b\[[0-9;]*m//g') || {
  echo "no logs from $CTR"
  exit 1
}
# field NAME LINE -> the value of NAME=... in a tracing line ("" when absent)
field() { grep -o "\b$1=[^ ]*" <<<"$2" | head -1 | cut -d= -f2-; }
pct() { awk -v a="$1" -v b="$2" 'BEGIN { if (b > 0) printf "%.1f%%", 100 * a / b; else printf "-" }'; }

census=$(grep ' liq_shadow_census' <<<"$logs" | grep -v 'census failed' | tail -1 || true)
if [[ -z $census ]]; then
  echo "no liq_shadow_census in the last $since (pipeline off, or the bot started < 1 min ago)"
  grep -E 'liq_shadow|liquidation shadow' <<<"$logs" | tail -3 || true
  exit 1
fi
c() { field "$1" "$census"; }
echo "Liquidation pipeline (shadow) — census at ${census%% *}"
printf '  borrowers  active %-9s inactive %-9s never read %s\n' "$(c active)" "$(c inactive)" "$(c unknown)"
printf '  hot        %-9s fresh %-9s (%s)   eligible (HF<1) %s\n' "$(c hot)" "$(c hot_fresh)" "$(pct "$(c hot_fresh)" "$(c hot)")" "$(c eligible)"
printf '  cold/dust  %-9s fresh %-9s (%s)   dirty %s\n' "$(c cold)" "$(c cold_fresh)" "$(pct "$(c cold_fresh)" "$(c cold)")" "$(c dirty)"

refresh=$(grep ' liq_shadow_refresh' <<<"$logs" | grep 'stored=' || true)
if [[ -n $refresh ]]; then
  awk '{ for (i = 1; i <= NF; i++) { split($i, kv, "="); if (kv[1] ~ /^(stored|conflicts|rpc_errors|store_errors)$/) s[kv[1]] += kv[2] } n++ }
       END { printf "  rechecks   last %s: %d rounds, stored %d, superseded %d, rpc errors %d, store errors %d\n", since, n, s["stored"], s["conflicts"], s["rpc_errors"], s["store_errors"] }' \
    since="$since" <<<"$refresh"
fi
batch_err=$(grep 'liq_shadow_refresh: batch failed' <<<"$logs" | tail -1 || true)
[[ -n $batch_err ]] && echo "  last batch error: ${batch_err#*error=}" | cut -c1-200

sched=$(grep 'liq_sched_shadow' <<<"$logs" | grep ' p0=' | tail -1 || true)
if [[ -n $sched ]]; then
  s() { field "$1" "$sched"; }
  printf '  schedule   P0 %s  P1 %s  P2 %s  P3 %s  dispatched %s  backlog P0/P1 %s/%s\n' \
    "$(s p0)" "$(s p1)" "$(s p2)" "$(s p3)" "$(s dispatched)" "$(s backlog_p0)" "$(s backlog_p1)"
  rec=$(grep -o 'record=.*' <<<"$sched" || true)
  tracked=$(grep -o '"tracked":[0-9]*' <<<"$rec" | cut -d: -f2 || true)
  deferred=$(grep -o '"deferred_unknown":[0-9]*' <<<"$rec" | cut -d: -f2 || true)
  violations=$(grep -o '"coverage_violations":[0-9]*' <<<"$rec" | head -1 | cut -d: -f2 || true)
  echo "             tracked ${tracked:-?}, deferred never-read ${deferred:-?}, coverage violations ${violations:-?}"
fi

echo "  lowest health factors (hot):"
lowest=$(field lowest "$census")
if [[ -z $lowest || $lowest == "[]" ]]; then
  echo "    (none yet)"
else
  sed 's/^\[{//; s/}\]$//; s/},{/\n/g' <<<"$lowest" | while IFS= read -r row; do
    b=$(grep -o '"borrower":"[^"]*"' <<<"$row" | cut -d'"' -f4)
    hf=$(grep -o '"hf":[^,]*' <<<"$row" | cut -d: -f2)
    debt=$(grep -o '"debt_usd":[^,]*' <<<"$row" | cut -d: -f2)
    age=$(grep -o '"age_s":[^,}]*' <<<"$row" | cut -d: -f2)
    awk -v b="$b" -v hf="$hf" -v d="$debt" -v a="$age" \
      'BEGIN { printf "    %s  HF %7.4f  debt $%12.2f  checked %ss ago\n", b, hf, d, a }'
  done
fi
