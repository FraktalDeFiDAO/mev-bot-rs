//! Shadow check of the local swap decoder (`mev_presim::decode`): for every presimulated feed transaction it can
//! read, predict the pool's price after the swap from calldata and our own pool state, and compare it with the
//! price in the node simulation's `Swap` event. Nothing here changes state; the counters say whether the decoder is
//! good enough to replace the node round trip (DECISIONS #89).

use alloy::{
    primitives::{Address, B256, U256},
    rpc::types::Log,
    sol_types::SolEvent,
};
use mev_bindings::{IPancakeV3Pool, IUniswapV3Pool};
use mev_presim::decode::{PoolRef, SwapIntent, decode};
use mev_state::PoolStore;
use mev_types::PoolKind;

/// How the prediction compared with the node simulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Not one of the contracts the decoder reads.
    NotDecoded,
    /// Decoded, but the pool is not (uniquely) in our store.
    UnknownPool,
    /// Our pool model cannot say (no tick map, or the swap runs past the loaded ticks).
    NoPrediction,
    /// The simulation produced no `Swap` event for that pool (the transaction reverted or touched another pool).
    NoActual,
    /// Predicted vs simulated price, in bps (positive: we predicted a higher price).
    Compared { price_diff_bps: f64 },
}

#[derive(Debug, Default)]
pub struct DecodeStats {
    not_decoded: u64,
    unknown_pool: u64,
    no_prediction: u64,
    no_actual: u64,
    exact: u64,
    close: u64,
    differ: u64,
    worst_bps: f64,
}

impl DecodeStats {
    pub fn record(&mut self, v: Verdict) {
        match v {
            Verdict::NotDecoded => self.not_decoded += 1,
            Verdict::UnknownPool => self.unknown_pool += 1,
            Verdict::NoPrediction => self.no_prediction += 1,
            Verdict::NoActual => self.no_actual += 1,
            Verdict::Compared { price_diff_bps } => {
                let d = price_diff_bps.abs();
                self.worst_bps = self.worst_bps.max(d);
                if d < 0.01 {
                    self.exact += 1;
                } else if d < 0.5 {
                    self.close += 1;
                } else {
                    self.differ += 1;
                }
            }
        }
    }

    pub fn take_summary(&mut self) -> String {
        let s = std::mem::take(self);
        format!(
            "exact={} close={} differ={} worst_bps={:.2} no_prediction={} no_actual={} unknown_pool={} not_decoded={}",
            s.exact,
            s.close,
            s.differ,
            s.worst_bps,
            s.no_prediction,
            s.no_actual,
            s.unknown_pool,
            s.not_decoded
        )
    }
}

fn pool_for(store: &PoolStore, intent: &SwapIntent) -> Option<Address> {
    match &intent.pool {
        PoolRef::Address(a) => store.get(a).map(|p| p.info.address),
        PoolRef::Pair {
            token_a,
            token_b,
            fee,
        } => {
            let found: Vec<_> = store
                .pools_for(*token_a, *token_b)
                .into_iter()
                .filter(|p| {
                    matches!(p.info.kind, PoolKind::UniV3 { fee: f } | PoolKind::PancakeV3 { fee: f } if f == *fee)
                })
                .collect();
            // Several pools with the same tokens and fee (different factories): which one is meant is unknown.
            (found.len() == 1).then(|| found[0].info.address)
        }
    }
}

/// `(sqrtPriceX96, liquidity)` from the last V3-style `Swap` event the simulation emitted for `pool`.
fn simulated_end(logs: &[Log], pool: Address) -> Option<(f64, f64)> {
    let swap: [B256; 2] = [
        IUniswapV3Pool::Swap::SIGNATURE_HASH,
        IPancakeV3Pool::Swap::SIGNATURE_HASH,
    ];
    let l = logs
        .iter()
        .rev()
        .find(|l| l.address() == pool && l.topics().first().is_some_and(|t| swap.contains(t)))?;
    let d = l.data().data.as_ref();
    if d.len() < 128 {
        return None;
    }
    let price = U256::from_be_slice(&d[64..96]);
    let liq = U256::from_be_slice(&d[96..128]);
    Some((
        mev_dex::sqrt_price_from_x96(f64::from(price)),
        f64::from(liq),
    ))
}

/// Compare the decoder's prediction for `call` (an `eth_simulateV1` call object) with the simulated `logs`.
pub fn check(store: &PoolStore, call: &serde_json::Value, logs: &[Log]) -> Verdict {
    let hex = |k: &str| {
        call.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| alloy::hex::decode(s.trim_start_matches("0x")).ok())
    };
    let (Some(to), Some(input)) = (hex("to").filter(|t| t.len() == 20), hex("input")) else {
        return Verdict::NotDecoded;
    };
    let Some(intent) = decode(Address::from_slice(&to), &input) else {
        return Verdict::NotDecoded;
    };
    let Some(addr) = pool_for(store, &intent) else {
        return Verdict::UnknownPool;
    };
    let pool = store.get(&addr).expect("found above");
    let limit = intent
        .sqrt_limit_x96
        .map(|l| mev_dex::sqrt_price_from_x96(f64::from(l)));
    let Some(predicted) = pool.state_after(intent.token_in, f64::from(intent.amount_in), limit)
    else {
        return Verdict::NoPrediction;
    };
    let mev_dex::PoolState::V3 { sqrt_price, .. } = predicted else {
        return Verdict::NoPrediction;
    };
    let Some((actual, _)) = simulated_end(logs, addr) else {
        return Verdict::NoActual;
    };
    let ratio = sqrt_price / actual;
    Verdict::Compared {
        price_diff_bps: (ratio * ratio - 1.0) * 1e4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_buckets_and_resets() {
        let mut s = DecodeStats::default();
        for v in [
            Verdict::Compared {
                price_diff_bps: 0.0,
            },
            Verdict::Compared {
                price_diff_bps: -0.2,
            },
            Verdict::Compared {
                price_diff_bps: 3.0,
            },
            Verdict::NoActual,
            Verdict::NotDecoded,
        ] {
            s.record(v);
        }
        let t = s.take_summary();
        assert!(
            t.starts_with("exact=1 close=1 differ=1 worst_bps=3.00"),
            "{t}"
        );
        assert!(t.contains("no_actual=1") && t.contains("not_decoded=1"));
        assert!(s.take_summary().starts_with("exact=0 close=0 differ=0"));
    }

    #[test]
    fn a_call_that_is_not_one_of_the_three_contracts_is_not_decoded() {
        let store = PoolStore::default();
        let call = serde_json::json!({"to": "0x0000000000000000000000000000000000000001", "input": "0x12345678"});
        assert_eq!(check(&store, &call, &[]), Verdict::NotDecoded);
        assert_eq!(
            check(&store, &serde_json::Value::Null, &[]),
            Verdict::NotDecoded
        );
    }
}
