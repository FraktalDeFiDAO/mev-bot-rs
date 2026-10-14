#!/usr/bin/env bash
# RAM/CPU monitor for the production host (systemd user timer every minute; or by hand):
#   scripts/resources.sh            sample once: append a JSONL line, print it, alert on thresholds
#   scripts/resources.sh show [N]   last N samples (default 20) as a table
# Samples go to data/arbitrum/resources-YYYY-MM-DD.jsonl. Alerts use the watchdog's channel:
# data/arbitrum/ALERTS.log and the optional data/arbitrum/alert_webhook. Exit 1 on any alert.
# Thresholds (env): MIN_MEM_AVAIL_MB (1024), MAX_BOT_MEM_MB (3072), MAX_LOAD_PER_CPU (1.5),
# MAX_SWAP_USED_MB (1024). A container restart since the previous sample is always an alert
# (an OOM kill restarts the bot; a rootless user cannot read the kernel log on every host).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
D=data/arbitrum
mkdir -p "$D" || exit 1
CONTAINERS=(mev-bot-arbitrum mev-jev-advisor)

if [[ ${1:-} == show ]]; then
  n="${2:-20}"
  files=$(ls "$D"/resources-*.jsonl 2>/dev/null | tail -2)
  [[ -n $files ]] || { echo "no samples yet in $D"; exit 0; }
  # shellcheck disable=SC2086
  tail -q -n "$n" $files | tail -n "$n" | jq -r '
    [.ts[11:19], (.mem_avail_mb|tostring), (.swap_used_mb|tostring), (.load1|tostring), (.cpu_pct|tostring),
     ((.containers["mev-bot-arbitrum"].mem_mb // "-")|tostring), ((.containers["mev-bot-arbitrum"].cpu_pct // "-")|tostring),
     ((.containers["mev-bot-arbitrum"].restarts // "-")|tostring),
     ((.containers["mev-jev-advisor"].mem_mb // "-")|tostring)] | @tsv' |
    (echo -e "time\tavail_MB\tswap_MB\tload1\thost_cpu%\tbot_MB\tbot_cpu%\tbot_restarts\tadvisor_MB"; cat) | column -t
  exit 0
fi

read -r mem_total mem_avail swap_total swap_free < <(awk '
  /^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} /^SwapTotal:/{st=$2} /^SwapFree:/{sf=$2}
  END{printf "%d %d %d %d\n", t/1024, a/1024, st/1024, sf/1024}' /proc/meminfo)
swap_used=$(( swap_total - swap_free ))
read -r load1 load5 _ < /proc/loadavg
cpus=$(nproc)

# Host CPU busy % over a 1 s window from /proc/stat.
cpu_sample() { awk '/^cpu /{idle=$5+$6; tot=0; for(i=2;i<=NF;i++) tot+=$i; print idle, tot}' /proc/stat; }
read -r i1 t1 < <(cpu_sample); sleep 1; read -r i2 t2 < <(cpu_sample)
cpu_pct=$(awk -v i="$((i2-i1))" -v t="$((t2-t1))" 'BEGIN{printf "%.1f", (t>0 ? 100*(1-i/t) : 0)}')

state="$D/.resources-state"
declare -A prev_restarts=()
if [[ -f $state ]]; then while read -r k v; do prev_restarts[$k]=$v; done < "$state"; fi

alerts=()
cjson="{}"
: > "$state.tmp"
for c in "${CONTAINERS[@]}"; do
  podman container exists "$c" 2>/dev/null || continue
  restarts=$(podman inspect "$c" --format '{{.RestartCount}}' 2>/dev/null || echo 0)
  started=$(podman inspect "$c" --format '{{.State.StartedAt}}' 2>/dev/null | cut -c1-19)
  stats=$(podman stats --no-stream --format '{{.MemUsage}}|{{.CPUPerc}}' "$c" 2>/dev/null | head -1)
  # MemUsage looks like "812.3MB / 8.3GB"; convert the first value to MB.
  mem_mb=$(awk -F'|' '{split($1,a," "); v=a[1]; u=v; gsub(/[0-9.]/,"",u); gsub(/[^0-9.]/,"",v);
    m=(u ~ /^G/)?1024:(u ~ /^k/)?1/1024:(u ~ /^B/)?1/1048576:1; printf "%d", v*m}' <<< "$stats")
  ccpu=$(awk -F'|' '{v=$2; gsub(/%/,"",v); printf "%.1f", v+0}' <<< "$stats")
  cjson=$(jq -c --arg c "$c" --argjson m "${mem_mb:-0}" --argjson p "${ccpu:-0}" --argjson r "${restarts:-0}" \
    --arg s "$started" '.[$c] = {mem_mb:$m, cpu_pct:$p, restarts:$r, started:$s}' <<< "$cjson")
  echo "$c $restarts" >> "$state.tmp"
  if [[ -n ${prev_restarts[$c]:-} && $restarts -gt ${prev_restarts[$c]} ]]; then
    alerts+=("$c restarted $((restarts - prev_restarts[$c]))x since last sample (OOM kill?); mem now ${mem_mb}MB")
  fi
  if [[ $c == mev-bot-arbitrum ]] && (( mem_mb > ${MAX_BOT_MEM_MB:-3072} )); then
    alerts+=("bot memory ${mem_mb}MB > ${MAX_BOT_MEM_MB:-3072}MB")
  fi
done
mv "$state.tmp" "$state"

(( mem_avail < ${MIN_MEM_AVAIL_MB:-1024} )) && alerts+=("host memory low: ${mem_avail}MB available of ${mem_total}MB")
(( swap_used > ${MAX_SWAP_USED_MB:-1024} )) && alerts+=("host swapping: ${swap_used}MB swap used")
awk -v l="$load1" -v c="$cpus" -v m="${MAX_LOAD_PER_CPU:-1.5}" 'BEGIN{exit !(l > c*m)}' \
  && alerts+=("host load ${load1} > ${MAX_LOAD_PER_CPU:-1.5} x ${cpus} CPUs")

ts="$(date -u +%FT%TZ)"
line=$(jq -c -n --arg ts "$ts" --argjson mt "$mem_total" --argjson ma "$mem_avail" --argjson su "$swap_used" \
  --argjson l1 "$load1" --argjson l5 "$load5" --argjson n "$cpus" --argjson cp "$cpu_pct" --argjson cs "$cjson" \
  --argjson na "${#alerts[@]}" \
  '{ts:$ts, mem_total_mb:$mt, mem_avail_mb:$ma, swap_used_mb:$su, load1:$l1, load5:$l5, cpus:$n, cpu_pct:$cp,
    containers:$cs, alerts:$na}')
echo "$line" >> "$D/resources-$(date -u +%F).jsonl"
echo "$line"

(( ${#alerts[@]} == 0 )) && exit 0
for a in "${alerts[@]}"; do
  echo "ALERT $a"
  echo "$ts resources: $a" >> "$D/ALERTS.log"
  [[ -s $D/alert_webhook ]] && curl -s -m 10 -H 'content-type: application/json' \
    -d "$(jq -n --arg t "mev-bot: $a" '{text:$t}')" "$(cat "$D/alert_webhook")" >/dev/null || true
done
exit 1
