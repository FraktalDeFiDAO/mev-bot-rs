---
graphify-generated: true
graphify-community: "160"
graphify-node-count: 16
graphify-internal-edges: 15
tags:
  - graphify/community
---

# JEV-LIQ-015 - REVM exact-call liquidation simulation and Aave fork evidence

Back to [[docs/knowledge/Knowledge Map|Knowledge Map]].

## Source files

- `docs/design/jev-liq-015-revm-liquidation.md` — 16 nodes

## Nodes (16)

- **JEV-LIQ-015 - REVM exact-call liquidation simulation and Aave fork evidence** — `docs/design/jev-liq-015-revm-liquidation.md:L1`
- **4. Fork evidence (all measured; anvil 1.8.4, forge toolchain container)** — `docs/design/jev-liq-015-revm-liquidation.md:L42`
- **2. API contract** — `docs/design/jev-liq-015-revm-liquidation.md:L17`
- **(a) Real borrower, REVM vs block-pinned eth_call (public RPC, `real_borrowers_revm_account_data_equals_block_pinned_eth_call`)** — `docs/design/jev-liq-015-revm-liquidation.md:L51`
- **(b) Eligibility boundary (anvil fork at 514537429, `anvil_eligibility_boundary_is_exact_in_u256`)** — `docs/design/jev-liq-015-revm-liquidation.md:L54`
- **(c) Exact calldata through ArbExecutor (`liquidateViaAave`, final calldata built by `final_liquidation_calldata`)** — `docs/design/jev-liq-015-revm-liquidation.md:L59`
- **(d) Stale opportunity (`anvil_stale_simulation_is_refused_after_the_state_moves`)** — `docs/design/jev-liq-015-revm-liquidation.md:L71`
- **1. What was added** — `docs/design/jev-liq-015-revm-liquidation.md:L5`
- **3. Calldata parity (gap 5)** — `docs/design/jev-liq-015-revm-liquidation.md:L33`
- **5. Limits (not claimed)** — `docs/design/jev-liq-015-revm-liquidation.md:L77`
- **6. What task 016 (ground-truth labels) must consume** — `docs/design/jev-liq-015-revm-liquidation.md:L83`
- **7. Reproduce** — `docs/design/jev-liq-015-revm-liquidation.md:L90`
- **Arbitrum L1 data fee: what was verified (2026-10-13)** — `docs/design/jev-liq-015-revm-liquidation.md:L30`
- **jev-liq-015-revm-liquidation.md** — `docs/design/jev-liq-015-revm-liquidation.md:L1`
- **Non-network unit tests (8 in `liquidation.rs`)** — `docs/design/jev-liq-015-revm-liquidation.md:L74`
- **Rules read from the deployed source (`LiquidationLogic.sol`)** — `docs/design/jev-liq-015-revm-liquidation.md:L46`

## Related communities

- No selected community has a direct edge to this one.
