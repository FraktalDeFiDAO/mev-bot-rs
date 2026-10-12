# Live cutover: audited revision (H01 / M11)

Prepared 2026-10-11. **Nothing here has been executed.** Every step that touches `mev-ohio`, a key, or funds is the
operator's. Context: `docs/audits/2026-10-11-complete-audit.md`, status in `docs/audits/2026-10-11-remediation.md`.

Why this exists: live mode now refuses to start without a durable risk ledger that names the wallet and the block it was
reconciled at (`Ledger::persistent_existing`). Production has no such file (`risk-state.json`, `tip-state.json`,
`gate-state.json` were absent when inspected), and the running image predates the accounting, nonce, and guard fixes.
Deploying the new image without seeding the ledger will fail closed (no live sends) — safe, but not useful.

## 0. Preconditions (local)
1. The remediation branch is reviewed, merged to `main` through a PR, and `scripts/check.sh` is green. `scripts/deploy.sh`
   refuses anything else.
2. Contracts: the `ArbExecutor` / `AtlasSolver` changes are source-only until you deploy them. The bot works with the
   currently deployed executor in shadow. Decide separately whether to redeploy the executor (`scripts/deploy-executor.sh`;
   owner key, owner decision). Do **not** enable `[atlas] bid`; the bot refuses it.
3. Pick a conservative native price for the ledger (`--native-usd`): use a quote at least as high as the day's maximum.

## 1. Freeze and capture (production host, read-only except STOP)
```
ssh mev-ohio
cd ~/src/mev-bot && scripts/run.sh halt          # kill switch: no new sends; process keeps running
scripts/run.sh status                            # record container image id, last summary line
git rev-parse HEAD > /tmp/prod-rev               # evidence of what was running
grep -h '"kind":"sent"' data/arbitrum/events-*.jsonl | jq -r '.data.hash' | sort -u > /tmp/sent-hashes.txt
wc -l /tmp/sent-hashes.txt
```
Keep `data/arbitrum/events-*.jsonl` (the `sent` events are the only record of which transactions the wallet signed).

## 2. Seed the ledger (read-only RPC, writes one candidate file)
Run from the workstation (any host with a mainnet Arbitrum RPC; the key is not needed, only the **public address**):
```
scripts/dev.sh cargo run -q --release -p mev-analytics --bin mev-ledger-seed -- \
  --rpc "$ARBITRUM_HTTP_URL" --owner 0x<hot-wallet-address> --hashes sent-hashes.txt \
  --native-usd <conservative-quote> --out data/arbitrum/risk-state.json
```
The tool reads one canonical block, verifies each receipt is on the canonical chain, requires contiguous evidence for
every nonce back to the start of the current UTC day, turns mempool transactions into pending liabilities, and refuses
(with the offending nonce) on any hole, foreign sender, or missing receipt. It never replaces an existing file.
Verified read-only against Arbitrum One on 2026-10-11: with stale hashes it refused with
`no transaction supplied for nonce 732 ... add its hash to the list` rather than seeding an incomplete day.
Review the output (`jq . risk-state.json`: `identity`, `reconciled_at`, `spent_microusd`, `pending`) and copy it to
`data/arbitrum/risk-state.json` on the host (the operator copies files; this repo never moves secrets or state).

## 3. Deploy in shadow first
```
scripts/deploy.sh shadow          # from main, clean tree, equal to origin/main
```
Soak in shadow for at least one UTC day. Acceptance:
- `scripts/run.sh status` healthy; heartbeat fresh; no `exact_reject` storm, no `persistence` blocks.
- Restart the container once (`scripts/run.sh stop && scripts/run.sh shadow`) and confirm the ledger/gate/tip files are
  reloaded (startup log shows accounting restored, not reset).
- Record the image digest and `git rev-parse HEAD` next to the audit evidence.

## 4. Watchdog (M11)
```
scripts/install-watchdog.sh       # on the host; systemd user timer
systemctl --user list-timers mev-watchdog.timer
```
Confirm `data/arbitrum/ALERTS.log` stays empty and put a webhook URL in `data/arbitrum/alert_webhook` if you want pushes.

## 5. Live (owner go required)
```
scripts/deploy.sh live --yes
```
Immediately after: `scripts/run.sh status`, then watch `sent` / `exact_requote` / `risk_block` events for the first hour. The
global stop (30 unsuccessful attempts, hourly probes) is on in `config/arbitrum.toml`.

## Rollback
`scripts/run.sh halt` first (stops sending). To return to the previous image: redeploy the recorded previous revision
with `scripts/deploy.sh` from its tag (`deploy-<UTC time>`). The old Contabo host stays stopped; never run two live bots on one wallet.
The ledger is append-only evidence: do not delete `risk-state.json` to "reset" it — a missing file blocks live by design.

## Not covered here
Atlas bidding (disabled by construction), executor redeploy, key handling and funding (owner only).
