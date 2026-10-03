# Contributing / Git workflow

Trunk-based development with short-lived branches and protected `main`.

## Setup

```bash
scripts/setup-dev.sh   # enables hooks: conventional commits, secret scan, fmt, graph update
```

## Branches

- `main` is always deployable. Direct pushes are blocked; everything lands via PR.
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
