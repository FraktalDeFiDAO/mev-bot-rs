//! Live event ingestion: WebSocket log subscriptions for pool state changes.

pub mod fast_feed;
pub mod lead;

use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
};
use anyhow::{Context, Result};
use futures::Stream;

/// Messages the log subscription may buffer before the oldest is dropped: many blocks of the busiest bursts.
pub const LOG_BUFFER: usize = 32_768;
// alloy's default is 16 and drops silently; a block's logs arrive one message each (50-150, up to a few hundred in the
// busiest), and the engine can be busy for several blocks. Keep two orders of magnitude of headroom, checked at build.
const _: () = assert!(LOG_BUFFER >= 300 * 100);

/// Stream of every pool event the decoder knows (swaps, liquidity changes, V2 `Sync`, fee collection, flash loans, V4
/// pool creation and donations) for `pools`
/// (for V4, the PoolManager's address; see `PoolStore::log_addresses`). The stream ends if the WS connection drops;
/// callers should resubscribe and resync state.
pub async fn pool_logs(
    ws: &DynProvider,
    pools: Vec<Address>,
) -> Result<impl Stream<Item = Log> + use<>> {
    // Every event a pool emits that the decoder knows, not only the ones that change our state: the journal records
    // them all (V2 swaps and liquidity, fee collection, flash loans, V4 pool creation and donations as well). The
    // engine applies only the state-relevant kinds.
    let filter = Filter::new()
        .address(pools)
        .event_signature(mev_decode::events::pool_event_topics());
    // alloy's subscription buffer defaults to 16 messages and, when the consumer falls behind, silently drops the oldest
    // (logged at debug only). A block's burst is 50-150 logs, delivered one message each, so the default lost events:
    // measured as ~0.3% of tracked-pool events missing from the journal, all inside busy blocks (DECISIONS #109).
    let sub = ws
        .subscribe_logs(&filter)
        .channel_size(LOG_BUFFER)
        .await
        .context("log subscription failed")?;
    Ok(sub.into_stream())
}
