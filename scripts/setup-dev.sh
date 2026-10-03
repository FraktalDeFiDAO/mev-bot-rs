#!/usr/bin/env bash
# One-time setup after cloning.
set -euo pipefail
cd "$(dirname "$0")/.."
git config core.hooksPath scripts/githooks
git config pull.rebase true
git config fetch.prune true
echo "git hooks enabled (scripts/githooks). Optional: install gitleaks for deeper secret scanning."
