# Task: a standalone sequencer decoder application (CLI + desktop GUI + web UI)

Status: prompt drafted 2026-10-09, not started. Paste everything below the line into a fresh session as the task prompt.

---

## Role and goal

You are a senior Rust engineer working in the `mev-bot` repository (read `CLAUDE.md`, `.ai/STATE.md`, `docs/PROGRESS.md`, `docs/design/protocol-crates.md`, `docs/design/watched-protocols.md` first). Build **`seqscope`**, a separate application that does one thing: read every transaction on the Arbitrum sequencer feed, decode it, filter it by user-defined rules, store what matches in a very fast database, and let a human tune the rules easily. It must run with no bot, no engine, no private key and no RPC key. The bot itself is not changed by this task.

The user is tuning decoding by hand, constantly: "show me every Aave `borrow` over 50k USD on these markets, every Uniswap V4 `modifyLiquidities`, everything sent to this router that I cannot decode yet." The product is the speed of that loop: change a rule, see the effect on live data in under a second.

## What already exists (reuse it, do not rewrite it)

- `crates/mev-sequencer`: connects to the feed (`wss://arb1.arbitrum.io/feed`), parses broadcast messages into `SequencedTx` (hash, seq, `to`, `input`, signer, gas, value), tracks sequence continuity and resumes from a sequence number. Free feed: transaction bodies only, no receipts, no state.
- `crates/mev-decode` (the registry) + `crates/mev-protocol` (the `Protocol` trait, `Decoded::{Swap,Liquidity,Liquidation}`, `PoolRef`, `EventKind`, `v4::{pool_id, stand_in}`) + one pure crate per protocol (`mev-protocol-uniswap-{v2,v3,v4,router}`, `-pancake-v3`, `-balancer`, `-aave-v3`, `-morpho-blue`, `-compound-v3`, `-fluid`, `-unidentified`).
- `mev_decode::calls::{calls_in, actions_in, decode_all}`: unwraps Multicall3, router multicalls, smart-account `execute`/`executeBatch` and ERC-4337 `handleOps`, giving every leaf call with the wrappers it came through. `Protocol::decode_calls` returns several intents per call.
- `mev_decode::events::{kinds, describe, pool_event_topics}`: event table and named-field decoding for logs.
- `mev_decode::known_contract` / `contracts()`: which destinations are identified, and whether their calls are decoded. `undecoded_known` in the bot's feed journal is the list of identified-but-unread calls.
- Today `mev-decode` is a small CLI (`calldata`, `events`, `feed`). `seqscope` supersedes its `feed` command and reuses its library.

## What to build

### 1. Core library (`seqscope-core`), the only place logic lives

- **Pipeline:** feed -> normalise -> decode (all leaves via `actions_in`) -> classify -> rule engine -> sink(s). Every stage is a pure function over data where possible; the feed is an injectable source (live feed, a recorded file, or a synthetic generator) so everything is testable offline and replayable at any speed.
- **Rule engine (the heart).** A rule is data, not code: `{name, enabled, match, action}`.
  - `match` is a boolean expression tree (AND / OR / NOT) over predicates on: destination address (exact, set, label, "any known contract of protocol X"), sender, **function selector** (hex, or name resolved from the protocol crate), **tx type** (legacy, EIP-1559, Arbitrum internal/retryable/deposit types), calldata length, calldata contains address/bytes, `value`, gas limit / fee, nonce, **wrapper path** (`via` contains `multicall3`, depth), **decoded kind** (swap / liquidity / liquidation / undecoded-known / unknown), **protocol** (`uniswap_v3`, `aave_v3`, ...), **decoded fields** (token in, pool, amount in with units, tick range, borrower, market id, health-related fields), and thresholds on USD value where a price source is configured.
  - `action`: store, store-with-tag, drop, sample 1-in-N, alert (webhook / desktop notification), count-only, and "store raw calldata only".
  - Rules are ordered, first-match or all-match (per rule set), grouped into named **profiles** (e.g. `aave-borrows`, `v4-liquidity`, `everything-unknown`), switchable live.
  - Per-protocol, per-function **decode settings**: for each (protocol, selector) the user can enable/disable decoding, choose which fields to extract and store, set display units/decimals, and mark "ignore" so noise (approvals, transfers) never reaches storage. Defaults come from the protocol crates, so a new protocol crate shows up automatically with sane settings.
  - Rules live in one human-readable file (TOML) AND can be edited through the UIs; the file and the UI always agree (single source of truth, hot reload with validation errors shown, never a crash on a bad rule). Version the rule file; keep an undo history.
  - Compile rules to a fast predicate form at load; the matcher must stay out of the way of the feed (see targets).
- **Self-description:** the core exposes a machine-readable catalogue (protocols, contracts with labels, functions with selectors and field schemas, event kinds, wrapper types) that the UIs use to build forms, autocomplete and validation. Nothing about protocols is hard-coded in a UI.
- **Honest semantics:** calldata decoding is an *intent* (the tx may revert, and the free feed has no receipts). Every stored row says so (`status: pending`), and the app can optionally reconcile with chain logs later (`confirmed` / `reverted` / `unknown`) when an RPC is configured. Never present an intent as an outcome.
- **Gaps are first-class:** transactions to identified contracts that no decoder reads, and transactions to unidentified destinations ranked by volume, are always one click away. This is how the decoder gets better.

### 2. Storage (decide, justify, benchmark)

Requirements: sustained ingest of at least several thousand rows/s with bursts 10x that, no feed backpressure, durable across crashes, queryable while ingesting, single machine, no server to run, bounded disk (retention by age/size per profile).

