//! RPC connections: HTTP reads, WS subscriptions, and batched reads via Multicall3.

use alloy::{
    primitives::{Address, Bytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    sol_types::SolCall,
};
use std::{future::Future, time::Duration};

use anyhow::{Context, Result};
use mev_bindings::{IMulticall3, MULTICALL3};
use tracing::warn;

#[derive(Clone)]
pub struct Rpc {
    pub http: DynProvider,
    pub ws: DynProvider,
    ws_url: String,
}

impl Rpc {
    pub async fn connect(http_url: &str, ws_url: &str) -> Result<Self> {
        let http = ProviderBuilder::new()
            .connect_http(http_url.parse().context("invalid HTTP URL")?)
            .erased();
        let ws = connect_ws(ws_url).await?;
        Ok(Self {
            http,
            ws,
            ws_url: ws_url.to_string(),
        })
    }

    /// Replace the WS provider with a fresh connection. Needed after alloy's own reconnect
    /// gives up (e.g. a multi-minute DNS outage): the old provider is dead for good.
    pub async fn reconnect_ws(&mut self) -> Result<()> {
        self.ws = connect_ws(&self.ws_url).await?;
        Ok(())
    }

    /// Batch many read calls into Multicall3 `aggregate3` requests. `None` = that call failed.
    pub async fn multicall(&self, calls: &[(Address, Bytes)]) -> Result<Vec<Option<Bytes>>> {
        multicall(&self.http, calls, 200).await
    }
}

/// Multicall3 batching on any provider, `chunk` calls per request (lower it for gas-heavy calls).
pub async fn multicall(
    provider: &DynProvider,
    calls: &[(Address, Bytes)],
    chunk: usize,
) -> Result<Vec<Option<Bytes>>> {
    let mc = IMulticall3::new(MULTICALL3, provider);
    let mut out = Vec::with_capacity(calls.len());
    for part in calls.chunks(chunk.max(1)) {
        let batch: Vec<IMulticall3::Call3> = part
            .iter()
            .map(|(target, data)| IMulticall3::Call3 {
                target: *target,
                allowFailure: true,
                callData: data.clone(),
            })
            .collect();
        let res = mc
            .aggregate3(batch)
            .call()
            .await
            .context("multicall failed")?;
        out.extend(res.into_iter().map(|r| r.success.then_some(r.returnData)));
    }
    Ok(out)
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
