# CLI Coder Handoff — Jev Liquidation Watchlist

**Read in full before changing code:** `copper_jev_liquidation_architecture.md`.

## Objective

Integrate an asynchronous hosted TypeSafe Jev decision advisor for **optional Aave V3 borrower monitoring and liquidation route preparation** into my existing Rust/Go Arbitrum One MEV/REVM engine. Preserve deterministic hot-path eligibility, exact-call REVM simulation, profitability gates, and sequencer/PGA submission. Rootless Podman must be the primary development/deployment environment; Docker Compose compatibility is mandatory.

## Starting conditions

I have an existing arbitrage/liquidation codebase. **Do not assume paths, crate structure, deployment addresses, REVM versions, or completeness of borrower indexing.** First inspect the actual repository and resolve existing interfaces. The spec's suggested layout is only a mapping guide. Do not create duplicate engines.

## Nonnegotiable instructions

- No unfinished functions, fake implementations, mocks in production, placeholder methods, guessed TypeSafe JSON fields, or false claims about tests.
- One `JEV-LIQ-NNN` task per agent context / independently reviewable PR.
- Keep high-priority account events and mandatory sweeps fully independent of Jev. Jev only supplies positive tie-break ranking within eligible discretionary P2 work after validated, revision-bound responses.
- Consume official `https://api.typesafe.ai/v1/systemone` API; not the local coding MCP as the bot's runtime.
- Redact features; no secrets, borrower address, private route data, or wallet signing inside advisor service.
- Aave deployment/ABI must be verified on chain; do not hardcode assumptions about protocol rule versions.
- Use exact U256/fixed-point data for health and money and consistent block snapshots; never use Jev or floats for financial correctness.
- Require replay comparing deterministic baseline, simple heuristic, shadow Jev, and proposed Jev policy at equal RPC/CPU budgets before live influence.
- Preserve all existing passing tests; introduce behavior-driven negative, integration, and fork tests.
- Maintain signed-off migration/rollback, structured logging/metrics, and documented container runbooks.

## First assignment — JEV-LIQ-001

Inventory repository architecture: source tree, Cargo workspace, Go modules, executable binaries, current borrower scanner/account index, Aave Pool/Oracle integration, RPC/log feeds, REVM simulation, current scheduler, transaction executor, data stores, CI, and container setup. Return exact file paths and callgraphs for each; identify missing components; produce a dependency graph and executable baseline test commands. **Do not change trading behavior in task 001.** Then implement the next dependency-ready JEV-LIQ task only after 001 is complete.

## Required completion report per PR

Report task ID, actual files changed, tests run with observed results, fmt/clippy status, dependency changes, infra changes, security and race analysis, measured metrics, unresolved blockers, and next independent task. Stop merging on red CI.
