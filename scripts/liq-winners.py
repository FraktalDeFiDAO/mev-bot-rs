#!/usr/bin/env python3
"""How liquidation winners land on an OP Stack chain (Base), to compare with what our bot sends.

For every Morpho Blue `Liquidate` and Compound v2 / Moonwell `LiquidateBorrow` in the last N days
(up to --max per venue), from the winner's transaction, its receipt and its block:

  - trigger: a Chainlink `AnswerUpdated` earlier in the same block (same-block backrun), in the
    previous block (next-block reaction), or neither (older update, other oracle, interest)
  - priority fee paid per gas (effective gas price - base fee) and its percentile in the block
  - position: transaction index, and the earliest flashblock the winner can be in (gas before it /
    one tenth of the block gas limit; Base builds each 2 s block in ten 200 ms flashblocks with
    cumulative gas budgets of 1/10, 2/10, ... of the limit)
  - how it was sent: transaction type, gas limit vs gas used, the contract called
  - rivals: transactions in the same block whose calldata names the same borrower, and whether
    they reverted, landed before or after the winner, and what they paid

usage: scripts/liq-winners.py --rpc https://base.api.pocket.network --days 14 \\
           --morpho 0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb --compound-markets 0x..,0x.. [--out f.json]

Read-only; public RPC; standard library only.
"""
import argparse
import json
import statistics as st
import sys
import time
import urllib.request
from collections import Counter

MORPHO_LIQUIDATE = "0xa4946ede45d0c6f06a0f5ce92c9ad3b4751452d2fe0e25010783bcab57a67e41"
COMPOUND_LIQUIDATE_BORROW = "0x298637f684da70674f26509b10f07ec2fbc77a335ab1e7d6215a4b2484d8bb52"
ANSWER_UPDATED = "0x0559884fd3a460db3073b7fc896cc77986f16e378210ded43186175bf646fc5f"
LOG_SPAN = 49_000
GWEI = 1e9


class Rpc:
    def __init__(self, url):
        self.url, self.calls = url, 0

    def call(self, method, params, tries=4):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        for k in range(tries):
            self.calls += 1
            try:
                req = urllib.request.Request(self.url, body, {"content-type": "application/json", "user-agent": "liq-winners/1"})
                with urllib.request.urlopen(req, timeout=60) as r:
                    d = json.load(r)
                if "error" in d:
                    raise RuntimeError(f"{method}: {d['error']}")
                return d["result"]
            except Exception:  # noqa: BLE001 - public endpoints fail transiently
                if k == tries - 1:
                    raise
                time.sleep(1 + 2 * k)


def h(x):
    return int(x, 16) if isinstance(x, str) else (x or 0)


def liquidations(rpc, lo, hi, morpho, markets):
    out = []
    emitters = ([morpho] if morpho else []) + markets
    logs, a, span = [], lo, LOG_SPAN
    while a <= hi:
        b = min(a + span - 1, hi)
        try:
            logs += rpc.call("eth_getLogs", [{"fromBlock": hex(a), "toBlock": hex(b), "address": emitters,
                                              "topics": [[MORPHO_LIQUIDATE, COMPOUND_LIQUIDATE_BORROW]]}], tries=2)
        except Exception:  # noqa: BLE001 - timeouts on wide ranges: halve and retry
            if span <= 500:
                raise
            span //= 2
            continue
        a = b + 1
    for l in logs:
        if l["topics"][0] == MORPHO_LIQUIDATE:
            borrower, venue = l["topics"][3][-40:], "morpho-blue"
        else:
            borrower, venue = l["data"][2 + 64 + 24:2 + 128], "moonwell"
        out.append({"venue": venue, "borrower": borrower.lower(), "block": h(l["blockNumber"]),
                    "tx": l["transactionHash"], "index": h(l["transactionIndex"])})
    return out


