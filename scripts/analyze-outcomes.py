#!/usr/bin/env python3
"""Join the feed recording with the chain outcomes and report what the decoder must still cover.

  scripts/analyze-outcomes.py [DATA_DIR]            (default data/arbitrum; run on the server)

Reads feedtx-*.jsonl (what the sequencer feed showed) and chaintx-*.jsonl (sampled block receipts), joined on the
transaction hash. Reports:
  1. Revert rate per (destination, selector) for feed candidates that were in a sampled block.
  2. Decoder coverage: of the chain transactions that emitted a swap / liquidity / liquidation event, how many had a
     decoded swap in the feed recording, and which destinations account for the ones that did not.
  3. Where reverted candidates sat in the block relative to their tip.
"""
import collections, glob, json, os, statistics, sys

d = sys.argv[1] if len(sys.argv) > 1 else "data/arbitrum"

def lines(pattern):
    for f in sorted(glob.glob(os.path.join(d, pattern))):
        for l in open(f):
            try:
                yield json.loads(l)
            except Exception:
                pass

feed = {r["hash"]: r for r in lines("feedtx-*.jsonl")}
chain = {r["hash"]: r for r in lines("chaintx-*.jsonl")}
print(f"feed candidates: {len(feed)}   chain txs recorded: {len(chain)}   joined: {len(set(feed) & set(chain))}")
if not chain:
    sys.exit("no chaintx data yet")

# 1. revert rate per destination + selector
agg = collections.defaultdict(lambda: [0, 0])
for h, c in chain.items():
    f = feed.get(h)
    if not f:
        continue
    k = (f["to"][:10] if f.get("to") else "-", f.get("sel") or "-")
    agg[k][0] += 1
    if not c["status"]:
        agg[k][1] += 1
print("\n1. revert rate of feed candidates seen in a sampled block (top by volume)")
print(f"  {'to':12s} {'selector':9s} {'n':>6s} {'reverted':>9s} {'rate':>6s}")
for k, (n, bad) in sorted(agg.items(), key=lambda kv: -kv[1][0])[:15]:
    print(f"  {k[0]:12s} {k[1]:9s} {n:6d} {bad:9d} {100*bad/n:5.1f}%")
tn = sum(v[0] for v in agg.values()); tb = sum(v[1] for v in agg.values())
if tn:
    print(f"  all joined candidates: {tn}, reverted {tb} ({100*tb/tn:.1f}%)")

# 2. coverage of event-emitting transactions
by_class = collections.defaultdict(lambda: [0, 0])      # class -> [txs, with decoded swap in feed record]
gaps = collections.defaultdict(collections.Counter)      # class -> Counter(to)
for h, c in chain.items():
    if not c["status"]:
        continue
    for cls in c.get("classes", []):
        if cls == "state":
            continue
        by_class[cls][0] += 1
        f = feed.get(h)
        # A decoded intent of the matching kind: a swap for swap events, a liquidity change for liquidity events, a
        # liquidation for liquidation events.
        field = {"swap": "swap", "liquidity": "liquidity", "liquidation": "liquidation"}.get(cls, "swap")
        if f and f.get(field):
            by_class[cls][1] += 1
        else:
            gaps[cls][(c.get("to") or "-")[:42]] += 1
print("\n2. decoder coverage of successful chain transactions, by event class")
for cls, (n, ok) in sorted(by_class.items()):
    print(f"  {cls:12s} txs {n:6d}   decoded in feed record {ok:6d}  ({100*ok/n:.1f}%)")
print("\n   largest uncovered destinations per class (to, count):")
for cls, cnt in sorted(gaps.items()):
    print(f"  {cls}")
    for to, n in cnt.most_common(8):
        print(f"    {to}  {n}")

# 3. reverted candidates: tip and position
rv = [(feed[h], c) for h, c in chain.items() if h in feed and not c["status"]]
ok = [(feed[h], c) for h, c in chain.items() if h in feed and c["status"]]
def tips(rows):
    return [int(f["tip"]) for f, _ in rows if f.get("tip") not in (None, "")]
for name, rows in (("reverted", rv), ("succeeded", ok)):
    t = tips(rows)
    if t:
        print(f"\n3. {name} candidates: n={len(rows)} tip wei median {statistics.median(t):.0f}, zero-tip share {100*sum(1 for x in t if x == 0)/len(t):.0f}%")
