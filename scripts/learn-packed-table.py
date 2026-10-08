#!/usr/bin/env python3
"""Fit the packed calldata of a swap bot from transaction traces and print its pool table as Rust.

  scripts/learn-packed-table.py TRACES.json --rpc URL

TRACES.json: [[tx_hash, input_hex, [ {pool, zfo, amount, limit}, ... ]], ...] as written by the tracing helper, i.e.
the contract's calldata plus the first `UniswapV3Pool.swap` (selector 0x128acb08) call found in its callTracer trace.

Layout fitted on mev-protocol-unidentified router D (selectors with a 32-byte argument):
  bytes 0-1  pool index into the contract's table (the selector chooses the table)
  byte  2    left shift, in bits, applied to the amount
  bytes 3-10 amount mantissa (uint64)
  bytes 11-30 sqrtPriceLimitX96 (uint160)
  byte  31   direction flag, bit 0 = zeroForOne
Prints the fit accuracy per selector, the (selector, index) -> pool table with token0/token1, and any index that mapped
to more than one pool (a table that is not one-to-one means the layout is wrong for that selector).
"""
import collections, json, sys, urllib.request

args = sys.argv[1:]
rows = json.load(open(args[0]))
rpc_url = args[args.index("--rpc") + 1]

def call(to, data):
    q = urllib.request.Request(rpc_url, json.dumps({"jsonrpc": "2.0", "id": 1, "method": "eth_call", "params": [{"to": to, "data": data}, "latest"]}).encode(), {"Content-Type": "application/json", "User-Agent": "x"})
    return json.load(urllib.request.urlopen(q, timeout=30)).get("result", "")

fit = collections.defaultdict(collections.Counter)
table = collections.defaultdict(collections.Counter)
for h, inp, swaps in rows:
    sel, arg = inp[2:10], inp[10:]
    if len(arg) != 64 or not swaps:
        continue
    s = swaps[0]
    shift, mant = int(arg[4:6], 16), int(arg[6:22], 16)
    limit, zfo = int(arg[22:62], 16), int(arg[62:64], 16) & 1
    f = fit[sel]
    f["n"] += 1
    f["amount"] += (mant << shift) == abs(s["amount"])
    f["zfo"] += zfo == s["zfo"]
    f["limit"] += limit == s["limit"]
    table[(sel, int(arg[0:4], 16))][s["pool"].lower()] += 1

for sel, f in sorted(fit.items()):
    print(f"// selector {sel}: n={f['n']} amount ok {f['amount']} zeroForOne ok {f['zfo']} limit ok {f['limit']}")
print("\n// (selector, index, pool, token0, token1, observations)")
tok = {}
for (sel, idx), c in sorted(table.items()):
    pool, n = c.most_common(1)[0]
    if len(c) > 1:
        print(f"// WARNING selector {sel} index {idx:#06x} maps to several pools: {dict(c)}")
    if pool not in tok:
        tok[pool] = ("0x" + call(pool, "0x0dfe1681")[-40:], "0x" + call(pool, "0xd21220a7")[-40:])
    t0, t1 = tok[pool]
    print(f'    ([{", ".join("0x" + sel[i:i+2] for i in range(0, 8, 2))}], {idx:#06x}, address!("{pool[2:]}"), address!("{t0[2:]}"), address!("{t1[2:]}")), // {n}')
