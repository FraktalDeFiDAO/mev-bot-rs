//! Read preconfirmed state: `eth_call` at `latest` becomes `eth_call` at `pending`.
//!
//! On OP Stack chains with Flashblocks (Base), `pending` is the block being built, updated every
//! ~200 ms, while `latest` is the last sealed 2 s block (checked 2026-10-16 on mainnet.base.org
//! and mainnet-preconf.base.org: `pending` = `latest` + 1 with the flashblocks so far). A health
//! re-check at `latest` lags a price move by up to a block; winners act within it. Calls pinned to
//! a block number or hash are left alone.

use std::task::{Context, Poll};

use alloy::{
    rpc::json_rpc::{Request, RequestPacket, ResponsePacket, SerializedRequest},
    transports::{TransportError, TransportFut},
};
use serde_json::value::RawValue;
use tower::{Layer, Service};

/// The params of an `eth_call` at `latest`, rewritten to `pending`; `None` = leave as is.
pub fn pending_params(method: &str, params: &str) -> Option<String> {
    if method != "eth_call" {
        return None;
    }
    let mut v: Vec<serde_json::Value> = serde_json::from_str(params).ok()?;
    match v.get(1) {
        Some(serde_json::Value::String(t)) if t == "latest" => {}
        _ => return None,
    }
    v[1] = serde_json::Value::String("pending".into());
    serde_json::to_string(&v).ok()
}

fn rewrite(req: SerializedRequest) -> SerializedRequest {
    let Some(p) = req
        .params()
        .and_then(|p| pending_params(req.method(), p.get()))
    else {
        return req;
    };
    let Ok(raw) = RawValue::from_string(p) else {
        return req;
    };
    let method = req.method().to_string();
    Request::new(method, req.id().clone(), raw)
        .serialize()
        .unwrap_or(req)
}

/// Tower layer applying [`pending_params`] to every request.
#[derive(Clone, Copy, Default)]
pub struct PendingLayer;

impl<S> Layer<S> for PendingLayer {
    type Service = PendingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PendingService { inner }
    }
}

#[derive(Clone)]
pub struct PendingService<S> {
    inner: S,
}

impl<S> Service<RequestPacket> for PendingService<S>
where
    S: Service<
            RequestPacket,
            Response = ResponsePacket,
            Error = TransportError,
            Future = TransportFut<'static>,
        > + Send
        + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        let req = match req {
            RequestPacket::Single(r) => RequestPacket::Single(rewrite(r)),
            RequestPacket::Batch(rs) => RequestPacket::Batch(rs.into_iter().map(rewrite).collect()),
        };
        self.inner.call(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_eth_call_at_latest_moves_to_pending() {
        let call = r#"[{"to":"0x01","data":"0x"},"latest"]"#;
        assert_eq!(
            pending_params("eth_call", call).as_deref(),
            Some(r#"[{"data":"0x","to":"0x01"},"pending"]"#)
        );
        assert_eq!(
            pending_params("eth_call", r#"[{"to":"0x01"},"0x10"]"#),
            None
        );
        assert_eq!(
            pending_params("eth_call", r#"[{"to":"0x01"},{"blockHash":"0xab"}]"#),
            None
        );
        assert_eq!(
            pending_params("eth_getBalance", r#"["0x01","latest"]"#),
            None
        );
    }
}
