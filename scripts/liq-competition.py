#!/usr/bin/env python3
"""Who wins liquidations, how much, how fast? Cross-chain competition scan for lending venues.

  scripts/liq-competition.py scan     [--days 90] [--chains a,b] [--venues x,y]   liquidation events per venue
  scripts/liq-competition.py validate [--chains a,b] [--venues x,y]               recount sampled ranges in small chunks
  scripts/liq-competition.py senders  [--min-bonus 1000]                          add each event's tx sender (operator)
  scripts/liq-competition.py summary                                              ranked table (summary.json, summary.md)
  scripts/liq-competition.py timing   --venues x,y [--sample 100]                 fees paid and delay after the oracle update

Read-only: public RPCs from chainlist.org (never the bot's keyed endpoints), DefiLlama current prices. Data goes to
data/liq-competition/ (`--out DIR` to change). Each venue is scanned backwards from the head with an adaptive getLogs
range and a call budget; `validate` re-counts random ranges with 500-block queries to catch endpoints that drop logs.

Venue addresses (checked 2026-10-15): Aave V3 POOL from bgd-labs/aave-address-book; Morpho Blue from morpho-org/sdks
(morpho-ts addresses.ts); Moonwell UNITROLLER from moonwell-fi/moonwell-contracts-v2 chains/<id>.json; Venus Core
Unitroller from VenusProtocol/venus-protocol deployments/bscmainnet_addresses.json; HyperLend POOL from
hyperlendx/hyperlend-sdk-ts; HypurrFi pool from its PoolAddressesProvider (getMarketId() = "HypurrFi Mainnet").
HyperLend and HypurrFi were verified on-chain: provider.getPool() == pool and pool.ADDRESSES_PROVIDER() == provider.

Caveats: bonuses use today's prices; "contracts" counts the liquidating contract (Venus routes every liquidation
through its Liquidator, so use "senders"); one operator can run several contracts and wallets.
"""
import json, os, random, statistics, sys, threading, time, urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor

AAVE_LIQ = "0xe413a321e8681d831f4dbccbca790d2952b56f977908e45be37335533e005286"     # LiquidationCall
MORPHO_LIQ = "0xa4946ede45d0c6f06a0f5ce92c9ad3b4751452d2fe0e25010783bcab57a67e41"   # Liquidate (Morpho Blue)
COMP_LIQ = "0x298637f684da70674f26509b10f07ec2fbc77a335ab1e7d6215a4b2484d8bb52"     # LiquidateBorrow (Compound v2)
ANSWER_UPDATED = "0x0559884fd3a460db3073b7fc896cc77986f16e378210ded43186175bf646fc5f"
TOPIC = {"aave": AAVE_LIQ, "morpho": MORPHO_LIQ, "compound": COMP_LIQ}
SEL = {"idToMarketParams": "0x2c3c9157", "getAllMarkets": "0xb0772d0b", "underlying": "0x6f307dc3",
       "exchangeRateStored": "0x182df0f5", "ADDRESSES_PROVIDER": "0x0542975c", "getPriceOracle": "0xfca513a8",
       "getSourceOfAsset": "0x92bf2be0", "aggregator": "0x245a7bfc", "BASE_FEED_1": "0xf50a4718",
       "BASE_FEED_2": "0xdc53858c", "QUOTE_FEED_1": "0x56095e11", "QUOTE_FEED_2": "0xacfbd39e"}

