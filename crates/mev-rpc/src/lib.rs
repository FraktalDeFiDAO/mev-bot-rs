//! RPC connections: HTTP reads, WS subscriptions, and batched reads via Multicall3.

use alloy::{
    primitives::{Address, Bytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    sol_types::SolCall,
};
use anyhow::{Context, Result};
use mev_bindings::{IMulticall3, MULTICALL3};

#[derive(Clone)]
pub struct Rpc {
    pub http: DynProvider,
    pub ws: DynProvider,
}

impl Rpc {
    pub async fn connect(http_url: &str, ws_url: &str) -> Result<Self> {
        let http = ProviderBuilder::new()
            .connect_http(http_url.parse().context("invalid HTTP URL")?)
            .erased();
        let ws = ProviderBuilder::new()
            .connect_ws(WsConnect::new(ws_url))
            .await
            .context("WS connect failed")?
            .erased();
        Ok(Self { http, ws })
    }

    /// Batch many read calls into Multicall3 `aggregate3` requests. `None` = that call failed.
    pub async fn multicall(&self, calls: &[(Address, Bytes)]) -> Result<Vec<Option<Bytes>>> {
        const CHUNK: usize = 200;
        let mc = IMulticall3::new(MULTICALL3, &self.http);
        let mut out = Vec::with_capacity(calls.len());
        for chunk in calls.chunks(CHUNK) {
            let batch: Vec<IMulticall3::Call3> = chunk
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
}

/// Encode a typed call for `Rpc::multicall`.
pub fn call<C: SolCall>(target: Address, c: C) -> (Address, Bytes) {
    (target, c.abi_encode().into())
}

/// Decode a multicall result for a typed call.
pub fn decode<C: SolCall>(data: &Option<Bytes>) -> Option<C::Return> {
    data.as_ref().and_then(|d| C::abi_decode_returns(d).ok())
}
