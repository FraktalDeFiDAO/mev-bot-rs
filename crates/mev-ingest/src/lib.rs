//! Live event ingestion: WebSocket log subscriptions for pool state changes.

pub mod fast_feed;

use alloy::{
    primitives::Address,
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use futures::Stream;
use mev_bindings::{IPancakeV3Pool, IPoolManager, IUniswapV2Pair, IUniswapV3Pool};

/// Stream of V2 `Sync`, V3/Algebra `Swap`, PancakeV3 `Swap` and Uniswap V4 `Swap` logs for `pools`
/// (for V4, the PoolManager's address; see `PoolStore::log_addresses`). The stream ends if the WS connection drops;
/// callers should resubscribe and resync state.
pub async fn pool_logs(
    ws: &DynProvider,
    pools: Vec<Address>,
) -> Result<impl Stream<Item = Log> + use<>> {
    let filter = Filter::new().address(pools).event_signature(vec![
        IUniswapV2Pair::Sync::SIGNATURE_HASH,
        IUniswapV3Pool::Swap::SIGNATURE_HASH,
        IPancakeV3Pool::Swap::SIGNATURE_HASH,
        IPoolManager::Swap::SIGNATURE_HASH,
    ]);
    let sub = ws
        .subscribe_logs(&filter)
        .await
        .context("log subscription failed")?;
    Ok(sub.into_stream())
}
