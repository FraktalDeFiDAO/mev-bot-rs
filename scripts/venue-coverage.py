#!/usr/bin/env python3
"""Are the tracked exchanges and markets decoded? Per venue, against the chain.

  scripts/venue-coverage.py [DATA_DIR] [--since 2026-10-08T20:25:00]      (run on the server)

Joins `tracked-venues.json` (what the bot holds: every pool with its venue, kind, tokens, fee; the lending markets),
`feedtx-*.jsonl` (what the decoders read from the sequencer feed) and `chaintx-*.jsonl` (sampled block receipts).
For every successful chain transaction that emitted a swap or liquidity event on a tracked pool:
  - decoded: the feed recording has an intent of that class for the transaction;
  - pool-correct: for swaps, the decoded pool (by address, V4 stand-in, or token pair and fee) is that pool.
Reports per venue and, for the gaps, the destinations responsible; then events from tracked pools that the decoder's
event table does not recognise (by topic), and liquidation events on tracked lending markets.
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

tracked = json.load(open(os.path.join(d, "tracked-venues.json")))
pools = {p["address"].lower(): p for p in tracked["pools"]}
for p in tracked["pools"]:
    if p.get("v4_id"):
        pools["0x" + p["v4_id"][-40:].lower()] = p       # V4 pools are named by the stand-in address
print(f"tracked: {len(tracked['pools'])} pools on {len({p['dex'] for p in tracked['pools']})} venues; v4 manager {tracked.get('v4_manager')}")

feed = {r["hash"]: r for r in lines("feedtx-*.jsonl") if r.get("t_ms", 0) >= since_ms}
chain = {r["hash"]: r for r in lines("chaintx-*.jsonl")}
if since_ms and feed:
    first_block = min(r["block"] for r in feed.values())
    chain = {h: c for h, c in chain.items() if (c.get("block") or 0) >= first_block}

def event_pool(l):
    """The tracked pool an event belongs to (V4 events name the pool by id in topic 1)."""
    if l["kind"] in ("swap_v4", "modify_liquidity_v4"):
        return pools.get("0x" + l["topics"][1][-40:].lower())
    return pools.get(l["addr"].lower())

def pool_matches(sw, p):
    s = sw["pool"].lower()
    if s.startswith("v2:"):
        a, b = s[3:].split("/")
        return {a, b} == {p["token0"], p["token1"]} and p["kind"] in ("uniswap_v2", "camelot_v2")
    if "/" in s:
        a, b, fee = s.split("/")
        return {a, b} == {p["token0"], p["token1"]} and (p["fee"] == int(fee) or p["kind"] == "algebra_v3")
    return s == p["address"].lower() or s == ("0x" + (p.get("v4_id") or "")[-40:]).lower()

SWAP = ("swap_v2", "swap_v3", "swap_v3_pancake", "swap_v4", "swap_balancer")
LIQ = ("mint_v2", "burn_v2", "mint_v3", "burn_v3", "increase_liquidity_nft", "decrease_liquidity_nft", "modify_liquidity_v4")
venue = collections.defaultdict(lambda: collections.Counter())
gaps = collections.defaultdict(collections.Counter)
active = collections.defaultdict(set)
for h, c in chain.items():
    if c.get("status") is not True:
        continue
    f = feed.get(h) or {}
    seen = set()
    for l in c["logs"]:
        p = event_pool(l)
        if not p:
            continue
        key = (p["dex"], "swap" if l["kind"] in SWAP else "liquidity" if l["kind"] in LIQ else None)
        if key[1] is None or (key, p["address"]) in seen:
            continue
        seen.add((key, p["address"]))
        v = venue[p["dex"]]
        active[p["dex"]].add(p["address"])
        v[key[1] + " txs"] += 1
        sw, lq = f.get("swap"), f.get("liquidity")
        if key[1] == "swap":
            if sw:
                v["swap decoded"] += 1
                if pool_matches(sw, p):
                    v["swap decoded on this pool"] += 1
            else:
                gaps[p["dex"]][(c.get("to") or "-")[:12]] += 1
        else:
            if lq:
                v["liquidity decoded"] += 1
            else:
                gaps[p["dex"] + " (liquidity)"][(c.get("to") or "-")[:12]] += 1

print(f"\n{'venue':14s} {'pools':>5s} {'active':>6s} | {'swap txs':>8s} {'decoded':>8s} {'on pool':>8s} {'%':>6s} | {'liq txs':>7s} {'decoded':>8s}")
for dex in sorted({p["dex"] for p in tracked["pools"]}):
    v = venue[dex]; n = sum(1 for p in tracked["pools"] if p["dex"] == dex)
    st, sd, so = v["swap txs"], v["swap decoded"], v["swap decoded on this pool"]
    lt, ld = v["liquidity txs"], v["liquidity decoded"]
    print(f"{dex:14s} {n:5d} {len(active[dex]):6d} | {st:8d} {sd:8d} {so:8d} {100*so/st if st else 0:5.1f}% | {lt:7d} {ld:8d}")
print("\nlargest undecoded destinations per venue (to prefix, tracked-pool swap/liquidity transactions):")
for dex, cnt in sorted(gaps.items(), key=lambda kv: -sum(kv[1].values())):
    print(f"  {dex}: " + ", ".join(f"{t} {n}" for t, n in cnt.most_common(5)))

print("\nevents from tracked pools that the decoder's table does not recognise (by emitter venue and topic0):")
unk = collections.Counter()
for c in chain.values():
    for u in c.get("unknown_topics", []):
        p = pools.get(u["addr"].lower())
        unk[((p["dex"] if p else "v4 manager" if u["addr"].lower() == (tracked.get("v4_manager") or "").lower() else "?"), u["topic0"][:10])] += 1
LABELS = {
    "0x70935338": "Collect (fee collection; no price effect)",
    "0xbdbdb71d": "Flash (flash loan; no price effect)",
    "0x1b3d7edb": "ERC-6909 Transfer (PoolManager balance accounting)",
    "0x29ef05ca": "Donate (adds to LP fee growth; no price effect)",
    "0xdd466e67": "Initialize (new V4 pool)",
    "0xddf252ad": "ERC-20 Transfer (LP token of a V2 pair)",
    "0x8a89de70": "Algebra per-swap fee notification (now in the table)",
}
for (dex, t0), n in unk.most_common(14):
    print(f"  {n:6d}  {dex:14s} {t0}  {LABELS.get(t0, 'UNLABELLED')}")
if not unk:
    print("  none recorded yet (the recorder needs this build running)")

liq = collections.Counter(); liq_dec = collections.Counter()
for h, c in chain.items():
    if c.get("status") is not True: continue
    for l in c["logs"]:
        if l["kind"].startswith(("liquidation", "absorb")):
            liq[l["kind"]] += 1
            f = feed.get(h) or {}
            if f.get("liquidation"): liq_dec[l["kind"]] += 1
print("\nliquidation events on chain (all markets) and how many were decoded from the feed:", {k: f"{liq_dec[k]}/{v}" for k, v in liq.items()} or "none in this window")
print("lending markets tracked:", {k: (v if v else "-") for k, v in tracked.get("lending", {}).items()})
