# Session state

Read this plus `CLAUDE.md` to resume. Rewrite at every breakpoint; keep under ~400 words.
Durable facts and next actions only.

---

**Updated:** 2026-10-03
**Phase:** planning complete; no code yet.
**Goal:** profitable L2 MEV bot (liquidations + long-tail arb, flash loans) on $200 capital.

## Done

- Plans in `docs/planning/` (PLAN, CRATES, INFRASTRUCTURE, DECISIONS) — index: `docs/planning/README.md`.
- Token-saving setup: `.ai/` policy + preamble, `.graphifyignore`, `scripts/graph.sh`,
  Obsidian projection script, project read-deny rules in `.claude/settings.json`.
- Git initialized; nothing committed yet.

## Next

1. Scaffold the Cargo workspace per `docs/planning/CRATES.md` (17 crates, empty lib stubs).
2. `scripts/graph.sh optimize` to build the first graph + `docs/knowledge/`.
3. Phase 0: `mev-analytics` — liquidation + arb competitor scans on Arbitrum, Base, Optimism.

## Open questions

- None blocking.
