# mev-decode

Standalone decoding of Arbitrum transactions and events. No RPC, engine or pool-state dependency.

```
mev-decode calldata <to> <0x-input>       decode one call
mev-decode events                         the event shapes it recognises (JSON lines)
mev-decode feed [--decoded-only] [--limit N] [--url wss://...]    decode the live sequencer feed
```

As a library (`default-features = false` drops the command-line dependencies):

```rust
let call = mev_decode::decode_call(to, &input);        // Option<Decoded::Swap(SwapIntent)>
let kind = mev_decode::events::classify(&topic0);      // swap / liquidity / liquidation / state
let price = mev_decode::events::price_update(kind?, &log_data);
let who = mev_decode::known_contract(to);              // identified destination, decoded or not
```

What is decoded and what is not: `docs/design/watched-protocols.md`. Decoded calldata is an *intent*; a feed
transaction may revert, so prices for charts come from event logs.
