# seqscope: filter and decode the Arbitrum sequencer feed

`seqscope` reads every transaction on the sequencer feed, decodes it (including calls inside multicalls, Multicall3, smart
accounts and ERC-4337 bundles), judges each call against rules you write, and keeps what matches in a local database.
It needs no bot, no key and no RPC. Spec and roadmap: `docs/tasks/sequencer-decoder-app.md`. Milestone 1 (engine,
storage, CLI) is built; the web UI and desktop shell are not.

Build and run through the project container: `scripts/dev.sh cargo build --release -p seqscope-cli`, binary
`target/release/seqscope`.

## Five minutes

```
seqscope rules template                       # starter templates
seqscope rules template aave-lending > seqscope.toml
seqscope rules check seqscope.toml            # compiles it; mistakes come with a place and a hint
seqscope tail -w "kind == swap && swap.amount_in >= 1e21"     # watch live, nothing saved
seqscope record -o rec.jsonl --secs 60        # save 60 s of the feed
seqscope rules test -f rec.jsonl -w "protocol == aave_v3"     # how many would match, with examples
seqscope run                                  # collect: feed -> rules -> seqscope.db (reloads the rules file when it changes)
seqscope stats; seqscope gaps                 # what is stored; what the decoder still cannot read
seqscope query --kind swap --minutes 10       # newest first
seqscope sql "SELECT to_addr, count(*) FROM v_rows WHERE kind='swap' GROUP BY 1 ORDER BY 2 DESC LIMIT 10"
seqscope decode 0xA51a... 0x3593564c...       # every action inside one call
```

## Rules

A rule file is TOML: `[[profile]]` blocks (each judged independently; `mode = "all"` applies every matching rule, `"first"`
stops at the first), each with `[[profile.rule]]` entries:

```toml
[[profile.rule]]
name   = "big borrows"
when   = "protocol == aave_v3 && selector == 0xa415bcad && input contains 0xaf88d065e77c8cc2239327c5edb3a432268e5831"
action = "alert"      # store (default) | raw (keeps calldata) | alert | count | drop
tag    = "borrow"
sample = 1            # keep one match in N (by transaction hash: reruns agree)
```

`when` is a boolean expression: `&&` `||` `!` (or `and` `or` `not`), parentheses, `== != < <= > >=`, `field in [a, b]`,
`field contains x`, `set(field)`. Numbers are whole: `5e6`, `1_000`, `0xff`, `-60`. Fields: `seqscope catalogue`.

* A **call** is the unit judged: the transaction itself, or one call inside a wrapper. `via` lists the wrappers
  (`multicall3`, `self_multicall`, `account_execute`, `entrypoint_v6`, `entrypoint_v7`); `depth == 0` means called directly.
* `kind` is `swap`, `liquidity`, `liquidation`, `undecoded_known` (an identified contract nothing reads yet) or `unknown`.
* A comparison on a field the call does not have (`swap.amount_in` on a liquidation) is false, including `!=`; use `set(...)`.
* Every row says `status = pending`: calldata is an intent. The transaction may revert, and the free feed has no receipts.

## What it costs

Measured in release (`cargo run --release -p seqscope-core --example bench`, `-p seqscope-store --example load`):
2.0 microseconds per transaction with 100 rules on an unknown contract, 4.6 on a Universal Router V4 swap (decoding
included); the database takes 10,000 transactions/s (10x the feed's peak) storing every one with no drops, and a full
queue drops rows *and counts them* (the status line shows `dropped`).

## Testing

`scripts/test-seqscope.sh` runs everything, offline, in the container, with no input from anyone:

| Layer | What it protects |
|---|---|
| `mev-decode` unit + `calls` tests | wrapper unwrapping: Multicall3, router multicalls, smart accounts, ERC-4337 v0.6 and v0.7, depth and width bounds |
| `seqscope-core` unit + engine tests | the rule language (parser property tests), every compiler error message, engine semantics, the rule file |
| `seqscope-core` corpus tests | 2,049 real feed transactions (`tests/fixtures/corpus.jsonl`): invariants for every call, decoder accuracy against the chain (receipts in `corpus_chain.json`: 75/75 swaps name a pool the transaction really swapped in, 12/12 liquidity changes have an event), a golden summary of what the decoder reads, mutation fuzzing of 24,000 damaged inputs, engine properties |
| `seqscope-store` tests | lossless round trip, filters, read-only SQL, `written + dropped + failed = sent` under pressure, persistence, schema versions, pruning, reading while writing |
| `seqscope-cli` end-to-end tests | the real binary: every offline command and exit code, deterministic output, and the collector run against a local fake sequencer feed with a SIGTERM stop |
| performance and load guards | under 20 microseconds per transaction with 100 rules; 10,000 tx/s with zero drops |

`--live` adds a smoke test against the real feed (record 30 s, every template over the recording, database invariants, a
20 s collector run that must drop nothing). `scripts/mutation-check-seqscope.sh` plants ten known bugs one at a time and
fails if the suite does not notice any of them. After a deliberate decoder improvement the golden summary fails on
purpose: regenerate it with `UPDATE_GOLDEN=1 scripts/test-seqscope.sh`, read the diff, commit it. To refresh the corpus,
record a new one with `seqscope record`, list the decoded hashes, and run `scripts/build-corpus-truth.py`.
