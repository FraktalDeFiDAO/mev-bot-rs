#!/usr/bin/env bash
# Deploy main to the server. Only a clean, gated main that is already on GitHub goes out.
#
#   scripts/deploy.sh shadow          simulate only (no transactions)
#   scripts/deploy.sh live --yes      REAL FUNDS: refuses without --yes
#
# Flow: develop locally -> PR -> merge to main on GitHub -> `git pull` main here -> deploy.
# Steps: checks (on main, clean tree, equal to origin/main), gate (scripts/check.sh in the dev
# container), the server fetches with its read-only deploy key (host alias github-mev-bot) and
# checks out that exact commit (refuses if the server checkout has local edits), saves the old
# container's log to data/logs/, rebuilds and restarts via scripts/run.sh, then tags the commit
# deploy-<UTC time>. The server's data/ and .env are untracked and never touched.
set -euo pipefail
cd "$(dirname "$0")/.."

# Live host: AWS us-east-2 t3.small next to the Arbitrum sequencer (DECISIONS #44). SSH alias
# `mev-ohio` (~/.ssh/config). The old Contabo host is stopped and kept only as a rollback.
SERVER=${SERVER:-mev-ohio}
REMOTE_DIR=${REMOTE_DIR:-/home/ubuntu/src/mev-bot}
# BUILD=local (default): build the image here and stream it to the server; the 2 GB host can't
# compile the workspace. BUILD=server: the server builds it (bigger hosts).
BUILD=${BUILD:-local}

mode="" yes=0
for a in "$@"; do
  case "$a" in
    shadow | live) mode=$a ;;
    --yes) yes=1 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done
[[ -n $mode ]] || { echo "usage: scripts/deploy.sh shadow | live --yes" >&2; exit 2; }
[[ $mode != live || $yes -eq 1 ]] || { echo "refusing to deploy live without --yes (real funds)" >&2; exit 2; }

branch=$(git symbolic-ref --short HEAD)
[[ $branch == main ]] || { echo "deploy from main only (on $branch): merge the PR, then 'git checkout main && git pull'" >&2; exit 1; }
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || { echo "working tree has uncommitted changes" >&2; exit 1; }
git fetch -q origin main
[[ $(git rev-parse HEAD) == $(git rev-parse origin/main) ]] || { echo "local main != origin/main: 'git pull' (or merge your PR) first" >&2; exit 1; }
sha=$(git rev-parse --short HEAD)

echo "== gate ($sha)"
scripts/dev.sh scripts/check.sh

echo "== server: fetch and check out $sha"
ssh "$SERVER" "set -e; cd $REMOTE_DIR; git fetch -q origin
  if [ -n \"\$(git status --porcelain --untracked-files=no)\" ]; then
    echo 'server checkout has local edits: resolve them first' >&2; exit 1
  fi
  git checkout -q -B main $sha; git branch -q -u origin/main main; git log --oneline -1"

if [[ $BUILD == local ]]; then
  echo "== build image locally and stream it to the server"
  podman build -q -t localhost/mev-bot:dev -f Containerfile --build-arg APP_UID=1000 --build-arg APP_GID=1000 . >/dev/null
  podman save localhost/mev-bot:dev | zstd -q -T0 | ssh "$SERVER" 'zstd -dq | podman load -q' >/dev/null
fi

no_build=$([[ $BUILD == local ]] && echo 1 || true)
echo "== server: save old log, rebuild, restart ($mode)"
ssh "$SERVER" "set -e; cd $REMOTE_DIR; mkdir -p data/logs
  podman image prune -f >/dev/null 2>&1 || true   # untagged images from earlier builds filled the disk (2026-10-07)
  if podman container exists mev-bot-arbitrum; then
    f=data/logs/mev-bot-arbitrum-\$(date -u +%Y%m%d-%H%M%S).log.gz
    podman logs mev-bot-arbitrum 2>&1 | gzip > \$f && echo \"   saved \$f\"
    ls -1t data/logs/mev-bot-arbitrum-*.log.gz | tail -n +21 | xargs -r rm -f
  fi
  echo y | NO_BUILD=$no_build scripts/run.sh $mode | tail -1
  podman image prune -f >/dev/null 2>&1 || true"

tag=deploy-$(date -u +%Y%m%d-%H%M%S)
git tag "$tag" "$sha" && git push -q origin "$tag"
echo "== deployed $sha as $tag ($mode). Check: ssh $SERVER 'cd $REMOTE_DIR && scripts/run.sh status'"
