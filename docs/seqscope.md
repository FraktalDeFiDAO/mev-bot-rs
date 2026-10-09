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
