---
graphify-generated: true
graphify-community: "37"
graphify-node-count: 47
graphify-internal-edges: 46
tags:
  - graphify/community
---

# Medium findings

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `docs/audits/2026-10-11-complete-audit.md` — 40 nodes
- `docs/audits/2026-10-11-remediation.md` — 7 nodes

## Nodes (47)

- **Medium findings** — `docs/audits/2026-10-11-complete-audit.md:L140`
- **MEV bot audit — 11 October 2026** — `docs/audits/2026-10-11-complete-audit.md:L1`
- **High findings** — `docs/audits/2026-10-11-complete-audit.md:L38`
- **Audit remediation — in progress** — `docs/audits/2026-10-11-remediation.md:L1`
- **Low findings** — `docs/audits/2026-10-11-complete-audit.md:L194`
- **2026-10-11-complete-audit.md** — `docs/audits/2026-10-11-complete-audit.md:L1`
- **2026-10-11-remediation.md** — `docs/audits/2026-10-11-remediation.md:L1`
- **Decision** — `docs/audits/2026-10-11-complete-audit.md:L3`
- **Finding status** — `docs/audits/2026-10-11-remediation.md:L19`
- **Findings from older audits that should not be repeated as current facts** — `docs/audits/2026-10-11-complete-audit.md:L246`
- **H01 — Production lacks newer persistent limits and the global loss gate** — `docs/audits/2026-10-11-complete-audit.md:L40`
- **H02 — Daily budgets count settled spend but do not reserve pending liabilities** — `docs/audits/2026-10-11-complete-audit.md:L48`
- **H03 — Restart and receipt errors discard outstanding transaction exposure** — `docs/audits/2026-10-11-complete-audit.md:L58`
- **H04 — Nonce reset can collide with held reservations; ambiguous acceptance can trigger a fresh execution** — `docs/audits/2026-10-11-complete-audit.md:L66`
- **H05 — Estimated profit increases spend limits as if it were realized profit** — `docs/audits/2026-10-11-complete-audit.md:L76`
- **H06 — The on-chain profit floor does not cover the submitted transaction's allowed gas cost** — `docs/audits/2026-10-11-complete-audit.md:L86`
- **H07 — Background tick reload can overwrite newer event-derived state and combine different blocks** — `docs/audits/2026-10-11-complete-audit.md:L96`
- **H08 — Decision paths violate the required integer arithmetic invariant** — `docs/audits/2026-10-11-complete-audit.md:L106`
- **H09 — Atlas bidding float is swept as liquidation profit and can subsidize a losing trade** — `docs/audits/2026-10-11-complete-audit.md:L116`
- **H10 — Atlas submissions do not settle against local exposure or spending limits** — `docs/audits/2026-10-11-complete-audit.md:L132`
- **Implemented locally** — `docs/audits/2026-10-11-remediation.md:L5`
- **L01 — Two locked dependencies are marked unmaintained** — `docs/audits/2026-10-11-complete-audit.md:L196`
- **L02 — Nitro launch-script fixes exist outside the installed path** — `docs/audits/2026-10-11-complete-audit.md:L200`
- **L03 — Module ownership and documentation make safety review harder** — `docs/audits/2026-10-11-complete-audit.md:L204`
- **L04 — Shell quality checks need cleanup** — `docs/audits/2026-10-11-complete-audit.md:L208`
- **Latest continuation** — `docs/audits/2026-10-11-remediation.md:L48`
- **Limits of this audit** — `docs/audits/2026-10-11-complete-audit.md:L266`
- **M01 — Backruns bypass the global outstanding transaction limit** — `docs/audits/2026-10-11-complete-audit.md:L142`
- **M02 — Persistence fails open and is not crash-durable** — `docs/audits/2026-10-11-complete-audit.md:L146`
- **M03 — Non-finite configuration and guard inputs can bypass comparisons** — `docs/audits/2026-10-11-complete-audit.md:L150`
- **M04 — V4 liquidity events leave loaded tick maps stale** — `docs/audits/2026-10-11-complete-audit.md:L154`
- **M05 — Opportunity version protection ends before actual submission** — `docs/audits/2026-10-11-complete-audit.md:L158`
- **M06 — Reconnection leaves a snapshot/subscription gap without contagious degraded confidence** — `docs/audits/2026-10-11-complete-audit.md:L162`
- **M07 — Trading hot path still performs remote and synchronous disk work** — `docs/audits/2026-10-11-complete-audit.md:L166`
- **M08 — Batch flush does not repeat admission checks and can build zero floors** — `docs/audits/2026-10-11-complete-audit.md:L170`
- **M09 — Rule expression nesting can abort the Seqscope process** — `docs/audits/2026-10-11-complete-audit.md:L174`
- **M10 — SQL row caps do not bound query work or response bytes** — `docs/audits/2026-10-11-complete-audit.md:L178`
- **M11 — Production watchdog is absent; its health pattern can accept “unhealthy”** — `docs/audits/2026-10-11-complete-audit.md:L182`
- **M12 — CI omits substantial suites; Seqscope test wrapper can hide failing exit status** — `docs/audits/2026-10-11-complete-audit.md:L186`
- **M13 — Redaction does not cover URL userinfo; historical credential-like logging exists** — `docs/audits/2026-10-11-complete-audit.md:L190`
- **Production observations** — `docs/audits/2026-10-11-complete-audit.md:L23`
- **Remediation order and acceptance criteria** — `docs/audits/2026-10-11-complete-audit.md:L258`
- **Scope and evidence** — `docs/audits/2026-10-11-complete-audit.md:L11`
- **Standing invariant assessment** — `docs/audits/2026-10-11-complete-audit.md:L212`
- **Verification** — `docs/audits/2026-10-11-complete-audit.md:L225`
- **Verification** — `docs/audits/2026-10-11-remediation.md:L33`
- **Verified Atlas source constraint** — `docs/audits/2026-10-11-remediation.md:L44`

## Related communities

- No selected community has a direct edge to this one.
