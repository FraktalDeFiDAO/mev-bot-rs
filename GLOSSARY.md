# mev-bot

An L2 MEV bot on Arbitrum One that finds and executes arbitrage and lending-protocol liquidations funded by flash loans.

## Liquidation

**Borrower**:
An address with open debt on a lending market, identified per chain and market.
_Avoid_: Account, user, position

**Health factor**:
A lending market's own measure of a borrower's collateral cover, read exactly from chain state; below one means the borrower may be liquidated.
_Avoid_: HF ratio, risk score

**Eligible**:
A borrower whose exact health factor, read at a single block, is below one while it still has debt.
_Avoid_: Liquidatable, underwater

**Actionable**:
An eligible borrower for which a route exists and a simulation of the exact liquidation shows positive net profit after all costs.
_Avoid_: Profitable, opportunity

**Mandatory tier**:
The recheck urgency of a borrower derived only from chain state and deadlines (P0 urgent, P1 safety, P2 discretionary, P3 background); advisory input can never change it.
_Avoid_: Priority, watch tier

**Maximum recheck age**:
The longest a tracked borrower may go without a fresh exact read before it becomes mandatory work.
_Avoid_: Sweep interval

**Hot borrower**:
A borrower whose last exact read shows debt at or above the dust cutoff; the maximum recheck age is guaranteed for it.
_Avoid_: Active borrower, big account

**Dust borrower**:
A borrower whose last exact read shows debt below the dust cutoff, too small to liquidate at a profit; it is re-read on a longer age and whenever an event touches it.
_Avoid_: Small account (code and config call this tier "cold")

**Never-read borrower**:
An indexed borrower with no exact read yet, so neither hot nor dust.
_Avoid_: Unknown account, new borrower

## State

**Block pin**:
A read of chain state fixed to one block hash so every value in it is mutually consistent.
_Avoid_: Snapshot (for this meaning), latest read

**Borrower snapshot**:
An immutable, exact record of one borrower's on-chain position at a block pin, with its provenance.
_Avoid_: Health, account state

**Account revision**:
The monotonically increasing version of one borrower's snapshot; a write naming an older revision is refused.
_Avoid_: Version, sweep revision

**Sweep revision**:
The single counter the legacy liquidation runner advances on each full sweep of all borrowers.
_Avoid_: Revision (unqualified)

**Canonical**:
State confirmed on the chain as built, as opposed to **provisional** state seen early on a sequencer feed that may still change.
_Avoid_: Final, confirmed (for feed data)

## Advisory

**Advisory decision**:
A hosted model's ranking hint for one borrower snapshot, valid only for that snapshot's revision and digest until it expires; it can reorder discretionary work and nothing else.
_Avoid_: Jev verdict, prediction, signal

**Shadow mode**:
Running a component so that it records what it would do without affecting trading.
_Avoid_: Dry run, paper mode
