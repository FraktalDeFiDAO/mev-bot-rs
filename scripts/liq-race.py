#!/usr/bin/env python3
"""Shadow liquidation race report: did the bot see each real liquidation before the winner took it?

Joins the bot's liquidation events (`liq_*` in data/<chain>/events-*.jsonl, one per planning
attempt, keyed by borrower) with the liquidations that actually happened on chain in the same
time window (Morpho Blue `Liquidate`, Compound v2 / Moonwell `LiquidateBorrow`).

For each real liquidation:
  seen_first  the bot flagged the borrower before the liquidation landed (lead = seconds ahead),
              with what it concluded (planned / no route / dust / ...)
  seen_after  first flagged only after it landed (the winner was faster than our polling)
  missed      never flagged in the window

usage:
  scripts/liq-race.py --events data/base --rpc https://base.api.pocket.network \\
      --morpho 0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb --compound-markets 0x..,0x..

Read-only; public RPC; standard library only.
"""
import argparse
import datetime as dt
import glob
import json
import os
import re
import sys
import urllib.request

MORPHO_LIQUIDATE = "0xa4946ede45d0c6f06a0f5ce92c9ad3b4751452d2fe0e25010783bcab57a67e41"
COMPOUND_LIQUIDATE_BORROW = "0x298637f684da70674f26509b10f07ec2fbc77a335ab1e7d6215a4b2484d8bb52"
LOG_SPAN = 49_000  # Pocket's getLogs cap is 50,000 blocks


class Rpc:
    def __init__(self, url):
        self.url = url

    def call(self, method, params):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = urllib.request.Request(self.url, body, {"content-type": "application/json", "user-agent": "liq-race/1"})
        with urllib.request.urlopen(req, timeout=60) as r:
            d = json.load(r)
        if "error" in d:
            raise RuntimeError(f"{method}: {d['error']}")
        return d["result"]

    def block_ts(self, n):
        return int(self.call("eth_getBlockByNumber", [hex(n), False])["timestamp"], 16)

    def block_at(self, ts, head):
        """Highest block with timestamp <= ts. Estimated from the head's timestamp and the recent
        block time, then corrected (endpoints prune old headers, so no search from genesis)."""
        head_ts = self.block_ts(head)
        bt = max((head_ts - self.block_ts(head - 1000)) / 1000, 0.01)
        n = min(head, max(0, head - int((head_ts - ts) / bt)))
        for _ in range(6):
            err = ts - self.block_ts(n)
            if 0 <= err < bt:
                break
            n = min(head, max(0, n + int(err / bt)))
        while n > 0 and self.block_ts(n) > ts:
            n -= 1
        return n


def parse_ts(s):
    """RFC 3339 with up to nanosecond fractions (Python parses at most microseconds)."""
    m = re.match(r"(.*T\d\d:\d\d:\d\d)(\.\d+)?(Z|[+-]\d\d:\d\d)?$", s)
    tz = (m.group(3) or "+00:00").replace("Z", "+00:00")
    return dt.datetime.fromisoformat(m.group(1) + tz).timestamp() + float(m.group(2) or 0)


def load_events(path):
    """borrower (lower-case) -> list of (ts, kind, protocol, bonus_usd)."""
    seen = {}
    t0, t1 = None, None
    for f in sorted(glob.glob(os.path.join(path, "events-*.jsonl"))):
        for line in open(f):
            try:
                e = json.loads(line)
            except ValueError:
                continue
            ts = parse_ts(e["ts"])
            t0 = ts if t0 is None else min(t0, ts)
            t1 = ts if t1 is None else max(t1, ts)
            if not e.get("kind", "").startswith("liq_"):
                continue
            d = e.get("data") or {}
            d = d.get("liq", d)  # liq_sim_revert nests the plan
            acct = str(d.get("account", "")).split("@")[0].lower()
            if acct:
                seen.setdefault(acct, []).append((ts, e["kind"], d.get("protocol"), d.get("bonus_usd")))
    return seen, t0, t1


def fetch_liquidations(rpc, lo, hi, morpho, markets):
    out = []
    emitters = ([morpho] if morpho else []) + markets
    for a in range(lo, hi + 1, LOG_SPAN):
        b = min(a + LOG_SPAN - 1, hi)
        logs = rpc.call("eth_getLogs", [{"fromBlock": hex(a), "toBlock": hex(b), "address": emitters,
                                          "topics": [[MORPHO_LIQUIDATE, COMPOUND_LIQUIDATE_BORROW]]}])
        for l in logs:
            t0 = l["topics"][0]
            if t0 == MORPHO_LIQUIDATE:
                borrower, venue = "0x" + l["topics"][3][-40:], "morpho-blue"
            else:
                borrower, venue = "0x" + l["data"][2 + 64 + 24:2 + 128], "moonwell"
            out.append({"venue": venue, "borrower": borrower.lower(), "block": int(l["blockNumber"], 16),
                        "tx": l["transactionHash"], "index": int(l["transactionIndex"], 16)})
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--events", required=True, help="the bot's data/<chain> directory")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--morpho", default="")
    ap.add_argument("--compound-markets", default="", help="comma-separated mToken/cToken markets")
    args = ap.parse_args()
    seen, t0, t1 = load_events(args.events)
    if t0 is None:
        sys.exit("no events found")
    rpc = Rpc(args.rpc)
    head = int(rpc.call("eth_blockNumber", []), 16)
    lo, hi = rpc.block_at(int(t0), head) + 1, rpc.block_at(int(t1), head)
    markets = [m for m in args.compound_markets.split(",") if m]
    liqs = fetch_liquidations(rpc, lo, hi, args.morpho, markets)
    rows, counts = [], {"seen_first": 0, "seen_after": 0, "missed": 0}
    for q in liqs:
        q["ts"] = rpc.block_ts(q["block"])
        q["sender"] = rpc.call("eth_getTransactionByHash", [q["tx"]])["from"]
        ours = sorted(seen.get(q["borrower"], []))
        before = [s for s in ours if s[0] <= q["ts"]]
        if before:
            status, lead = "seen_first", q["ts"] - before[0][0]
            kinds = sorted({s[1] for s in before})
        elif ours:
            status, lead, kinds = "seen_after", q["ts"] - ours[0][0], sorted({s[1] for s in ours})
        else:
            status, lead, kinds = "missed", None, []
        counts[status] += 1
        rows.append((q, status, lead, kinds))
    span_h = (t1 - t0) / 3600
    print(f"window {dt.datetime.fromtimestamp(t0, dt.UTC):%F %T} .. {dt.datetime.fromtimestamp(t1, dt.UTC):%T} UTC "
          f"({span_h:.1f} h), blocks {lo}-{hi}; real liquidations: {len(liqs)}; {counts}")
    print(f"{'venue':12} {'borrower':12} {'block':>9} {'idx':>4} {'winner':12} {'status':11} {'lead_s':>7}  our view")
    for q, status, lead, kinds in sorted(rows, key=lambda r: r[0]["block"]):
        print(f"{q['venue']:12} {q['borrower'][:12]} {q['block']:>9} {q['index']:>4} {q['sender'][:12]} "
              f"{status:11} {'' if lead is None else round(lead):>7}  {','.join(kinds)}")


if __name__ == "__main__":
    main()