Evaluate and choose, with a short written comparison and a micro-benchmark on real recorded feed data (the repo has `feedtx-*.jsonl` shapes; the free feed is ~200 messages/s, ~500-1,000 tx/s peak): an append-only segmented log + columnar files (Parquet/Arrow) queried with DuckDB; DuckDB alone; SQLite (WAL) with strict batching; an embedded KV (redb / RocksDB / LMDB) for the hot window plus an analytical store for history; QuestDB/ClickHouse (reject if they need a server unless the margin is large). My expectation, to be confirmed or overturned by your benchmark: a lock-free bounded channel into a single writer thread that batches into an append-only log and a hot in-memory ring for the live view, with a columnar/DuckDB layer for history and ad-hoc SQL. Whatever you pick: schema is explicit and versioned, writes are batched and never block the feed (drop-and-count with a visible counter if ever overloaded, never silent), and the user can run arbitrary read-only SQL against it.

### 3. Three front ends over one API

Do **not** build three apps. Build one local server in the core that exposes a typed API (HTTP + WebSocket for live streaming), and make each front end a client of it.
- **CLI (`seqscope`)**: `run` (headless collector), `tail` (live, filtered, human or JSON lines), `rules` (list / add / edit / test / enable / disable / import / export), `decode <to> <input>` (one-off, shows all wrapped actions), `query` (SQL or saved queries, table/JSON/CSV out), `replay <file> --speed N`, `stats`, `gaps`, `catalogue`. Every flag also available via config file; `--help` is excellent; shell completions.
- **Web UI** (served by the same binary, localhost by default, embedded static assets, no CDN, no telemetry): the primary tuning surface. Live table with virtual scrolling and pause; **rule builder** that needs no syntax knowledge (pick protocol -> function -> field -> operator -> value, with autocomplete from the catalogue, plain-language preview, and "what would this match over the last N minutes / on this recorded file" with counts and samples *before* saving); per-protocol decode-settings screens; a transaction inspector (raw calldata, decoded fields, wrapper tree, link to explorer); charts (rate by protocol, by function, gaps); saved views; export. Dark and light. Keyboard-first. Usable at a glance by someone who has never read the rule file.
- **Desktop GUI**: a thin shell that embeds the web UI (Tauri or equivalent) plus native niceties (tray, notifications). Only build it after the web UI is good; if it adds more than a few hundred lines of code beyond packaging, stop and say so.

### 4. Make it easy (explicit design goals)

- Zero-config start: `seqscope run` connects, decodes everything, stores nothing but counters until the user picks a profile; a "start from template" list (Aave lending actions, Uniswap V2/V3/V4 swaps, liquidity changes, liquidations, undecoded-known, unknown high-volume destinations).
- Every rule edit gives instant feedback on live and recent data; every validation error says which field and why.
- Sensible presets, no required knowledge of selectors (but selectors always visible and searchable).
- Everything the UI can do, the CLI and file can do, and the reverse.

## Constraints (from the repo; non-negotiable)

- Rust off-chain only; one crate per component under `crates/`, dependencies point only to lower layers (`docs/planning/CRATES.md`). Suggested: `seqscope-core` (rules, pipeline, catalogue), `seqscope-store`, `seqscope-server` (API), `seqscope-cli`, `seqscope-web` (frontend assets), `seqscope-desktop`. `mev-decode` and the protocol crates stay serde-light and untouched except to add what the catalogue needs (field schemas, labels) in a backward-compatible way.
- Build and test through `scripts/dev.sh` (Podman); never run on the host; no secrets in code, logs or the UI; never print or store API keys; do not read `.env`.
- Integer math for amounts (U256 / i128), no floats in decoding; USD values are display-only and labelled as such.
- No fabricated data: unknown stays unknown; a decoder that cannot read a call says so.
- Tests are required: rule-engine semantics (property tests for boolean algebra and ordering), round-trip of the rule file, every stored schema migration, replay determinism (same file + same rules -> byte-identical output), the fixtures of real transactions already in `crates/*/tests/fixtures` run through the whole pipeline, and a load test at 10x feed rate showing no feed stall and a correct drop counter.
- Follow the workflow: branch -> small PRs -> gate (`scripts/check.sh`) -> decision rows in `docs/planning/DECISIONS.md`; update `docs/PROGRESS.md` and `docs/planning/CRATES.md`. Stop for review after the core + CLI + storage (milestone 1) before building any UI.

## Performance targets (verify, do not assume)

- Rule evaluation: under 20 microseconds per transaction for 100 active rules on a laptop core, zero allocation on the non-matching path.
- Feed to stored row (batched): p99 under 50 ms; the feed reader is never blocked by storage or UI.
- Web UI stays smooth at 1,000 rows/s of matches (virtualised, batched updates).
- Cold start to first decoded transaction: under 3 seconds.

## Milestones and what to report back

1. Core + rule engine + replay + CLI + storage chosen and benchmarked. Report the storage comparison, the benchmark numbers, and the rule file format with three worked examples (an Aave `borrow` filter, a Uniswap V4 `modifyLiquidities` filter, an "undecoded-known" filter).
2. Server API + web UI (live view, rule builder with dry-run preview, inspector, gaps).
3. Decode-settings screens and saved views; alerting.
4. Desktop packaging (only if cheap).
5. Optional reconciliation with chain logs.

Ask me before: choosing a storage engine that needs a running server; adding a dependency with a non-permissive license; anything that touches the live bot, its config, or its server.

## Questions to answer in your first message (before writing code)

1. Your chosen storage design and why, in five lines.
2. The exact rule-file schema (a full example).
3. Which parts of `mev-decode` need extending for the catalogue (field schemas, labels, per-function metadata) and how you will keep that backward compatible.
4. Anything in this spec that is ambiguous, contradictory or too large for one milestone, and your proposed cut.