# chain: chain id, DefiLlama chain key, native coin (coingecko id), wrapped native (cToken markets without underlying())
CHAINS = {
    "arbitrum": (42161, "arbitrum", "ethereum", "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
    "base": (8453, "base", "ethereum", "0x4200000000000000000000000000000000000006"),
    "optimism": (10, "optimism", "ethereum", "0x4200000000000000000000000000000000000006"),
    "polygon": (137, "polygon", "polygon-ecosystem-token", None),
    "avalanche": (43114, "avax", "avalanche-2", None),
    "bnb": (56, "bsc", "binancecoin", "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"),
    "gnosis": (100, "xdai", "xdai", None),
    "linea": (59144, "linea", "ethereum", None),
    "scroll": (534352, "scroll", "ethereum", None),
    "sonic": (146, "sonic", "sonic-3", None),
    "mantle": (5000, "mantle", "mantle", None),
    "celo": (42220, "celo", "celo", None),
    "metis": (1088, "metis", "metis-token", None),
    "soneium": (1868, "soneium", "ethereum", None),
    "plasma": (9745, "plasma", "plasma", None),
    "ink": (57073, "ink", "ethereum", None),
    "monad": (143, "monad", "monad", None),
    "megaeth": (4326, "megaeth", "ethereum", None),
    "xlayer": (196, "xlayer", "okb", None),
    "unichain": (130, "unichain", "ethereum", None),
    "hyperevm": (999, "hyperliquid", "hyperliquid", None),
    "katana": (747474, "katana", "ethereum", None),
    "worldchain": (480, "wc", "ethereum", None),
}
AAVE = {"arbitrum": "0x794a61358D6845594F94dc1DB02A252b5b4814aD", "base": "0xA238Dd80C259a72e81d7e4664a9801593F98d1c5",
        "optimism": "0x794a61358D6845594F94dc1DB02A252b5b4814aD", "polygon": "0x794a61358D6845594F94dc1DB02A252b5b4814aD",
        "avalanche": "0x794a61358D6845594F94dc1DB02A252b5b4814aD", "bnb": "0x6807dc923806fE8Fd134338EABCA509979a7e0cB",
        "gnosis": "0xb50201558B00496A145fE76f7424749556E326D8", "linea": "0xc47b8C00b0f69a36fa203Ffeac0334874574a8Ac",
        "scroll": "0x11fCfe756c05AD438e312a7fd934381537D3cFfe", "sonic": "0x5362dBb1e601abF3a4c14c22ffEdA64042E5eAA3",
        "mantle": "0x458F293454fE0d67EC0655f3672301301DD51422", "celo": "0x3E59A31363E2ad014dcbc521c4a0d5757d9f3402",
        "metis": "0x90df02551bB792286e8D4f13E0e357b4Bf1D6a57", "soneium": "0xDd3d7A7d03D9fD9ef45f3E587287922eF65CA38B",
        "plasma": "0x925a2A7214Ed92428B5b1B090F80b25700095e12", "ink": "0x2816cf15F6d2A220E789aA011D5EE4eB6c47FEbA",
        "monad": "0x69a5F9AD4f96ebf0a0C792dD42a01cC5C0102fef", "megaeth": "0x7e324AbC5De01d112AfC03a584966ff199741C28",
        "xlayer": "0xE3F3Caefdd7180F884c01E57f65Df979Af84f116"}
MORPHO = {"arbitrum": "0x6c247b1F6182318877311737BaC0844bAa518F5e", "base": "0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb",
          "optimism": "0xce95AfbB8EA029495c66020883F87aaE8864AF92", "polygon": "0x1bF0c2541F820E775182832f06c0B7Fc27A25f67",
          "bnb": "0x01b0Bd309AA75547f7a37Ad7B1219A898E67a83a", "linea": "0x6B0D716aC0A45536172308e08fC2C40387262c9F",
          "scroll": "0x2d012EdbAdc37eDc2BC62791B666f9193FDF5a55", "sonic": "0xd6c916eB7542D0Ad3f18AEd0FCBD50C582cfa95f",
          "celo": "0xd24ECdD8C1e0E57a4E26B1a7bbeAa3e95466A569", "soneium": "0xE75Fc5eA6e74B824954349Ca351eb4e671ADA53a",
          "ink": "0x857f3EefE8cbda3Bc49367C996cd664A880d3042", "monad": "0xD5D960E8C380B724a48AC59E2DfF1b2CB4a1eAee",
          "megaeth": "0x18120312A7cf44DcfEc6dCe5632a431579ED9100", "unichain": "0x8f5ae9CddB9f68de460C77730b018Ae7E04a140A",
          "hyperevm": "0x68e37dE8d93d3496ae143F2E900490f6280C57cD", "katana": "0xD50F2DffFd62f94Ee4AEd9ca05C61d0753268aBc",
          "worldchain": "0xE741BC7c34758b4caE05062794E8Ae24978AF432"}
# (chain, venue, kind, address): Aave V3 forks use kind "aave", Compound v2 forks give their comptroller
VENUES = ([(c, "aave-v3", "aave", a) for c, a in AAVE.items()] + [(c, "morpho-blue", "morpho", a) for c, a in MORPHO.items()] + [
    ("base", "moonwell", "compound", "0xfBb21d0380beE3312B33c4353c8936a0F13EF26C"),
    ("optimism", "moonwell", "compound", "0xCa889f40aae37FFf165BccF69aeF1E82b5C511B9"),
    ("bnb", "venus-core", "compound", "0xfD36E2c2a6789Db23113685031d7F16329158384"),
    ("hyperevm", "hyperlend", "aave", "0x00A89d7a5A02160f20150EbEA7a2b5E4879A1A8b"),
    ("hyperevm", "hypurrfi", "aave", "0xceCcE0EB9DD2Ef7996e01e25DD70e461F918A14b"),
])

CHAINLIST = "https://chainlist.org/rpcs.json"
PREFERRED = ("publicnode.com", "drpc.org", "llamarpc", "1rpc.io", "ankr.com", "blastapi", "tenderly", "blockpi")
CALL_BUDGET = 8000
_lock = threading.Lock()


def log(*a):
    with _lock:
        print(*a, file=sys.stderr, flush=True)


def post(url, body, timeout=30):
    req = urllib.request.Request(url, json.dumps(body).encode(), {"Content-Type": "application/json", "User-Agent": "liq-competition/1"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def get(url, timeout=60):
    req = urllib.request.Request(url, headers={"User-Agent": "liq-competition/1"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


class Rpc:
    def __init__(self, urls):
        self.urls, self.i, self.calls = urls, 0, 0

    def call(self, method, params, tries=None):
        last = None
        for _ in range(tries or 2 * len(self.urls)):
            url = self.urls[self.i % len(self.urls)]
            self.calls += 1
            try:
                j = post(url, {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                if "error" in j:
                    raise RuntimeError(str(j["error"])[:160])
                return j["result"]
            except Exception as e:  # noqa: BLE001 - rotate to the next endpoint on any failure
                last = e
                self.i += 1
                time.sleep(0.3)
        raise RuntimeError(f"{method} failed on all endpoints: {last}")

    def batch(self, calls):
        """[(method, params)] -> results (None on failure); JSON-RPC batches of 25, single calls if unsupported."""
        out = [None] * len(calls)
        for s in range(0, len(calls), 25):
            part = calls[s:s + 25]
            body = [{"jsonrpc": "2.0", "id": k, "method": m, "params": p} for k, (m, p) in enumerate(part)]
            try:
                url = self.urls[self.i % len(self.urls)]
                self.calls += 1
                res = post(url, body, 60)
                if not isinstance(res, list):
                    raise RuntimeError("no batch support")
                for r in res:
                    out[s + r["id"]] = r.get("result")
            except Exception:  # noqa: BLE001
                self.i += 1
            for k, (m, p) in enumerate(part):
                if out[s + k] is None:
                    try:
                        out[s + k] = self.call(m, p, tries=3)
                    except Exception:  # noqa: BLE001
                        pass
        return out

    def eth_call(self, to, data, block="latest"):
        return self.call("eth_call", [{"to": to, "data": data}, block], tries=3)


def words(data):
    d = data[2:]
    return [int(d[i:i + 64], 16) for i in range(0, len(d), 64)]


def addr(x):
    return "0x%040x" % x if isinstance(x, int) else "0x" + x[-40:]


def try_addr(rpc, to, sel):
    try:
        r = rpc.eth_call(to, sel)
        return addr(r[26:66]) if r and len(r) >= 66 and int(r[2:66], 16) else None  # first word: some endpoints pad results
    except Exception:  # noqa: BLE001
        return None


_chainlist = []


def chainlist():
    """chainlist.org RPC list, cached for a week; loaded once (venues scan in parallel threads)."""
    with _lock:
        if not _chainlist:
            path = os.path.join(OUT, "chainlist-rpcs.json")
            if not os.path.exists(path) or time.time() - os.path.getmtime(path) > 7 * 86400:
                data = get(CHAINLIST, 120)
                with open(path + ".tmp", "w") as f:
                    json.dump(data, f)
                os.replace(path + ".tmp", path)
            _chainlist.extend(json.load(open(path)))
    return _chainlist


def pick_rpcs(chain_id, probe_addrs, topics, days, want=3):
    """Public endpoints that answer for this chain AND still serve logs at the start of the window."""
    c = next((c for c in chainlist() if c.get("chainId") == chain_id), None)
    urls = [] if not c else [u for u in ((r["url"] if isinstance(r, dict) else r) for r in c["rpc"])
                             if u.startswith("https://") and "${" not in u and "API_KEY" not in u.upper()]
    urls.sort(key=lambda u: not any(p in u for p in PREFERRED))
    good = []
    for u in urls[:30]:
        try:
            q = lambda m, p, t=15: post(u, {"jsonrpc": "2.0", "id": 1, "method": m, "params": p}, t)  # noqa: E731
            if int(q("eth_chainId", [])["result"], 16) != chain_id:
                continue
            head = int(q("eth_blockNumber", [])["result"], 16)
            t1 = int(q("eth_getBlockByNumber", [hex(head), False])["result"]["timestamp"], 16)
            t0 = int(q("eth_getBlockByNumber", [hex(head - 10000), False])["result"]["timestamp"], 16)
            lo = max(1, head - int(days * 86400 / max((t1 - t0) / 10000, 1e-3)))
            j = q("eth_getLogs", [{"fromBlock": hex(lo), "toBlock": hex(lo + 500), "address": probe_addrs, "topics": [topics]}], 20)
            if "result" in j and q("eth_getBlockByNumber", [hex(lo), False]).get("result"):
                good.append(u)
                if len(good) >= want:
                    break
        except Exception:  # noqa: BLE001 - unusable endpoint
            continue
    return good


def block_ts(rpc, n):
    return int(rpc.call("eth_getBlockByNumber", [hex(n), False])["timestamp"], 16)


def block_at(rpc, ts, head, head_ts):
    """Secant search from the head with the measured block time (never touches pre-window history)."""
    bt = max((head_ts - block_ts(rpc, head - 10000)) / 10000, 1e-3)
    n = max(1, head - int((head_ts - ts) / bt))
    for _ in range(4):
        n = max(1, min(head, n + int((ts - block_ts(rpc, n)) / bt)))
    return n


def venue_path(chain, venue):
    return os.path.join(OUT, f"{chain}.{venue}.json")


def emitters(rpc, kind, address):
    """Addresses that emit the liquidation event: the pool, or every market of a Compound v2 comptroller."""
    if kind != "compound":
        return [address]
    w = words(rpc.eth_call(address, SEL["getAllMarkets"]))
    return [addr(x) for x in w[2:2 + w[1]]]


def scan_venue(days, v):
    chain, venue, kind, address = v
    chain_id, _, _, wrapped = CHAINS[chain]
    topic = TOPIC[kind]
    probe = Rpc(pick_rpcs(chain_id, [address], [topic], days, want=1) or [])
    if not probe.urls:
        return log(f"[{chain}.{venue}] no usable public RPC")
    addrs = emitters(probe, kind, address)
    rpc = Rpc(pick_rpcs(chain_id, addrs, [topic], days))
    if not rpc.urls:
        return log(f"[{chain}.{venue}] no public RPC serves logs for all {len(addrs)} emitters at the window start")
    head = int(rpc.call("eth_blockNumber", []), 16)
    head_ts = block_ts(rpc, head)
    start = block_at(rpc, head_ts - days * 86400, head, head_ts)

    logs, to, chunk, budget = [], head, 20000, CALL_BUDGET
    while to >= start and budget > 0:
        frm = max(start, to - chunk + 1)
        budget -= 1
        try:
            res = rpc.call("eth_getLogs", [{"fromBlock": hex(frm), "toBlock": hex(to), "address": addrs, "topics": [[topic]]}], tries=1)
        except Exception:  # noqa: BLE001 - range too wide or endpoint hiccup: shrink and rotate
            chunk = max(10, chunk // 2)
            continue
        if len(res) >= 500 and chunk > 10:  # some providers silently cap results: keep each answer small
            chunk = max(10, chunk // 4)
            continue
        logs.extend(res)
        to = frm - 1
        if len(res) < 100:
            chunk = min(2_000_000, int(chunk * 1.5))
    covered_from = to + 1 if to >= start else start
    covered_ts = block_ts(rpc, covered_from)

    events, cache = [], {}

    def underlying(market):
        if ("u", market) not in cache:
            cache[("u", market)] = try_addr(rpc, market, SEL["underlying"]) or wrapped or market
        return cache[("u", market)]

    for lg in logs:
        t, w = lg["topics"], words(lg["data"])
        e = {"block": int(lg["blockNumber"], 16), "tx": lg["transactionHash"], "index": int(lg["transactionIndex"], 16)}
        if kind == "aave":
            e.update(collateral=addr(t[1]), debt=addr(t[2]), user=addr(t[3]), debt_amt=w[0], coll_amt=w[1], contract=addr(w[2]))
        elif kind == "morpho":
            if t[1] not in cache:
                r = words(rpc.eth_call(address, SEL["idToMarketParams"] + t[1][2:]))
                cache[t[1]] = (addr(r[0]), addr(r[1]), addr(r[2]))
            loan, coll, oracle = cache[t[1]]
            e.update(collateral=coll, debt=loan, oracle=oracle, user=addr(t[3]), debt_amt=w[0], coll_amt=w[2], contract=addr(t[2]))
        else:
            c_token = addr(w[3])
            if ("x", c_token) not in cache:
                cache[("x", c_token)] = int(rpc.eth_call(c_token, SEL["exchangeRateStored"])[2:66], 16)
            e.update(collateral=underlying(c_token), debt=underlying(lg["address"].lower()), user=addr(w[1]), debt_amt=w[2],
                     coll_amt=w[4] * cache[("x", c_token)] // 10**18, contract=addr(w[0]), market=lg["address"].lower())
        e["ts"] = covered_ts + (head_ts - covered_ts) * (e["block"] - covered_from) / max(1, head - covered_from)
        events.append(e)

    res = {"chain": chain, "venue": venue, "kind": kind, "address": address, "emitters": addrs, "rpcs": rpc.urls,
           "head": head, "head_ts": head_ts, "covered_from": covered_from, "covered_ts": covered_ts,
           "window_days": (head_ts - covered_ts) / 86400, "complete": to < start, "rpc_calls": rpc.calls + probe.calls,
           "block_time": (head_ts - covered_ts) / max(1, head - covered_from), "events": events}
    json.dump(res, open(venue_path(chain, venue), "w"))
    log(f"[{chain}.{venue}] {len(events)} liquidations over {res['window_days']:.1f} d, {res['rpc_calls']} calls, complete={res['complete']}")


def selected(opts):
    chains = set(opts["chains"].split(",")) if opts.get("chains") else None
    venues = set(opts["venues"].split(",")) if opts.get("venues") else None
    return [v for v in VENUES if (not chains or v[0] in chains) and (not venues or v[1] in venues or f"{v[0]}.{v[1]}" in venues)]


def load_all(opts=None):
    out = []
    for v in (selected(opts) if opts else VENUES):
        p = venue_path(v[0], v[1])
        if os.path.exists(p):
            out.append(json.load(open(p)))
    return out


def run_parallel(fn, items, workers=8):
    def safe(x):
        try:
            return fn(x)
        except Exception as e:  # noqa: BLE001 - one venue failing must not stop the rest
            log(f"[{x[0]}.{x[1]}] failed: {str(e)[:200]}")
    with ThreadPoolExecutor(max_workers=workers) as ex:
        return list(ex.map(safe, items))


def cmd_scan(opts):
    days = int(opts.get("days", 90))
    run_parallel(lambda v: scan_venue(days, v), selected(opts))


def cmd_validate(opts):
    random.seed(7)

    def check(d):
        rpc, lo_w, hi_w = Rpc(d["rpcs"]), d["covered_from"], d["head"]
        centers = [e["block"] for e in random.sample(d["events"], min(3, len(d["events"])))] + [random.randint(lo_w, hi_w) for _ in range(2)]
        verdict = "VALID"
        for c in centers:
            lo, hi = max(lo_w, c - 2000), min(hi_w, c + 2000)
            try:
                n = sum(len(rpc.call("eth_getLogs", [{"fromBlock": hex(a), "toBlock": hex(min(a + 499, hi)), "address": d["emitters"],
                                                      "topics": [[TOPIC[d["kind"]]]]}], tries=4)) for a in range(lo, hi + 1, 500))
            except Exception as e:  # noqa: BLE001
                verdict = "UNVERIFIED"
                log(f"[{d['chain']}.{d['venue']}] {lo}-{hi} error {str(e)[:80]}")
                continue
            s = sum(lo <= e["block"] <= hi for e in d["events"])
            if s != n:
                verdict = "MISMATCH"
                log(f"[{d['chain']}.{d['venue']}] {lo}-{hi} scan {s} reference {n}")
        d["validated"] = verdict
        json.dump(d, open(venue_path(d["chain"], d["venue"]), "w"))
        log(f"[{d['chain']}.{d['venue']}] {verdict}")
    with ThreadPoolExecutor(max_workers=8) as ex:
        list(ex.map(check, load_all(opts)))


_prices = {}


def prices(keys):
    want = sorted({k.lower() for k in keys} - set(_prices))
    for i in range(0, len(want), 60):
        for _ in range(3):
            try:
                got = get("https://coins.llama.fi/prices/current/" + ",".join(want[i:i + 60])).get("coins", {})
                _prices.update({k.lower(): v for k, v in got.items()})
                break
            except Exception:  # noqa: BLE001
                time.sleep(2)
    return _prices


def price_events(d):
    """Adds debt_usd / coll_usd / bonus_usd to each event (None if a token has no price)."""
    llama = CHAINS[d["chain"]][1]
    px = prices([f"{llama}:{e[k]}" for e in d["events"] for k in ("collateral", "debt")])
    for e in d["events"]:
        c, b = px.get(f"{llama}:{e['collateral']}".lower()), px.get(f"{llama}:{e['debt']}".lower())
        if not c or not b:
            e["debt_usd"] = e["coll_usd"] = e["bonus_usd"] = None
            continue
        e["coll_usd"] = e["coll_amt"] / 10 ** c["decimals"] * c["price"]
        e["debt_usd"] = e["debt_amt"] / 10 ** b["decimals"] * b["price"]
        bonus = max(0.0, e["coll_usd"] - e["debt_usd"])
        e["bonus_usd"] = 0.0 if bonus > 50 * max(e["debt_usd"], 1) else bonus  # decimals/pricing outlier


def cmd_senders(opts):
    """Adds each event's tx sender (the operator's wallet) for venues paying at least --min-bonus a month."""
    floor = float(opts.get("min-bonus", 1000))

    def enrich(d):
        price_events(d)
        per30 = sum(e["bonus_usd"] or 0 for e in d["events"]) * 30 / max(d["window_days"], 0.01)
        todo = [e for e in d["events"] if "sender" not in e]
        if per30 < floor or not todo:
            return
        rpc = Rpc(d["rpcs"])
        txs = rpc.batch([("eth_getTransactionByHash", [e["tx"]]) for e in todo])
        for e, tx in zip(todo, txs):
            if tx:
                e["sender"] = tx["from"].lower()
                e["to"] = (tx.get("to") or "").lower()
        json.dump(d, open(venue_path(d["chain"], d["venue"]), "w"))
        log(f"[{d['chain']}.{d['venue']}] senders for {sum('sender' in e for e in d['events'])}/{len(d['events'])} events, {rpc.calls} calls")
    with ThreadPoolExecutor(max_workers=8) as ex:
        list(ex.map(enrich, load_all(opts)))


def shares(by):
    total = sum(by.values())
    s = sorted(by.values(), reverse=True)
    return (round(s[0] / total, 2), round(sum(s[:3]) / total, 2)) if total and s else (None, None)


def cmd_summary(opts):
    rows = []
    for d in load_all():
        price_events(d)
        evs, scale = d["events"], 30 / max(d["window_days"], 0.01)
        priced = [e for e in evs if e["bonus_usd"] is not None]
        bonus = sum(e["bonus_usd"] for e in priced)
        by_contract, by_sender, by_day = defaultdict(float), defaultdict(float), defaultdict(float)
        for e in priced:
            by_contract[e["contract"]] += e["bonus_usd"]
            by_day[int(e["ts"] // 86400)] += e["bonus_usd"]
            if "sender" in e:
                by_sender[e["sender"]] += e["bonus_usd"]
        top_sender = shares(by_sender) if len(by_sender) and sum("sender" in e for e in evs) >= 0.9 * len(evs) else (None, None)
        rows.append({
            "chain": d["chain"], "venue": d["venue"], "window_days": round(d["window_days"], 1), "validated": d.get("validated", "-"),
            "liq_per_30d": round(len(evs) * scale, 1), "repaid_usd_30d": round(sum(e["debt_usd"] for e in priced) * scale),
            "bonus_usd_30d": round(bonus * scale),
            "median_repaid_usd": round(statistics.median([e["debt_usd"] for e in priced])) if priced else 0,
            "contracts": len({e["contract"] for e in evs}), "senders": len({e["sender"] for e in evs if "sender" in e}) or None,
            "top1_share": shares(by_contract)[0], "top3_share": shares(by_contract)[1],
            "top1_sender_share": top_sender[0], "top3_sender_share": top_sender[1],
            "biggest_day_share": round(max(by_day.values()) / bonus, 2) if bonus else None,
            "unpriced": len(evs) - len(priced),
        })
    rows.sort(key=lambda r: -r["bonus_usd_30d"])
    json.dump(rows, open(os.path.join(OUT, "summary.json"), "w"), indent=1)
    cols = ["chain", "venue", "liq_per_30d", "repaid_usd_30d", "bonus_usd_30d", "median_repaid_usd", "contracts", "senders",
            "top1_share", "top3_share", "top1_sender_share", "top3_sender_share", "biggest_day_share", "unpriced", "validated"]
    md = ["| " + " | ".join(cols) + " |", "|" + "---|" * len(cols)] + ["| " + " | ".join(str(r[c]) for c in cols) + " |" for r in rows]
    open(os.path.join(OUT, "summary.md"), "w").write("\n".join(md) + "\n")
    print("\n".join(md))


def oracle_aggregators(rpc, d, e, cache):
    """Chainlink aggregators whose AnswerUpdated can make this position liquidatable (empty if not Chainlink-style)."""
    if d["kind"] == "aave":
        key = ("aave", e["collateral"], e["debt"])
        if key not in cache:
            provider = try_addr(rpc, d["address"], SEL["ADDRESSES_PROVIDER"])
            oracle = provider and try_addr(rpc, provider, SEL["getPriceOracle"])
            feeds = [oracle and try_addr(rpc, oracle, SEL["getSourceOfAsset"] + "%064x" % int(a, 16)) for a in (e["collateral"], e["debt"])]
            cache[key] = feeds
        feeds = cache[key]
    elif d["kind"] == "morpho":
        key = ("morpho", e["oracle"])
        if key not in cache:
            cache[key] = [try_addr(rpc, e["oracle"], SEL[f]) for f in ("BASE_FEED_1", "BASE_FEED_2", "QUOTE_FEED_1", "QUOTE_FEED_2")]
        feeds = cache[key]
    else:
        return []
    aggs = []
    for f in filter(None, feeds):
        if ("agg", f) not in cache:
            cache[("agg", f)] = try_addr(rpc, f, SEL["aggregator"]) or f
        aggs.append(cache[("agg", f)])
    return sorted(set(aggs))


def cmd_timing(opts):
    """Per sampled liquidation: what the winner paid (priority fee, total fee) and how long after the price update it landed."""
    n_sample = int(opts.get("sample", 100))
    random.seed(11)
    report = []
    for d in load_all(opts):
        price_events(d)
        chain_native = CHAINS[d["chain"]][2]
        native = prices([f"coingecko:{chain_native}"]).get(f"coingecko:{chain_native}", {}).get("price")
        priced = sorted((e for e in d["events"] if e["bonus_usd"]), key=lambda e: -e["bonus_usd"])
        sample = priced[: n_sample * 6 // 10]
        rest = priced[len(sample):]
        sample += random.sample(rest, min(len(rest), n_sample - len(sample)))
        rpc, cache, lookback = Rpc(d["rpcs"]), {}, int(900 / max(d["block_time"], 0.05))
        rows = []
        for e in sample:
            try:
                tx = rpc.call("eth_getTransactionByHash", [e["tx"]])
                rc = rpc.call("eth_getTransactionReceipt", [e["tx"]])
                blk = rpc.call("eth_getBlockByNumber", [hex(e["block"]), False])
            except Exception as ex:  # noqa: BLE001
                log(f"[{d['chain']}.{d['venue']}] {e['tx']} {str(ex)[:80]}")
                continue
            gas, egp = int(rc["gasUsed"], 16), int(rc.get("effectiveGasPrice") or tx.get("gasPrice") or "0x0", 16)
            base_fee = int(blk.get("baseFeePerGas") or "0x0", 16)
            l1 = int(rc.get("l1Fee") or "0x0", 16)
            r = {"tx": e["tx"], "sender": tx["from"].lower(), "bonus_usd": e["bonus_usd"], "index": e["index"],
                 "block_txs": len(blk["transactions"]),
                 "priority_usd": max(0, egp - base_fee) * gas / 1e18 * (native or 0),
                 "fee_usd": (egp * gas + l1) / 1e18 * (native or 0), "delay_s": None, "same_block_backrun": None}
            aggs = oracle_aggregators(rpc, d, e, cache)
            if aggs:
                try:
                    ups = rpc.call("eth_getLogs", [{"fromBlock": hex(e["block"] - lookback), "toBlock": hex(e["block"]),
                                                    "address": aggs, "topics": [[ANSWER_UPDATED]]}], tries=3)
                    ups = [u for u in ups if (int(u["blockNumber"], 16), int(u["transactionIndex"], 16)) < (e["block"], e["index"])]
                    if ups:
                        u = max(ups, key=lambda u: (int(u["blockNumber"], 16), int(u["transactionIndex"], 16)))
                        ub = int(u["blockNumber"], 16)
                        r["delay_s"] = int(blk["timestamp"], 16) - (int(blk["timestamp"], 16) if ub == e["block"] else block_ts(rpc, ub))
                        r["same_block_backrun"] = ub == e["block"]
                        r["delay_blocks"] = e["block"] - ub
                except Exception:  # noqa: BLE001
                    pass
            r["oracle"] = "chainlink" if aggs else "other"
            rows.append(r)
        if not rows:
            continue
        with_upd = [r for r in rows if r["delay_s"] is not None]
        bon = sum(r["bonus_usd"] for r in rows)
        rep = {
            "chain": d["chain"], "venue": d["venue"], "sampled": len(rows),
            "bonus_sampled_usd": round(bon), "fees_paid_usd": round(sum(r["fee_usd"] for r in rows)),
            "fees_share_of_bonus": round(sum(r["fee_usd"] for r in rows) / bon, 2) if bon else None,
            "median_priority_usd": round(statistics.median(r["priority_usd"] for r in rows), 3),
            "p90_priority_usd": round(sorted(r["priority_usd"] for r in rows)[int(0.9 * (len(rows) - 1))], 2),
            "max_fee_usd": round(max(r["fee_usd"] for r in rows), 2),
            "chainlink_update_found": f"{len(with_upd)}/{len(rows)}",
            "same_block_backrun": f"{sum(r['same_block_backrun'] for r in with_upd)}/{len(with_upd)}" if with_upd else "-",
            "median_delay_s": statistics.median(r["delay_s"] for r in with_upd) if with_upd else None,
            "within_2s": f"{sum(r['delay_s'] <= 2 for r in with_upd)}/{len(with_upd)}" if with_upd else "-",
            "later_than_60s": f"{sum(r['delay_s'] > 60 for r in with_upd)}/{len(with_upd)}" if with_upd else "-",
            "median_tx_index": statistics.median(r["index"] for r in rows), "senders": len({r["sender"] for r in rows}),
            "rpc_calls": rpc.calls,
        }
        log(json.dumps(rep))
        report.append(rep)
        json.dump(rows, open(os.path.join(OUT, f"timing.{d['chain']}.{d['venue']}.json"), "w"))
    json.dump(report, open(os.path.join(OUT, "timing.json"), "w"), indent=1)
    print(json.dumps(report, indent=1))


def parse(argv):
    opts, k = {}, None
    for a in argv:
        if a.startswith("--"):
            k = a[2:]
            opts[k] = "1"
        elif k:
            opts[k], k = a, None
    return opts


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in ("scan", "validate", "senders", "summary", "timing"):
        sys.exit(__doc__)
    OPTS = parse(sys.argv[2:])
    OUT = OPTS.get("out", "data/liq-competition")
    os.makedirs(OUT, exist_ok=True)
    {"scan": cmd_scan, "validate": cmd_validate, "senders": cmd_senders, "summary": cmd_summary, "timing": cmd_timing}[sys.argv[1]](OPTS)
