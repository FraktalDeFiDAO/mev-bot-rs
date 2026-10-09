#!/usr/bin/env python3
"""A complete view of the watched exchanges, from the journals, with a completeness check.

  scripts/exchange-view.py [DATA_DIR] [--since 2026-10-09T03:00:00]        (run on the server)

Reads `poolevents-*.jsonl` (every event of every tracked pool, every block: written by the engine from its log
subscription), `tracked-venues.json` and, for the check, `chaintx-*.jsonl` (receipts of a sampled 1 in 4 blocks, an
independent source). Reports per venue the swaps, liquidity added and removed (with amounts), the busiest pools, the
liquidity actions in full, and:

  COMPLETENESS: of the events the receipt sampler saw on tracked pools, how many are in the journal (same transaction
  and event kind and pool). 100% is the standard; any miss is listed.
"""
import calendar, collections, glob, json, os, sys, time

args = [a for a in sys.argv[1:] if not a.startswith("--")]
d = args[0] if args else "data/arbitrum"
since_ms = 0
if "--since" in sys.argv:
    since_ms = calendar.timegm(time.strptime(sys.argv[sys.argv.index("--since") + 1], "%Y-%m-%dT%H:%M:%S")) * 1000

def lines(pattern):
    for f in sorted(glob.glob(os.path.join(d, pattern))):
        for l in open(f):
            try:
                yield json.loads(l)
            except Exception:
                pass

ev = [e for e in lines("poolevents-*.jsonl") if e.get("t_ms", 0) >= since_ms]
if not ev:
    sys.exit("no journal lines yet (is [journal] pool_events on, and has the build run?)")
tracked = json.load(open(os.path.join(d, "tracked-venues.json")))
pool_dex = {p["address"].lower(): p["dex"] for p in tracked["pools"]}
for p in tracked["pools"]:
    if p.get("v4_id"):
        pool_dex["0x" + p["v4_id"][-40:].lower()] = p["dex"]
t0, t1 = min(e["t_ms"] for e in ev), max(e["t_ms"] for e in ev)
b0, b1 = min(e["block"] for e in ev if e.get("block")), max(e["block"] for e in ev if e.get("block"))
print(f"journal: {len(ev)} events, blocks {b0}..{b1} ({(t1 - t0) / 60000:.0f} min)\n")

by_kind = collections.Counter((e["dex"] or "(untracked)", e["kind"]) for e in ev)
print(f"{'venue':16s} {'kind':26s} {'events':>7s}")
for (dex, k), n in sorted(by_kind.items(), key=lambda kv: (kv[0][0], -kv[1])):
    print(f"{dex:16s} {k:26s} {n:7d}")

liq = [e for e in ev if e["class"] == "liquidity"]
adds = [e for e in liq if e["kind"].startswith(("mint", "increase")) or (e["kind"] == "modify_liquidity_v4" and not e["fields"].get("liquidity_delta", "0").startswith("-") and e["fields"].get("liquidity_delta", "0") != "0")]
print(f"\nliquidity actions: {len(liq)} (adds {len(adds)}, removes {len(liq) - len(adds)})")
for e in sorted(liq, key=lambda e: (e["block"] or 0, e["log_index"] or 0))[-12:]:
    f = e["fields"]
    amt = f.get("liquidity") or f.get("liquidity_delta") or "?"
    print(f"  block {e['block']} {e['dex'] or '(untracked)':14s} {e['kind']:22s} pool {e['pool'][:12]} ticks {f.get('tick_lower','-')}..{f.get('tick_upper','-')} liquidity {amt} tx {str(e['tx'])[:12]}")

busy = collections.Counter(e["pool"] for e in ev if e["class"] == "swap")
print("\nbusiest pools by swaps:", ", ".join(f"{p[:10]}({pool_dex.get(p, 'untracked')}) {n}" for p, n in busy.most_common(6)))
untracked = sum(1 for e in ev if not e["tracked"]); print(f"events on untracked pools (V4 manager events of pools we do not hold): {untracked}")

# completeness: the receipt sampler is independent of the engine's subscription
chain = [c for c in lines("chaintx-*.jsonl") if (c.get("block") or 0) >= b0 and (c.get("block") or 0) <= b1 and c.get("status") is True]
KINDS = {"swap_v2","sync_v2","mint_v2","burn_v2","swap_v3","mint_v3","burn_v3","swap_v3_pancake","swap_v4","modify_liquidity_v4"}
have = {(e["tx"], e["kind"], e["pool"]) for e in ev}
seen = miss = 0; misses = collections.Counter()
for c in chain:
    for l in c["logs"]:
        if l["kind"] not in KINDS:
            continue
        pool = ("0x" + l["topics"][1][-40:]).lower() if l["kind"] in ("swap_v4", "modify_liquidity_v4") else l["addr"].lower()
        if pool not in pool_dex:
            continue                                  # not a tracked pool
        seen += 1
        if (c["hash"], l["kind"], pool) not in have:
            miss += 1; misses[(l["kind"], pool_dex[pool])] += 1
pct = 100 * (seen - miss) / seen if seen else 0
print(f"\nCOMPLETENESS vs the receipt sampler: {seen - miss}/{seen} events on tracked pools are in the journal ({pct:.2f}%)")
for (k, dex), n in misses.most_common(8):
    print(f"  missing {n}: {k} on {dex}")
sys.exit(0 if seen and miss == 0 else (0 if not seen else 1))
