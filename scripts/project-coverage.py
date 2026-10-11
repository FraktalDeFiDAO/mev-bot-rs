#!/usr/bin/env python3
"""Projected early-knowledge coverage against the chain, with the CURRENT decoders.

  scripts/project-coverage.py DATA_DIR        (DATA_DIR holds tracked-venues.json, feedtx-DATE.jsonl, chaintx-DATE.jsonl)

Takes every successful chain transaction that emitted a swap event on a tracked pool, finds those the bot had NOT learned
of before the block (neither decoded to that pool nor returned by the node simulation), fetches their calldata from
ARBITRUM_HTTP_URL (.env), runs `data/tmp/mev-decode calldata <to> <input> <value>` (build it with
`cargo build --release -p mev-decode`) and reports how many the current decoders would now name the right pool for.
Edit the two date strings below for another day. Read-only; needs a node that serves eth_getTransactionByHash.
"""
import json, re, subprocess, sys, urllib.request, collections, time
env = dict(l.strip().split("=",1) for l in open(".env") if re.match(r"^ARBITRUM_HTTP_URL=", l)); url = env["ARBITRUM_HTTP_URL"].strip('"')
D = sys.argv[1]
tracked = json.load(open(D + "/tracked-venues.json"))
pools = {}
for p in tracked["pools"]:
    pools[p["address"].lower()] = p
    if p.get("v4_id"): pools["0x" + p["v4_id"][-40:].lower()] = p
feed = {}
for l in open(D + "/feedtx-2026-10-10.jsonl"):
    try: e = json.loads(l)
    except: continue
    feed[e["hash"]] = e
chain = {}
for l in open(D + "/chaintx-2026-10-10.jsonl"):
    try: e = json.loads(l)
    except: continue
    chain[e["hash"]] = e
first_block = min(r["block"] for r in feed.values())
chain = {h: c for h, c in chain.items() if (c.get("block") or 0) >= first_block}
SWAP = ("swap_v2", "swap_v3", "swap_v3_pancake", "swap_v4")
def event_pool(l):
    if l["kind"] in ("swap_v4", "modify_liquidity_v4"): return pools.get("0x" + l["topics"][1][-40:].lower())
    return pools.get(l["addr"].lower())
def pool_matches(sw, p):
    s = sw["pool"].lower()
    if s.startswith("v2:"):
        a, b = s[3:].split("/"); return {a, b} == {p["token0"], p["token1"]} and p["kind"] in ("uniswap_v2", "camelot_v2")
    if "/" in s:
        a, b, fee = s.split("/"); return {a, b} == {p["token0"], p["token1"]} and (p["fee"] == int(fee) or p["kind"] == "algebra_v3")
    return s == p["address"].lower() or s == ("0x" + (p.get("v4_id") or "")[-40:]).lower()
total = 0; early_then = 0; blind = []   # (hash, pool)
for h, c in chain.items():
    if c.get("status") is not True: continue
    f = feed.get(h) or {}; seen = set()
    for l in c["logs"]:
        p = event_pool(l)
        if not p or l["kind"] not in SWAP or p["address"] in seen: continue
        seen.add(p["address"]); total += 1
        sw = f.get("swap")
        on_pool = bool(sw) and pool_matches(sw, p)
        stand_in = ("0x" + p["v4_id"][-40:]).lower() if p.get("v4_id") else p["address"].lower()
        by_presim = stand_in in {m.lower() for m in f.get("moved", [])}
        if on_pool or by_presim: early_then += 1
        else: blind.append((h, p))
print(f"swap events on tracked pools: {total}; early at record time: {early_then} ({100*early_then/total:.1f}%); blind then: {len(blind)}")
hashes = sorted({h for h, _ in blind})
def batch(hs):
    body = [{"jsonrpc":"2.0","id":i,"method":"eth_getTransactionByHash","params":[h]} for i,h in enumerate(hs)]
    for a in range(5):
        try:
            r = urllib.request.Request(url, json.dumps(body).encode(), {"content-type":"application/json"})
            out = json.load(urllib.request.urlopen(r, timeout=90)); return {o["id"]: o.get("result") for o in out}
        except Exception:
            time.sleep(2 + a)
    return {}
tx = {}
for i in range(0, len(hashes), 100):
    chunk = hashes[i:i+100]; res = batch(chunk)
    for j, h in enumerate(chunk):
        if res.get(j): tx[h] = res[j]