def analyse(rpc, q):
    block = rpc.call("eth_getBlockByNumber", [hex(q["block"]), True])
    receipts = rpc.call("eth_getBlockReceipts", [hex(q["block"])])
    prev = rpc.call("eth_getBlockReceipts", [hex(q["block"] - 1)])
    base_fee = h(block.get("baseFeePerGas"))
    gas_limit = h(block["gasLimit"])
    txs = block["transactions"]
    rc = {r["transactionHash"]: r for r in receipts}
    w = next(t for t in txs if t["hash"] == q["tx"])
    wr = rc[q["tx"]]
    i = h(w["transactionIndex"])
    prio = lambda r: max(h(r["effectiveGasPrice"]) - base_fee, 0)  # noqa: E731
    gas_before = h(wr["cumulativeGasUsed"]) - h(wr["gasUsed"])
    same_block_update = any(l["topics"] and l["topics"][0] == ANSWER_UPDATED
                            for r in receipts if h(r["transactionIndex"]) < i for l in r["logs"])
    prev_update = any(l["topics"] and l["topics"][0] == ANSWER_UPDATED for r in prev for l in r["logs"])
    user_prios = sorted(prio(r) for r in receipts if h(r.get("type", "0x0")) != 0x7E)
    wp = prio(wr)
    rivals = []
    for t in txs:
        if t["hash"] == q["tx"] or q["borrower"] not in (t.get("input") or "").lower():
            continue
        r = rc[t["hash"]]
        rivals.append({"index": h(t["transactionIndex"]), "ok": h(r["status"]) == 1, "from": t["from"],
                       "to": t["to"], "prio_gwei": prio(r) / GWEI, "gas_used": h(r["gasUsed"])})
    return {
        **q,
        "from": w["from"], "to": w["to"], "selector": (w.get("input") or "0x")[:10], "type": h(w["type"]),
        "prio_gwei": wp / GWEI,
        "prio_pct": sum(p <= wp for p in user_prios) / max(len(user_prios), 1),
        "fee_usd_gas": h(wr["gasUsed"]) * h(wr["effectiveGasPrice"]),
        "gas_used": h(wr["gasUsed"]), "gas_limit": h(w["gas"]),
        "block_txs": len(txs), "flashblock_min": gas_before * 10 // gas_limit + 1 if gas_limit else None,
        "trigger": "same_block" if same_block_update else ("prev_block" if prev_update else "older_or_other"),
        "rivals": rivals,
    }


def pct(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(q * (len(v) - 1)))] if v else None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rpc", required=True, help="getLogs endpoint (wide ranges)")
    ap.add_argument("--detail-rpc", help="blocks, receipts (default: --rpc)")
    ap.add_argument("--days", type=float, default=14)
    ap.add_argument("--morpho", default="")
    ap.add_argument("--compound-markets", default="")
    ap.add_argument("--max", type=int, default=250, help="most recent liquidations analysed per venue")
    ap.add_argument("--block-time", type=float, default=2.0)
    ap.add_argument("--out")
    a = ap.parse_args()
    rpc = Rpc(a.rpc)
    detail = Rpc(a.detail_rpc or a.rpc)
    head = h(rpc.call("eth_blockNumber", []))
    lo = head - int(a.days * 86400 / a.block_time)
    liqs = liquidations(rpc, lo, head, a.morpho, [m for m in a.compound_markets.split(",") if m])
    rows = []
    for venue in sorted({q["venue"] for q in liqs}):
        mine = sorted((q for q in liqs if q["venue"] == venue), key=lambda q: -q["block"])[: a.max]
        for n, q in enumerate(mine):
            try:
                rows.append(analyse(detail, q))
            except Exception as e:  # noqa: BLE001
                print(f"skip {q['tx']}: {str(e)[:100]}", file=sys.stderr)
            if n % 50 == 49:
                print(f"{venue}: {n + 1}/{len(mine)} ({detail.calls} calls)", file=sys.stderr)
    if a.out:
        json.dump(rows, open(a.out, "w"), indent=1)
    print(f"blocks {lo}-{head} ({a.days} days); liquidations found {len(liqs)}, analysed {len(rows)}; {rpc.calls + detail.calls} RPC calls")
    for venue in sorted({r["venue"] for r in rows}):
        R = [r for r in rows if r["venue"] == venue]
        n = len(R)
        print(f"\n== {venue}: {n} liquidations, {len({r['from'] for r in R})} sender wallets, {len({r['to'] for r in R})} contracts")
        print(f"trigger: {dict(Counter(r['trigger'] for r in R))}")
        print(f"priority fee gwei p10/p50/p90/max: {[round(pct([r['prio_gwei'] for r in R], q), 6) for q in (.1, .5, .9, 1)]}")
        print(f"priority-fee percentile in its block p50/p90: {[round(pct([r['prio_pct'] for r in R], q), 2) for q in (.5, .9)]}")
        print(f"tx index p10/p50/p90: {[pct([r['index'] for r in R], q) for q in (.1, .5, .9)]}; "
              f"earliest flashblock p50/p90: {[pct([r['flashblock_min'] for r in R], q) for q in (.5, .9)]}")
        print(f"tx types: {dict(Counter(r['type'] for r in R))}; selectors: {Counter(r['selector'] for r in R).most_common(5)}")
        print(f"gas limit / used p50: {round(pct([r['gas_limit'] / max(r['gas_used'], 1) for r in R], .5), 2)}")
        with_rivals = [r for r in R if r["rivals"]]
        failed = [x for r in R for x in r["rivals"] if not x["ok"]]
        print(f"blocks with rival txs naming the borrower: {len(with_rivals)}/{n}; rival txs {sum(len(r['rivals']) for r in R)} "
              f"(reverted {len(failed)}, before winner {sum(x['index'] < r['index'] for r in R for x in r['rivals'])})")
        if failed:
            print(f"reverted rivals' priority fee gwei p50: {round(pct([x['prio_gwei'] for x in failed], .5), 6)}")
        top = Counter(r["from"] for r in R).most_common(5)
        print(f"top senders: {[(s[:10], c) for s, c in top]}")


if __name__ == "__main__":
    main()
