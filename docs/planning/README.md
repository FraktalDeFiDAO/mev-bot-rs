# Planning Index

Start here when resuming development.

| Doc | Contents |
|---|---|
| [PLAN.md](PLAN.md) | Strategy, phases 0–5, go/no-go gates, capital allocation, risks |
| [CRATES.md](CRATES.md) | Cargo workspace layout: one crate per component, layering rules |
| [INFRASTRUCTURE.md](INFRASTRUCTURE.md) | Servers, free/paid RPC, node providers, latency, cost stages, sources |
| [DECISIONS.md](DECISIONS.md) | Log of decisions made so far and why |
| [SIM_ENGINE.md](SIM_ENGINE.md) | Simulation-engine audit, roadmap and measured findings |
| [AUDIT_2026-10-05.md](AUDIT_2026-10-05.md) | Efficiency/profitability audit; PGA ordering change; decisions #52–#58 |
| [AWS_MIGRATION.md](AWS_MIGRATION.md), [TESTING.md](TESTING.md) | Ohio host move; test strategy |

## Current status (2026-10-05)

The bot is live on AWS us-east-2. Resume from `.ai/STATE.md`. Open item from the audit: confirm the
live Arbitrum ordering rule (PGA vs Timeboost), then add a priority-fee bid and win-rate gating
(DECISIONS #52–#54).
