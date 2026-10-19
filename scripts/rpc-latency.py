#!/usr/bin/env python3
"""RPC latency from this host: which endpoint should the bot's latency-critical reads use?

For each endpoint, over one kept-alive HTTPS connection (as the bot's client does):
  - connect: TCP + TLS handshake of the first request
  - eth_call at `pending` (a token's totalSupply): p50 / p95 / max of the warm requests
  - head: how many blocks the endpoint's `latest` trails the highest one seen in the same round
The sequencer (send-only) answers every other method with an error; its round trip still counts.

Keyed endpoints are read from environment variables named with --env, so keys never appear on the
command line; output shows hosts only, never paths or query strings (where keys live).

usage: scripts/rpc-latency.py --chain base [--env BASE_HTTP_URL --env BASE_HTTP_URL_2] [-n 50]
       scripts/rpc-latency.py --chain arbitrum --url https://arb1.arbitrum.io/rpc

Read-only; standard library only.
"""
import argparse, http.client, json, os, socket, ssl, statistics, sys, time, urllib.parse

CHAINS = {
    "base": {
        "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",  # USDC
        "sequencer": "https://mainnet-sequencer.base.org",
        "public": ["https://mainnet.base.org", "https://base.gateway.tenderly.co", "https://base-mainnet.public.blastapi.io",
                   "https://base.drpc.org", "https://base-rpc.publicnode.com", "https://base.api.pocket.network", "https://1rpc.io/base"],
    },
    "arbitrum": {
        "token": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",  # USDC
        "sequencer": "https://arb1-sequencer.arbitrum.io/rpc",
        "public": ["https://arb1.arbitrum.io/rpc", "https://arbitrum.drpc.org", "https://arbitrum-one-rpc.publicnode.com",
                   "https://arbitrum.gateway.tenderly.co", "https://1rpc.io/arb"],
    },
}


class Endpoint:
    def __init__(self, label, url):
        self.label, self.url = label, urllib.parse.urlsplit(url)
        self.conn, self.lat, self.errors, self.lag = None, [], 0, []
        self.connect_ms = None

    def post(self, method, params):
        """One JSON-RPC request on the kept-alive connection: (ms, result or None)."""
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        path = self.url.path or "/"
        if self.url.query:
            path += "?" + self.url.query
        for attempt in (0, 1):  # one reconnect if the server closed the idle connection
            if self.conn is None:
                t = time.perf_counter()
                self.conn = http.client.HTTPSConnection(self.url.netloc, timeout=10, context=ssl.create_default_context())
                self.conn.connect()
                if self.connect_ms is None:
                    self.connect_ms = (time.perf_counter() - t) * 1000
            try:
                t = time.perf_counter()
                self.conn.request("POST", path, body, {"Content-Type": "application/json", "User-Agent": "rpc-latency"})
                resp = self.conn.getresponse()
                raw = resp.read()
                ms = (time.perf_counter() - t) * 1000
            except (OSError, http.client.HTTPException):
                self.conn.close()
                self.conn = None
                if attempt:
                    raise
                continue
            try:
                j = json.loads(raw)
            except ValueError:
                return ms, None
            return ms, (j.get("result") if isinstance(j, dict) else None)


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--chain", choices=CHAINS, required=True)
    ap.add_argument("--env", action="append", default=[], help="environment variable holding a (keyed) RPC URL")
    ap.add_argument("--url", action="append", default=[], help="extra keyless RPC URL")
    ap.add_argument("--no-public", action="store_true", help="skip the built-in public endpoints")
    ap.add_argument("-n", type=int, default=40, help="warm requests per endpoint")
    ap.add_argument("--gap", type=float, default=0.25, help="seconds between rounds (stay under free rate limits)")
    a = ap.parse_args()
    c = CHAINS[a.chain]

    eps = []
    for name in a.env:
        v = os.environ.get(name, "").strip()
        if not v:
            print(f"{name}: not set, skipped", file=sys.stderr)
            continue
        eps.append(Endpoint(f"${name} ({urllib.parse.urlsplit(v).hostname})", v))
    for u in a.url + ([] if a.no_public else c["public"]):
        eps.append(Endpoint(urllib.parse.urlsplit(u).hostname, u))
    seq = Endpoint(f"sequencer ({urllib.parse.urlsplit(c['sequencer']).hostname})", c["sequencer"])

    call = [{"to": c["token"], "data": "0x18160ddd"}, "pending"]
    for e in eps + [seq]:  # warm up: connect once, outside the timed rounds
        try:
            e.post("eth_chainId", [])
        except Exception:
            e.errors += 1
    print(f"{a.chain}: {len(eps)} endpoints, {a.n} rounds from {socket.gethostname()}", file=sys.stderr)
    for _ in range(a.n):
        heads = {}
        for e in eps:
            try:
                ms, r = e.post("eth_call", call)
                if r is None:
                    e.errors += 1
                else:
                    e.lat.append(ms)
                _, h = e.post("eth_blockNumber", [])
                if h:
                    heads[e] = int(h, 16)
            except Exception:
                e.errors += 1
        try:
            seq.lat.append(seq.post("eth_chainId", [])[0])
        except Exception:
            seq.errors += 1
        top = max(heads.values(), default=0)
        for e, h in heads.items():
            e.lag.append(top - h)
        time.sleep(a.gap)

    print(f"{'endpoint':52} {'connect':>8} {'p50':>7} {'p95':>7} {'max':>7} {'lag':>5} {'err':>4}")
    for e in sorted(eps, key=lambda e: pct(e.lat, 50) if e.lat else 1e9) + [seq]:
        conn = f"{e.connect_ms:.0f}ms" if e.connect_ms is not None else "-"
        ms = [f"{pct(e.lat, 50):.1f}ms", f"{pct(e.lat, 95):.1f}ms", f"{max(e.lat):.1f}ms"] if e.lat else ["-"] * 3
        lag = f"{statistics.mean(e.lag):.1f}" if e.lag else "-"
        print(f"{e.label[:52]:52} {conn:>8} {ms[0]:>7} {ms[1]:>7} {ms[2]:>7} {lag:>5} {e.errors:>4}")
    print("ms per warm request; lag = mean blocks behind the freshest endpoint each round; "
          "sequencer row: round trip of a rejected eth_chainId (send-only endpoint)")


if __name__ == "__main__":
    main()
