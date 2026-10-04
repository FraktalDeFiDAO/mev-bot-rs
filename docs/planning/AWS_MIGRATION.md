# Move the bot to AWS us-east-2 (next to the Arbitrum sequencer)

Status: **planned** (2026-10-04). Decision record: DECISIONS #44 once executed.

## Why

| Measured from | Sequencer gateway RTT (warm) | Feed stalls |
|---|---|---|
| Contabo, Lauterbourg FR (now) | **110 ms** | every ~10–15 min on 2026-10-04 |
| AWS us-east-2 (Ohio) | ~1–2 ms expected (`arb1-sequencer-gateway.ue2…`) | to measure |

Every race pays the gap twice (feed in, tx out). After the presim/backrun work, 5/5 live arbs lost
**in the same block** to faster bots — this is now the biggest remaining handicap.

## Sizing (measured on the current server)

Bot: **5.5 % of one CPU, 266 MB RAM**. Runtime image 685 MB. Builds need ~4 GB RAM → build off-box.

| Choice | Value | Why |
|---|---|---|
| Instance | **t3.small** (2 vCPU, 2 GB, x86_64), credit spec **standard** | Runtime fits with headroom; standard mode = no surprise surplus-credit charges (baseline 20 %/vCPU ≫ 5.5 %). Free-plan eligible. x86 because the image is Arch (no official arm64). |
| OS | Ubuntu 24.04 LTS (x86_64) | Same podman 4.9 + podman-compose as Contabo → `run.sh`/`deploy.sh` unchanged |
| Disk | 20 GB gp3, encrypted | images + logs + index caches |
| Address | 1 Elastic IP | stable SSH target / `SERVER=` for deploys |
| AZ | try use2-az1/az2/az3, keep the lowest RTT to the sequencer | ~1 ms differences matter |
| Metadata | IMDSv2 required, hop limit 2 | security default; bot doesn't use IMDS |

**Cost (on-demand, us-east-2):** t3.small $15.2 + gp3 20 GB $1.6 + public IPv4 $3.6 ≈ **$20.4/month**
→ ~5 months on the $100 Free-plan credits (expire 2027-04-04). Add an AWS budget alert at $25/month.
Bigger options if ever needed: c7i-flex.large (4 GB) $61.9/mo, m7i-flex.large (8 GB) $69.9/mo.

## Access

- Security group: inbound **TCP 22 from the operator's current IP only**; nothing else inbound.
  The bot needs only outbound (RPCs, sequencer, feed).
- SSH key: a new ed25519 key pair for this host; `~/.ssh/config` alias `mev-ohio`.
- Fallback: SSM Session Manager (instance profile `AmazonSSMManagedInstanceCore`) if SSH breaks.

## Build and deploy change

The 2 GB instance can't compile the workspace, so `scripts/deploy.sh` gains a build-locally path:
build `mev-bot:<sha>` on the workstation (podman), `podman save | zstd | ssh podman load` to the
server, then the server checks out the same commit (config, scripts) and restarts without
building. Same guarantees as today: main only, clean tree, equal to origin/main, gate first, tag.

## Cutover (no double-live, no gap > a few minutes)

1. Launch + harden the instance; install podman/podman-compose; read-only GitHub deploy key; clone.
2. User copies `.env` (chmod 600). Copy `data/arbitrum` index caches (saves hours of backfill).
3. Run **shadow** in Ohio next to live Contabo for ~30 min: measure sequencer RTT, presim latency,
   feed stalls, pool-update lead.
4. If better: stop Contabo (`scripts/run.sh stop`), start Ohio **live** (`deploy.sh live --yes`
   with `SERVER=` the Ohio host). One wallet, one live bot, always.
5. Keep Contabo checkout as rollback for 2 weeks (`run.sh live` there to roll back).

## Risks

- Free-plan **spend limit** can pause the project → bot down. Mitigation: budget alert; watch credits.
- Credits run out ~2027-03 at this size → decide on paid plan only if the bot is profitable.
- Public RPC endpoints may rate-limit AWS IP ranges differently than Contabo; shadow run checks it.
