# Subagent preamble

Prepend the block below to any subagent prompt that explores, implements, reviews, or debugs.
Subagents start cold: no `CLAUDE.md`, no hooks, none of the parent's context.
Pass identifiers (paths, symbols, node ids), never pasted content.

```
Repo: Rust Cargo workspace (crates/, one crate per component) + Foundry contracts (contracts/).
Plans: docs/planning/README.md. Do not read all plan docs; open only the one you need.

If graphify-out/graph.json exists, query it before reading source:
  graphify query "<question>" --budget 800
  graphify explain "<symbol>"     graphify path "<A>" "<B>"     graphify affected "<X>"
  docs/knowledge/Files.md          # source path -> communities

Escalate one step at a time: graph -> one hop -> community note -> `rg -n` symbol ->
read that range -> whole file last. Never read the repo broadly for background.

Keep command output short: cargo check -q --message-format=short; cargo test -q -p <crate>.

Report back a delta, not a narrative: files/symbols changed, decisions and why, what failed,
what remains. Do not restate context the caller gave you.
```
