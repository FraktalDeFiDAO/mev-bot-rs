# Contributing / Git workflow

Trunk-based development with short-lived branches and protected `main`.

## Setup

```bash
scripts/setup-dev.sh   # enables hooks: conventional commits, secret scan, fmt, graph update
```

## Containers (podman)

Everything builds and runs in podman, following the conventions in `~/projects/AI_Chatbot`
(pinned Arch base, multi-stage build, `userns_mode: keep-id`, read-only rootfs, `cap_drop: ALL`).

```bash
scripts/dev.sh cargo test -q -p mev-dex                 # any cargo command in the toolchain container
scripts/dev.sh bash -c 'cd contracts && forge test'    # Foundry
scripts/run.sh shadow | live | status | logs | halt | resume | stop
```

## Branches

- `main` is always deployable. Direct pushes are blocked by the local `pre-push` hook; everything lands via PR.
  (Server-side rules need GitHub Pro for private repos — see "Server-side protection" below.)
- Branch from `main`, keep branches small and short-lived (≤ 2–3 days):

| Prefix | Use |
|---|---|
| `feat/<crate>-<short-desc>` | New capability (`feat/mev-liquidate-aave-v3`) |
| `fix/…` | Bug fix |
| `perf/…` | Latency/gas optimization |
| `refactor/…`, `test/…`, `docs/…`, `ci/…`, `chore/…` | As named |
| `security/…` | Contract or key-handling hardening |
| `hotfix/…` | Urgent fix for a live loss; same PR flow, expedited review |

Rebase on `main` (`git pull --rebase origin main`); don't merge `main` into branches.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), scope = crate or area:

```
feat(mev-liquidate): add Aave v3 health-factor monitor
fix(contracts): verify flash-loan callback initiator
perf(mev-sim): cache pool storage slots between sims
docs(planning): record node provider decision
```

`!` after the scope marks a breaking change. Enforced by the `commit-msg` hook.

## Pull requests

- PR title must be a Conventional Commit (CI-checked) — it becomes the squash commit message.
- **Squash merge only**; linear history; branch auto-deleted after merge.
- Required to merge: `ci-ok` check green (fmt, clippy, tests, forge, gitleaks), PR template filled, conversations resolved.
- Changes to `contracts/`, `mev-exec`, or `mev-risk` move funds: require fork tests and an explicit
  security pass in the PR description (see CODEOWNERS).

## Releases & deploys

- Tag `main` with SemVer: `git tag -s v0.3.0 -m "v0.3.0" && git push origin v0.3.0`.
- Only tagged commits are deployed to the live bot. Contract deployments record address + tag in
  `docs/planning/DECISIONS.md`.
- Rollback = redeploy the previous tag.

## Never commit

Private keys, mnemonics, RPC URLs containing API keys, `.env` files. Config takes secrets from env vars.
The pre-commit hook and CI gitleaks job block common patterns; they are a backstop, not a guarantee.

## Server-side protection

GitHub's free plan does not enforce branch rules on private repos. Until the account is upgraded to
GitHub Pro, protection is local: the `pre-push` hook refuses pushes to `main`, and CI must be green
before you merge (check with `gh pr checks`). After upgrading, apply the ruleset in
`.github/rulesets/protect-main.json`:

```bash
gh api -X POST repos/FraktalDeFiDAO/mev-bot-rs/rulesets --input .github/rulesets/protect-main.json
```