print(f"calldata fetched for {len(tx)} of {len(hashes)} blind transactions")
def intents(t):
    p = subprocess.run(["data/tmp/mev-decode","calldata",t["to"] or "0x"+"0"*40,t["input"],str(int(t.get("value","0x0"),16))],capture_output=True,text=True)
    try: d = json.loads(p.stdout)
    except: return []
    out = []
    for a in d.get("actions", []):
        dd = a.get("decoded")
        if dd and dd.get("kind") in ("swap", "pools"): out.append(dd)
    top = d.get("decoded")
    if top and top.get("kind") in ("swap", "pools"): out.append(top)
    return out
def matches(dd, p):
    if dd.get("kind") == "pools":
        return p["address"].lower() in [x.lower() for x in dd["pools"]] or ("0x" + (p.get("v4_id") or "")[-40:]).lower() in [x.lower() for x in dd["pools"]]
    pool = dd["pool"]; toks = {p["token0"].lower(), p["token1"].lower()}
    if "address" in pool: return pool["address"].lower() in (p["address"].lower(), ("0x" + (p.get("v4_id") or "")[-40:]).lower())
    if "fee" in pool: return {pool["token_a"].lower(), pool["token_b"].lower()} == toks and (p["fee"] == pool["fee"] or p["kind"] == "algebra_v3")
    if "v2_token_a" in pool: return {pool["v2_token_a"].lower(), pool["v2_token_b"].lower()} == toks and p["kind"] in ("uniswap_v2", "camelot_v2")
    if "any_token_a" in pool: return {pool["any_token_a"].lower(), pool["any_token_b"].lower()} == toks
    return False
# Aggregators whose calldata carries their route's pools literally: the engine holds back the watched ones it finds
# (mev_decode::POOL_SCAN_DESTINATIONS).
SCAN = {"0x09f94b5fc68e227c323a6fbae3bd98c97fd8c849", "0x111111125421ca6dc452d289314280a0f8842a65",
        "0x20cce25c88506e37d2812961802ce16a66ac5428", "0x6131b5fae19ea4f9d964eac0408e4408b66337b5",
        "0x9008d19f58aabd9ed0d60971565aa8510560ab41",
        "0x00000000b4ec312b95e8b1d4fd4eb0d1d2220975", "0xe6b82c6eccdfb2d6bc55bb11a61bae5ab2c3f807",
        "0x69bb29e2bf617bf74098d95054f72aee7999e4e3", "0xaa0000dc97008b87383600006100003b6b3c2020",
        "0xc6dd6c021cfbb284f0c0d788547b51fc90fcf663", "0x0000000071727de22e5e9d8baf0edac6f37da032",
        "0xdb9b1e94b5b69df7e401ddbede43491141047db3"}
import itertools
_by_pair = collections.defaultdict(list)
for _p in tracked["pools"]:
    _by_pair[frozenset((_p["token0"].lower()[2:], _p["token1"].lower()[2:]))].append(_p)
_tokens = {k for pr in _by_pair for k in pr}
def pair_hit(t, p):
    """feacf3f2: any pair of the listed tokens; fee-tier pools only if their tier appears in the calldata."""
    if not t or (t.get("to") or "").lower() != "0xfeacf3f2a6525b52661161f0d5ea6f62f5be6274": return False
    d = bytes.fromhex(t["input"][2:])
    tiers = {x for x in (100, 500, 2500, 3000, 10000) if x.to_bytes(3, "big") in d}
    toks = []
    for i in range(len(d) - 19):
        a = d[i:i+20].hex()
        if a in _tokens and a not in toks and len(toks) < 6: toks.append(a)
    for a, b in itertools.combinations(toks, 2):
        for q in _by_pair.get(frozenset((a, b)), []):
            if q["address"].lower() == p["address"].lower() and (q["kind"] not in ("uniswap_v3", "pancake_v3", "uniswap_v4") or q.get("fee") in tiers): return True
    return False
def literal_hit(t, p):
    if pair_hit(t, p): return True
    if not t or (t.get("to") or "").lower() not in SCAN: return False
    data = bytes.fromhex(t["input"][2:])
    return bytes.fromhex(p["address"][2:]) in data
cache = {h: intents(t) for h, t in tx.items()}
now_early = early_then; rescued = collections.Counter(); still = collections.Counter()
for h, p in blind:
    ok = any(matches(dd, p) for dd in cache.get(h, [])) or literal_hit(tx.get(h), p)
    if ok: now_early += 1; rescued[p["dex"]] += 1
    else: still[(tx.get(h, {}).get("to") or "-")[:12]] += 1
print(f"projected early with the current decoders: {now_early} ({100*now_early/total:.1f}%)")
print("rescued by venue:", dict(rescued)); print("still blind, top destinations:", still.most_common(10))
