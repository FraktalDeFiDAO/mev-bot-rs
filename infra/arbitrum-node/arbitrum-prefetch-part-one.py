#!/usr/bin/env python3
"""Migration-only: download part one while retained part zero replays.

This shares the importer's lock. Only one new-volume part is staged. It never
extracts data and has no effect once the fully verified part has been consumed.
"""
import importlib.machinery
import importlib.util
import json
from pathlib import Path

loader = importlib.machinery.SourceFileLoader("snapshot_import", "/usr/local/sbin/arbitrum-snapshot-init")
spec = importlib.util.spec_from_loader(loader.name, loader)
module = importlib.util.module_from_spec(spec)
loader.exec_module(module)
plan = json.loads(Path("/data/arbitrum/snapshot-import-plan.json").read_text())
part = plan["metadata"]["parts"][1]
module.download_ranges(plan["url"] + part["filename"], Path("/data/arbitrum/snapshot-cache") / part["filename"], part["size_bytes"])
