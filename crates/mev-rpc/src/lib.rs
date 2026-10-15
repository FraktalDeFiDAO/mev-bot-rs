//! RPC connections: HTTP reads, WS subscriptions, and batched reads via Multicall3.

use alloy::{
    primitives::{Address, Bytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    sol_types::SolCall,
};
pub mod meter;
pub mod rotate;

use std::{future::Future, sync::Arc, time::Duration};

use alloy::rpc::client::ClientBuilder;
use anyhow::{Context, Result};
pub use meter::{MeterLayer, RpcMeter};
use mev_bindings::{IMulticall3, MULTICALL3};
pub use rotate::RotatingTransport;
use tracing::warn;

#[derive(Clone)]
pub struct Rpc {
    pub http: DynProvider,
    pub ws: DynProvider,
    /// Requests made through `http` (the paid, latency-critical endpoint).
    pub meter: Arc<RpcMeter>,
    ws_urls: Vec<String>,
    /// Index into `ws_urls` of the current WS connection.
    ws_idx: usize,
}

impl Rpc {
    /// Paid reads rotate across all `http_urls` (falling through on rate limits / provider errors);
    /// the WS connects to the first `ws_urls` entry that answers.
    pub async fn connect(http_urls: &[String], ws_urls: &[String]) -> Result<Self> {
        Self::connect_with_fallbacks(http_urls, &[], ws_urls).await
    }

    /// Like [`Rpc::connect`], plus `http_fallbacks` (free public endpoints) that serve reads only
    /// while every keyed endpoint fails or is rate-limited (e.g. its monthly quota is used up).
    /// WS fallbacks go at the end of `ws_urls`: the WS fails over in order on its own.
    pub async fn connect_with_fallbacks(
        http_urls: &[String],
        http_fallbacks: &[String],
        ws_urls: &[String],
    ) -> Result<Self> {
        anyhow::ensure!(
            !http_urls.is_empty() && !ws_urls.is_empty(),
            "need at least one HTTP and one WS URL"
        );
        let meter = RpcMeter::new();
        let http = if http_urls.len() == 1 && http_fallbacks.is_empty() {
            metered_http_provider(&http_urls[0], meter.clone())?
        } else {
            let client = ClientBuilder::default()
                .layer(MeterLayer(meter.clone()))
                .transport(
                    RotatingTransport::http_with_fallbacks(http_urls, http_fallbacks)?,
                    false,
                );
            ProviderBuilder::new().connect_client(client).erased()
        };
        let mut last_err = None;
        for (i, url) in ws_urls.iter().enumerate() {
            match connect_ws(url).await {
                Ok(ws) => {
                    return Ok(Self {
                        http,
                        ws,
                        meter,
                        ws_urls: ws_urls.to_vec(),
                        ws_idx: i,
                    });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap())
    }

    /// Replace the WS provider with a fresh connection. Needed after alloy's own reconnect
    /// gives up (e.g. a multi-minute DNS outage): the old provider is dead for good.
    pub async fn reconnect_ws(&mut self) -> Result<()> {
        // Try the next endpoint first (the current one just failed), wrapping around.
        let n = self.ws_urls.len();
        let mut last_err = None;
        for k in 1..=n {
            let i = (self.ws_idx + k) % n;
            match connect_ws(&self.ws_urls[i]).await {
                Ok(ws) => {
                    self.ws = ws;
                    self.ws_idx = i;
                    return Ok(());
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap())
    }

    pub async fn multicall_at(
        &self,
        calls: &[(Address, Bytes)],
        block: alloy::eips::BlockId,
    ) -> Result<Vec<Option<Bytes>>> {
        multicall_at(&self.http, calls, 200, block).await
    }

    /// Batch many read calls into Multicall3 `aggregate3` requests. `None` = that call failed.
    pub async fn multicall(&self, calls: &[(Address, Bytes)]) -> Result<Vec<Option<Bytes>>> {
        multicall(&self.http, calls, 200).await
    }
}

/// EIP-1898 keeps every round and chunk on the same canonical state.
pub async fn snapshot_block(provider: &DynProvider) -> Result<(alloy::eips::BlockId, u64)> {
    let block = provider
        .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
        .await?
        .context("latest block unavailable")?;
    Ok((
        alloy::eips::BlockId::hash_canonical(block.header.hash),
        block.header.number,
    ))
}

/// Chunks in flight at once. With a rotating provider each lands on a different endpoint.
const MULTICALL_CONCURRENCY: usize = 4;

/// Multicall3 batching on any provider, `chunk` calls per request (lower it for gas-heavy calls).
/// Chunks run concurrently (bounded); results keep the input order.
pub async fn multicall(
    provider: &DynProvider,
    calls: &[(Address, Bytes)],
    chunk: usize,
) -> Result<Vec<Option<Bytes>>> {
    multicall_at(provider, calls, chunk, alloy::eips::BlockId::latest()).await
}

pub async fn multicall_at(
    provider: &DynProvider,
    calls: &[(Address, Bytes)],
    chunk: usize,
    block: alloy::eips::BlockId,
) -> Result<Vec<Option<Bytes>>> {
    use futures::{StreamExt, TryStreamExt};
    let mc = IMulticall3::new(MULTICALL3, provider);
    let parts: Vec<Vec<IMulticall3::Call3>> = calls
        .chunks(chunk.max(1))
        .map(|part| {
            part.iter()
                .map(|(target, data)| IMulticall3::Call3 {
                    target: *target,
                    allowFailure: true,
                    callData: data.clone(),
                })
                .collect()
        })
        .collect();
    let results: Vec<Vec<IMulticall3::Result>> = futures::stream::iter(parts)
        .map(|batch| {
            let mc = &mc;
            async move {
                mc.aggregate3(batch)
                    .block(block)
                    .call()
                    .await
                    .context("multicall failed")
            }
        })
        .buffered(MULTICALL_CONCURRENCY)
        .try_collect()
        .await?;
    Ok(results
        .into_iter()
        .flatten()
        .map(|r| r.success.then_some(r.returnData))
        .collect())
}

/// True for HTTP 429 / provider rate-limit errors (back off instead of shrinking requests).
pub fn is_rate_limit(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}");
    s.contains("429") || s.contains("Too Many Requests") || s.contains("rate limit")
}

/// HTTP provider whose requests are counted on `meter`.
pub fn metered_http_provider(url: &str, meter: Arc<RpcMeter>) -> Result<DynProvider> {
    let client = ClientBuilder::default()
        .layer(MeterLayer(meter))
        .http(url.parse().context("invalid HTTP URL")?);
    Ok(ProviderBuilder::new().connect_client(client).erased())
}

/// Metered provider rotating across several HTTP endpoints (falls through on provider errors).
pub fn metered_rotating_provider(urls: &[String], meter: Arc<RpcMeter>) -> Result<DynProvider> {
    let client = ClientBuilder::default()
        .layer(MeterLayer(meter))
        .transport(RotatingTransport::http(urls)?, false);
    Ok(ProviderBuilder::new().connect_client(client).erased())
}

/// HTTP-only provider (e.g. a public RPC used for heavy scans).
pub fn http_provider(url: &str) -> Result<DynProvider> {
    Ok(ProviderBuilder::new()
        .connect_http(url.parse().context("invalid HTTP URL")?)
        .erased())
}

async fn connect_ws(url: &str) -> Result<DynProvider> {
    Ok(ProviderBuilder::new()
        .connect_ws(WsConnect::new(url))
        .await
        .context("WS connect failed")?
        .erased())
}

/// Exponential backoff (1s doubling, capped at 60s) for retrying network operations.
#[derive(Debug, Clone)]
pub struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            next: Duration::from_secs(1),
        }
    }
}

impl Backoff {
    pub const MAX: Duration = Duration::from_secs(60);

    /// Delay to wait now; doubles the next one.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(Self::MAX);
        d
    }

    pub fn reset(&mut self) {
        self.next = Duration::from_secs(1);
    }
}

/// Retry `op` until it succeeds, sleeping with exponential backoff between attempts.
pub async fn retry_forever<T, F, Fut>(what: &str, mut op: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut backoff = Backoff::default();
    loop {
        match op().await {
            Ok(v) => return v,
            Err(e) => {
                let d = backoff.next_delay();
                warn!(what, error = %format!("{e:#}"), retry_in_s = d.as_secs(), "failed; retrying");
                tokio::time::sleep(d).await;
            }
        }
    }
}

/// Encode a typed call for `Rpc::multicall`.
pub fn call<C: SolCall>(target: Address, c: C) -> (Address, Bytes) {
    (target, c.abi_encode().into())
}

/// Decode a multicall result for a typed call.
pub fn decode<C: SolCall>(data: &Option<Bytes>) -> Option<C::Return> {
    data.as_ref().and_then(|d| C::abi_decode_returns(d).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut b = Backoff::default();
        let delays: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 32, 60, 60]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 1);
    }
}
