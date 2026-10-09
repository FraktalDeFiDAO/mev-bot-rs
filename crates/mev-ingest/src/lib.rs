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
    let sub = ws
        .subscribe_logs(&filter)
        .await
        .context("log subscription failed")?;
    Ok(sub.into_stream())
}
