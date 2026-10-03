# mev-bot

L2 MEV bot (arbitrage + liquidations via flash loans). No sandwiching.

- **Resume here:** `.ai/STATE.md` (handoff) → `docs/planning/README.md` (plan index). Don't reconstruct history from transcripts.
- Planning docs live in `docs/planning/`; record decisions in `docs/planning/DECISIONS.md`.
- Rust only off-chain; Solidity/Foundry only for the executor in `contracts/`.
- Every component is its own crate under `crates/` (`docs/planning/CRATES.md`); dependencies point only to lower layers.
- **Podman is the standard** (same conventions as ~/projects/AI_Chatbot): build/test with `scripts/dev.sh <cmd>` (Rust + Foundry toolchain container), run the bot with `scripts/run.sh shadow|live|status|halt|stop` (compose, read-only, caps dropped). Don't run the bot directly on the host.
- Budget: $200 starting capital, no owned server — keep infra ~$0/month until profitable.

## Token discipline

Every read is re-sent on every later turn. Before any Read, `cat`, or broad grep: do I need the file, or one fact from it?
Follow the retrieval ladder in `.ai/CONTEXT_POLICY.md` (graph → one hop → community note → `rg -n` → line range → whole file last).

- Always pass `--budget` to `graphify query` (300–500 one fact, 500–1000 a question, 800–1500 to implement).
- Quiet commands: `cargo check -q --message-format=short`, `cargo test -q -p <crate>`, `cargo clippy -q --message-format=short -- -D warnings`.
- Subagents: prepend `.ai/SUBAGENT_PREAMBLE.md`; pass paths/symbols, not content. Use Sonnet for mechanical work.
- At a breakpoint, rewrite `.ai/STATE.md` and start a fresh session instead of running long.

## Knowledge graph & Obsidian

- The repo root is an Obsidian vault. `docs/knowledge/` is generated from the graph — never hand-edit.
- After code changes: `scripts/graph.sh optimize` (AST update + reflect + Obsidian projection; no LLM cost).
- Record graph answer quality: `graphify save-result --outcome useful|dead_end|corrected`, then `scripts/graph.sh reflect`.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
