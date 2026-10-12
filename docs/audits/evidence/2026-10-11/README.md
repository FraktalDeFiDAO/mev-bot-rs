# Audit evidence — 11 October 2026

This directory contains isolated reproductions and sanitized results for the [audit report](/home/admministrator/projects/mev-bot/docs/audits/2026-10-11-complete-audit.md). The tests reproduce defects in the reviewed production source; they do not fix them. The new harnesses use no RPC, production keys, signing, or live transactions. The separate integration tests make read-only RPC calls.

## Captured results

- [Verification metadata](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/verification.json): revision identifiers and command exit results.
- [Production summary](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/production-summary.jsonl): allowlisted configuration, file existence, monitoring and journal aggregates at 13:17:33 UTC. This is not raw production configuration or proof of image provenance.
- [Rust gate](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/rust-check.log): formatting/lint passed; 493 tests passed, zero failed, 16 ignored.
- [Solidity build/offline tests](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/forge.log): runtime sizes and 25 offline tests. Full `forge test -q` separately exited zero; its quiet log was empty.
- [Browser suite](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/browser.log): 23 logic and 17 Chromium checks; clean server shutdown with 2,861 rows stored, zero dropped/failed.
- [Rust reproductions](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/repro.log): budget, pending state, NaN, nonce and redaction assertions; parser child aborted as expected with exit 134.
- [Atlas reproductions](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/atlas-repro.log): three tests passed while asserting the existing defective behavior.
- [Dependency scan](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/dependency-scan.json): 603 locked dependencies, zero vulnerability advisories; two unmaintained notices. Initial attempt used a read-only advisory path; the completed scan used a writable temporary database.
- [ShellCheck](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/shellcheck.log): six diagnostics. Shell/Python syntax separately passed for 35/10 scripts.

URLs in copied test logs are replaced with `[redacted URL]` to avoid retaining provider endpoints or credentials. No credential scan matches are copied here. Reproduction logs may include compiler/test diagnostics; a stack-overflow message is an expected result.

## Rust reproductions

Run from the repository root using its normal development container:

```bash
scripts/dev.sh cargo run -q --locked --offline \
  --manifest-path docs/audits/evidence/2026-10-11/repro/Cargo.toml
```

The standalone workspace depends directly on `mev-risk`, `mev-config`, `mev-types`, and `seqscope-core`, and includes the actual nonce manager source. Its own lockfile pins the harness dependencies. Temporary accounting/config files are written only to a process-specific `/tmp/mev-audit-repro-*` directory.

Expected assertions:

| Case | Existing behavior asserted |
|---|---|
| Pending budget | Three concurrent `$0.70` admissions against `$1.20` remaining; settled spend `$3.90` exceeds `$3`. |
| Outstanding backruns | 100 admissions without receipts despite limit of three. |
| Restart | Pending send disappears from saved exposure and another send is admitted. |
| Corrupt snapshot | Spend becomes zero and the entire daily budget is admitted. |
| NaN | Guard admits NaN profit/gas; config loader accepts NaN minimum profit. |
| Nonce reset | Held nonce 7 is allocated again after reset to chain pending count 7. |
| URL redaction | Synthetic path key is removed, synthetic userinfo password survives. |

Run the parser failure in a separate process so its expected abort does not terminate the other checks:

```bash
scripts/dev.sh bash -c '
  cargo build -q --locked --offline --manifest-path docs/audits/evidence/2026-10-11/repro/Cargo.toml || exit
  ulimit -c 0
  /workspace/target/debug/mev-audit-repro expression-depth
  rc=$?
  echo "expression_depth_exit=$rc"
  test "$rc" -eq 134
'
```

This uses a two-MiB thread stack and a 40-KB expression. The expected result is stack overflow, not a caught parse error. Core dumps are disabled.

## Solidity reproductions

```bash
scripts/dev.sh forge test \
  --root docs/audits/evidence/2026-10-11/solidity \
  --match-contract AtlasBalanceAuditTest -vv
```

The isolated Foundry project imports the real `ArbExecutor` and `AtlasSolver`, plus the repository's vault/protocol/Atlas stand-ins. `BalanceCheckedWETH` adds owned-token accounting missing from the existing mock. `/workspace` remappings intentionally match `scripts/dev.sh`; cache and build output go under `/tmp` inside that container.

The three tests assert bid failure after float sweeping, initial float counted as profit, and a losing trade subsidized by that float. Access control is retained; the evidence does not assert unauthorized theft. A future fix should make these bug assertions fail and replace them with preservation/profit-delta tests.

## RPC-backed validation

The first combined run stopped on a missing `MEV_IT_POOLS` fixture. The completed discovery run supplies the two public pool addresses in [pools.txt](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/pools.txt) and the pre-existing `data/v4_registry_seed.json` fixture. Lending runs set `MEV_IT_RPC` and `MEV_IT_LOG_RPC` to the launcher-provided `ARBITRUM_HTTP_URL`, without printing its value.

Detailed logs:

- [Initial combined attempt](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/network-rust.log).
- [Aave retry](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/network-rust-completion.log).
- [Discovery completion](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/discovery-completion.log).
- [Remaining lending/presimulation packages](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/network-rust-remainder.log).
- [Isolated Morpho failure diagnosis](/home/admministrator/projects/mev-bot/docs/audits/evidence/2026-10-11/logs/morpho-failure.log).

Aave completed with two passing tests and one failure: borrower log sampling requests a 20,000-block range exceeding the selected provider's range limit. Compound completed with two passing tests. Morpho completed with one pass and one failure caused by the same provider range restriction; the isolated borrower test reproduced that error. Fluid completed with one passing test. Discovery completed with six passing tests, and presimulation with two. The final additional RPC-backed result is **14 passed and two failed**, excluding earlier attempts and the isolated diagnostic retry.

Tests depend on current market state and existing borrower/registry fixtures. Some test branches explicitly skip missing local caches or unavailable quotes. A passing test process therefore does not prove all venues, quote sizes, liquidation opportunities or RPC failure cases were exercised. No failed RPC-dependent test is counted as a successful release gate.

## Main validation commands

```bash
scripts/dev.sh bash -c 'cargo fmt --all --check && cargo clippy -q --workspace --all-targets --locked -- -D warnings && cargo test -q --workspace --locked'
scripts/dev.sh bash -c 'cd contracts && forge fmt --check && forge build --sizes && forge test'
scripts/test-seqscope-ui.sh
```

The shell/Python syntax checks compile no business logic and perform no deployment. The dependency scan installed `cargo-audit` temporarily inside the development container and selected `--db /tmp/mev-audit-db`.
