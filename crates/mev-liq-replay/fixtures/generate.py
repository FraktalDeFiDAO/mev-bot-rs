#!/usr/bin/env python3
"""Generate small SYNTHETIC scheduler scenarios. No chain/API data or economic labels."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SEC = 10**9
WAD = 10**18


def addr(n):
    return "0x" + format(n, "040x")


def write(name, count, duration, shock=False):
    header = dict(
        kind="Header", format_version=1, dataset_id=f"synthetic-{name}-v1",
        synthetic=True, scenario=name,
        provenance="fixtures/generate.py: deterministic synthetic accounts; 1 s modeled blocks; no RPC/REVM or historical economic evidence",
        chain_id=42161, pool=addr(0xAA), source_block_start=100,
        source_block_end=100 + duration, duration_ns=duration * SEC,
        measure_from_ns=SEC, seed=0,
    )
    events = []

    def add(t, kind, **fields):
        events.append(dict(seq=len(events)+1, t_ns=t*SEC, block=100+t, kind=kind, **fields))

    def state(n, t, hf):
        add(t, "State", borrower=addr(n), health_factor_wad=str(hf),
            total_debt_base="10000000000", total_collateral_base="20000000000",
            reserve_asset=addr(0xEE))

    for n in range(1, count+1):
        add(0, "Register", borrower=addr(n))
        state(n, 0, WAD*101//100 if shock else WAD*2)
    if shock:
        for n in range(1, count//2+1):
            state(n, 30, WAD*98//100)
            add(30, "Eligible", borrower=addr(n), actionable=False)
        add(30, "OracleMove", asset=addr(0xEE), move_bps=500)
        for n in range(1, count//2+1):
            state(n, 60, WAD*110//100)
            add(60, "BorrowerEvent", borrower=addr(n))
    with (ROOT/f"{name}.jsonl").open("w") as f:
        for row in [header, *events]:
            f.write(json.dumps(row, sort_keys=True, separators=(",", ":"))+"\n")


if __name__ == "__main__":
    write("quiet", 16, 600)
    write("oracle-shock", 48, 120, shock=True)
    write("overload", 80, 60)
