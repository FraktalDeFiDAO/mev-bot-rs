# Standing invariants: Arbitrum sequencer-first MEV engine

(Supplied by the project owner as the `CLAUDE.md` for the sequencer-first build. Stored here so the
repo's own `CLAUDE.md` keeps its project rules; `CLAUDE.md` links to this file. **Scope (decided by the owner
2026-10-07, "all live"):** these invariants bind **everything**, including the bot that is live on `mev-ohio`.
The live bot predates them; `docs/design/audit-sequencer-first.md` §6 is the compliance matrix (status per invariant,
remediation, and what blocks it). Changes to the running bot are made in small reviewed PRs and deployed only with the owner's go.)

The full build spec lives in `docs/tasks/sequencer-first-engine.md`; progress lives in
`docs/PROGRESS.md`. Read both at the start of each session before doing anything.

## Target

Arbitrum One, chain ID 42161. Ordering policy as of 2026-09: Priority Gas Auctions
(PGA), which replaced Timeboost. Ordering policy is DAO-controlled and has changed
before. Before relying on any Arbitrum-specific behavior (feed formats, round
timing, fee semantics, RPC methods, precompiles), confirm it against current docs
(`https://docs.arbitrum.io/llms.txt`, append `.md` to any docs URL for markdown)
or the Nitro source, and record the fact plus its source and date in
`docs/arbitrum-facts.md`. Training-data knowledge of Arbitrum is not sufficient.

## Hard invariants (never violate; if a task seems to require it, stop and ask)

1. **Integer math only** on every amount, reserve, price, fee, gas, and profit
   path. U256 (or equivalent) with each protocol's exact on-chain rounding.
   No `float`/`f64` except in metrics/display code that never feeds a decision.
2. **Chain state wins.** When local state disagrees with node state, the node
   wins, and the correction is recorded as a `StateCorrection` with a reason
   code. Never overwrite cached state without emitting that record.
3. **Every opportunity carries the exact state versions it was computed from.**
   If any version is superseded before simulation or before submission, drop it.
4. **Hot path purity.** Between feed message receipt and submission: no remote
   RPC, no disk/DB I/O, no blocking log or metric writes, no global scans,
   no unbounded allocation. Diagnostics go to async channels that may drop.
5. **Execution safety.** `EXECUTION_MODE` defaults to `shadow` (compute, simulate,
   build, sign with a throwaway key, log what would have been sent; never
   submit). Do not change the default, do not handle or generate production
   keys, do not fund wallets, and do not set `live`. A human does that.
6. **Our own transactions** flow through the normal ingestion → state →
   reconciliation path. No special-casing.
7. **Degraded state is contagious.** Any market whose state confidence is below
   the strategy's required level cannot be used to produce a submission.
8. **No stubs reported as done.** A phase is complete only when its Definition
   of Done in the task spec passes. `todo!()`, `unimplemented!()`, `panic("TODO")`,
   or mocked data in a production path means the phase is not done.

## State status vocabulary (use these names exactly)

- `Tentative`: derived from a Fast Feed message. Executed by the Sequencer, but
  the block is not stored yet and can be lost.
- `SoftFinal`: block observed on the standard sequencer feed / local Nitro node.
- `Reconciled`: our derived state was compared against node state for that block
  and matched (or was corrected).
- `L1Posted`, `L1Finalized`: batch posted / finalized on Ethereum. Track only
  where operationally useful.
- `Hypothetical`: state produced by simulating a transaction that has not been
  sequenced (e.g. our candidate). Never published to the shared cache.

## Working style

- Interface sketches in the spec are illustrative. Implement in the repo's
  existing language and idioms.
- Prefer small, reviewable commits per sub-step, each with passing tests.
- Update `docs/PROGRESS.md` at the end of every phase: what was done, what was
  verified and how, open issues, and what the next phase needs.
- When context gets long, write state to `docs/PROGRESS.md` before continuing so
  a fresh session can resume.
