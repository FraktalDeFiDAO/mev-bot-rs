#!/usr/bin/env python3
"""Check decoded swap intents against what the chain did.

  scripts/verify-decoder.py [DATA_DIR] [--since 2026-10-08T19:59:55]   (default data/arbitrum; run on the server)

--since keeps only feed records received at or after that UTC time, so a decoder fix can be judged without the
records written by the build before it.

For every feed transaction the decoder read a swap from (feedtx `swap` field) that landed successfully in a sampled
block (chaintx), compare the intent with the transaction's own swap events:
  - pool: some Swap event was emitted by the decoded pool (only checked when the call names the pool by address);
  - amount: the input side of that swap equals the decoded `amount_in` (exact), or is smaller (a price limit stopped it);
  - a successful transaction with a decoded swap but no swap event at all is a false positive.
Reports accuracy per destination and selector. A decoder is trusted when pool and amount agree in (nearly) every case.
"""
import collections, glob, json, os, sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
d = args[0] if args else "data/arbitrum"
since_ms = 0
if "--since" in sys.argv:
    import calendar, time
    since_ms = calendar.timegm(time.strptime(sys.argv[sys.argv.index("--since") + 1], "%Y-%m-%dT%H:%M:%S")) * 1000

def lines(pattern):
    for f in sorted(glob.glob(os.path.join(d, pattern))):
        for l in open(f):
            try:
                yield json.loads(l)
            except Exception:
                pass

def signed(word_hex):
    v = int(word_hex, 16)
    return v - (1 << 256) if v >> 255 else v

def swap_inputs(log):
    """(pool, input amount) of a swap event log, or None."""
    data = log.get("data", "")
    words = [data[i:i + 64] for i in range(0, len(data), 64)]
    kind = log.get("kind", "")
    if kind in ("swap_v3", "swap_v3_pancake") and len(words) >= 2:
        a0, a1 = signed(words[0]), signed(words[1])
        return log["addr"].lower(), max(a0, a1, 0)
    if kind == "swap_v2" and len(words) >= 4:
        return log["addr"].lower(), max(int(words[0], 16), int(words[1], 16))
    if kind == "swap_v4" and len(words) >= 2:
        # V4 pools have no contract: the event comes from the PoolManager and names the pool by id (topic 1). Decoders
        # name a V4 pool by its stand-in address, the last 20 bytes of the id. Which sign is "in" depends on whose side
        # the amounts are quoted from, so the input is taken as the larger magnitude on the negative side, else either.
        a0, a1 = signed(words[0]), signed(words[1])
        pool = "0x" + log["topics"][1][-40:]
        return pool.lower(), max(-a0, -a1, 0) or max(abs(a0), abs(a1))
    return None

feed = {r["hash"]: r for r in lines("feedtx-*.jsonl") if r.get("swap") and r.get("t_ms", 0) >= since_ms}
chain = {r["hash"]: r for r in lines("chaintx-*.jsonl")}
agg = collections.defaultdict(lambda: collections.Counter())
for h, f in feed.items():
    c = chain.get(h)
    if not c or c.get("status") is not True:
        continue
    k = ((f.get("to") or "-")[:10], f.get("sel") or "-")
    a = agg[k]
    a["n"] += 1
    events = [x for x in (swap_inputs(l) for l in c.get("logs", [])) if x]
    if not events:
        a["no_swap_event"] += 1
        continue
    s = f["swap"]
    amount = int(s["amount_in"])
    pool = s["pool"].lower()
    if "/" not in pool:                      # named by address
        match = [e for e in events if e[0] == pool]
        if match:
            a["pool_ok"] += 1
        else:
            a["pool_wrong"] += 1
            continue
        used = max(e[1] for e in match)
    else:                                    # named by token pair: check amount against any swap in the transaction
        a["pool_by_pair"] += 1
        used = max(e[1] for e in events)
    if used == amount:
        a["amount_exact"] += 1
    elif 0 < used < amount:
        a["amount_less"] += 1                # stopped early (price limit or partial fill)
    elif used > amount:
        a["amount_more"] += 1                # the decoder under-read the amount
    else:
        a["amount_zero"] += 1

print(f"decoded swaps joined to a successful chain transaction: {sum(v['n'] for v in agg.values())}")
print(f"{'to':12s} {'selector':9s} {'n':>5s} {'pool ok':>8s} {'by pair':>8s} {'wrong':>6s} {'no event':>9s} | {'amt exact':>9s} {'less':>5s} {'more':>5s}")
tot = collections.Counter()
for k, a in sorted(agg.items(), key=lambda kv: -kv[1]["n"]):
    tot.update(a)
    print(f"{k[0]:12s} {k[1]:9s} {a['n']:5d} {a['pool_ok']:8d} {a['pool_by_pair']:8d} {a['pool_wrong']:6d} {a['no_swap_event']:9d} | {a['amount_exact']:9d} {a['amount_less']:5d} {a['amount_more']:5d}")
n = tot["n"] or 1
print(f"\nTOTAL n={tot['n']}: pool wrong {tot['pool_wrong']} ({100*tot['pool_wrong']/n:.1f}%), no swap event {tot['no_swap_event']} ({100*tot['no_swap_event']/n:.1f}%), amount over-read? none; amount under-read {tot['amount_more']} ({100*tot['amount_more']/n:.1f}%)")
