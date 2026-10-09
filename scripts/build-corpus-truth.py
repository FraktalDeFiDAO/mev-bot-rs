#!/usr/bin/env python3
"""Fetch the on-chain outcome of every corpus transaction the decoder reads, for the offline accuracy tests.

  scripts/build-corpus-truth.py crates/seqscope-core/tests/fixtures/decoded_hashes.txt crates/seqscope-core/tests/fixtures/corpus_chain.json

For each hash: receipt status and the pools its swap / liquidity events touched (emitter address for V2/V3/Pancake/Algebra
pools; the 20-byte stand-in of the pool id for Uniswap V4, whose events come from the PoolManager). Uses the public Arbitrum
RPC; run it once after recording a corpus, commit the result.
"""
import json, sys, time, urllib.request

URL = "https://arb1.arbitrum.io/rpc"
V4_MANAGER = "0x360e68faccca8ca495c1b759fd9eee466db9fb32"
# topic0 -> (kind, v4?)
TOPICS = {
    "0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822": ("swap_v2", False),  # topic0 (event signature hash)
    "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67": ("swap_v3", False),  # topic0 (event signature hash)
    "0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83": ("swap_pancake", False),  # topic0 (event signature hash)
    "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f": ("swap_v4", True),  # topic0 (event signature hash)
    "0x7a53080ba414158be7ec69b987b5fb7d07dee101fe85488f0853ae16239d0bde": ("mint_v3", False),  # topic0 (event signature hash)
    "0x0c396cd989a39f4459b5fa1aed6a9a8dcdbc45908acfd67e028cd568da98982c": ("burn_v3", False),  # topic0 (event signature hash)
    "0xf208f4912782fd25c7f114ca3723a2d5dd6f3bcc3ac8db5af63baa85f711d5ec": ("modify_v4", True),  # topic0 (event signature hash)
    "0x3067048beee31b25b2f1681f88dac838c8bba36af25bfb2b7cf7473a5847e35f": ("increase_nft", False),  # topic0 (event signature hash)
    "0x26f6a048ee9138f2c0ce266f322cb99228e8d619ae2bff30c67f8dcf9d2377b4": ("decrease_nft", False),  # topic0 (event signature hash)
}

def rpc(method, params):
    for attempt in range(5):
        try:
            r = urllib.request.Request(URL, json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                                       {"Content-Type": "application/json", "User-Agent": "curl/8.5"})
            return json.load(urllib.request.urlopen(r, timeout=30)).get("result")
        except Exception as e:  # noqa: BLE001
            time.sleep(1 + attempt)
    return None

hashes = [h for h in open(sys.argv[1]).read().split() if h]
out = {}
for i, h in enumerate(hashes):
    rc = rpc("eth_getTransactionReceipt", [h])
    if rc is None:
        out[h] = {"status": None, "events": []}
        continue
    ev = []
    for l in rc["logs"]:
        t0 = l["topics"][0] if l["topics"] else ""
        if t0 in TOPICS:
            kind, v4 = TOPICS[t0]
            pool = "0x" + l["topics"][1][-40:] if v4 and len(l["topics"]) > 1 else l["address"].lower()
            ev.append({"kind": kind, "pool": pool.lower()})
    out[h] = {"status": int(rc["status"], 16), "events": ev}
    if i % 25 == 0:
        print(f"{i}/{len(hashes)}", file=sys.stderr)
json.dump(out, open(sys.argv[2], "w"), indent=1, sort_keys=True)
ok = sum(1 for v in out.values() if v["status"] == 1)
print(f"{len(out)} receipts, {ok} succeeded, {sum(len(v['events']) for v in out.values())} pool events", file=sys.stderr)
