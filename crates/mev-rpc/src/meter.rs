//! RPC usage metering: a transport layer counting every JSON-RPC request by method.
//!
//! Until the bot has its own node, every paid-RPC call costs quota. Each provider gets its own
//! [`RpcMeter`]; the engine drains them once a minute into logs and the metrics file, so data
//! efficiency is measured rather than assumed.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use alloy::{
    rpc::json_rpc::{RequestPacket, ResponsePacket},
    transports::{TransportError, TransportFut},
};
use tower::{Layer, Service};

/// Request counts by method for one provider.
#[derive(Debug, Default)]
pub struct RpcMeter {
    counts: Mutex<HashMap<String, u64>>,
}

impl RpcMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, method: &str) {
        *self
            .counts
            .lock()
            .unwrap()
            .entry(method.to_string())
            .or_default() += 1;
    }

    /// Counts since the last drain, most-called first; resets the meter.
    pub fn drain(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.counts.lock().unwrap().drain().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

/// Tower layer that records each request's method on an [`RpcMeter`].
#[derive(Clone)]
pub struct MeterLayer(pub Arc<RpcMeter>);

impl<S> Layer<S> for MeterLayer {
    type Service = MeterService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MeterService {
            inner,
            meter: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub struct MeterService<S> {
    inner: S,
    meter: Arc<RpcMeter>,
}

impl<S> Service<RequestPacket> for MeterService<S>
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
        for m in req.method_names() {
            self.meter.record(m);
        }
        self.inner.call(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_sorts_by_count_and_resets() {
        let m = RpcMeter::new();
        for _ in 0..3 {
            m.record("eth_call");
        }
        m.record("eth_gasPrice");
        assert_eq!(
            m.drain(),
            vec![("eth_call".to_string(), 3), ("eth_gasPrice".to_string(), 1)]
        );
        assert!(m.drain().is_empty());
    }
}
