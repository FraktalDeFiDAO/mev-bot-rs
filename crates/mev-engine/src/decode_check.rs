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
use mev_presim::decode::{PoolRef, ROUTER_A, ROUTER_B, SwapIntent, decode};
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
    /// The simulation produced no `Swap` event for that pool.
    NoActual,
    /// Predicted vs simulated price, in bps (positive: we predicted a higher price), and which router (A/B/C).
    Compared { price_diff_bps: f64, router: char },
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
    /// Per router letter: exact, close, differ.
    by_router: std::collections::BTreeMap<char, [u64; 3]>,
}

impl DecodeStats {
    pub fn record(&mut self, v: Verdict) {
        match v {
            Verdict::NotDecoded => self.not_decoded += 1,
            Verdict::UnknownPool => self.unknown_pool += 1,
            Verdict::NoPrediction => self.no_prediction += 1,
            Verdict::NoActual => self.no_actual += 1,
            Verdict::Compared {
                price_diff_bps,
                router,
            } => {
                let d = price_diff_bps.abs();
                self.worst_bps = self.worst_bps.max(d);
                let slot = if d < 0.01 {
                    self.exact += 1;
                    0
                } else if d < 0.5 {
                    self.close += 1;
                    1
                } else {
                    self.differ += 1;
                    2
                };
                self.by_router.entry(router).or_default()[slot] += 1;
            }
        }
    }

    pub fn take_summary(&mut self) -> String {
        let s = std::mem::take(self);
        let routers: String = s
            .by_router
            .iter()
            .map(|(r, [e, c, d])| format!(" {r}={e}/{c}/{d}"))
            .collect();
        format!(
            "exact={} close={} differ={} worst_bps={:.2} no_prediction={} no_actual={} unknown_pool={} not_decoded={} by_router(exact/close/differ){}",
            s.exact,
            s.close,
            s.differ,
            s.worst_bps,
            s.no_prediction,
            s.no_actual,
            s.unknown_pool,
            s.not_decoded,
            routers
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

struct SimSwap {
    amount0: f64,
    amount1: f64,
    sqrt_price: f64,
    liquidity: f64,
}

fn signed(w: &[u8]) -> f64 {
    let v = U256::from_be_slice(w);
    if w[0] & 0x80 != 0 {
        -f64::from(U256::ZERO.wrapping_sub(v))
    } else {
        f64::from(v)
    }
}

fn simulated_swap(logs: &[Log], pool: Address) -> Option<SimSwap> {
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
    Some(SimSwap {
        amount0: signed(&d[0..32]),
        amount1: signed(&d[32..64]),
        sqrt_price: mev_dex::sqrt_price_from_x96(f64::from(U256::from_be_slice(&d[64..96]))),
        liquidity: f64::from(U256::from_be_slice(&d[96..128])),
    })
}

/// The pool price the simulation started from, recovered from its own swap: with liquidity `L` unchanged over the
/// swap, `1/s_before = 1/s_after - dx/L` (token0 in) or `s_before = s_after - dy/L` (token1 in), where dx/dy is the
/// input net of the fee. Inexact only when the swap crossed a tick.
fn implied_start(sw: &SimSwap, token0_in: bool, fee: f64) -> Option<f64> {
    if sw.liquidity <= 0.0 {
        return None;
    }
    let start = if token0_in {
        1.0 / (1.0 / sw.sqrt_price - sw.amount0.max(0.0) * (1.0 - fee) / sw.liquidity)
    } else {
        sw.sqrt_price - sw.amount1.max(0.0) * (1.0 - fee) / sw.liquidity
    };
    (start.is_finite() && start > 0.0).then_some(start)
}

/// Test helper: only the verdict.
#[cfg(test)]
fn check(store: &PoolStore, call: &serde_json::Value, logs: &[Log]) -> Verdict {
    check_detailed(store, call, logs).0
}

/// Compare the decoder's prediction for `call` (an `eth_simulateV1` call object) with the simulated `logs`, and
/// return the numbers behind a comparison (for the mismatch event).
///
/// Calls with a price limit (routers A and B) are measured to end at the limit when the bot sized its input to
/// reach it (129 of 137 mismatches on 2026-10-08, input fully used): the end price is the limit, independent of our
/// possibly stale start price; liquidity at the limit still comes from walking the ticks. Router C has no limit, so
/// it is judged on the size of the move (our state and the node's can differ, which is not the decoder's error).
pub fn check_detailed(
    store: &PoolStore,
    call: &serde_json::Value,
    logs: &[Log],
) -> (Verdict, Option<serde_json::Value>) {
    let hex = |k: &str| {
        call.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| alloy::hex::decode(s.trim_start_matches("0x")).ok())
    };
    let (Some(to), Some(input)) = (hex("to").filter(|t| t.len() == 20), hex("input")) else {
        return (Verdict::NotDecoded, None);
    };
    let to = Address::from_slice(&to);
    let Some(intent) = decode(to, &input) else {
        return (Verdict::NotDecoded, None);
    };
    let Some(addr) = pool_for(store, &intent) else {
        return (Verdict::UnknownPool, None);
    };
    let pool = store.get(&addr).expect("found above");
    let limit = intent
        .sqrt_limit_x96
        .map(|l| mev_dex::sqrt_price_from_x96(f64::from(l)));
    let Some(by_amount) = pool.state_after(intent.token_in, f64::from(intent.amount_in), limit)
    else {
        return (Verdict::NoPrediction, None);
    };
    let mev_dex::PoolState::V3 {
        sqrt_price: amount_sqrt,
        liquidity: amount_liq,
    } = by_amount
    else {
        return (Verdict::NoPrediction, None);
    };
    let Some(sim) = simulated_swap(logs, addr) else {
        return (Verdict::NoActual, None);
    };
    let ours_before = match pool.state {
        mev_dex::PoolState::V3 { sqrt_price, .. } => sqrt_price,
        _ => return (Verdict::NoPrediction, None),
    };
    let token0_in = intent.token_in == pool.info.token0;
    let fee = if token0_in { pool.fee0 } else { pool.fee1 };
    let Some(node_before) = implied_start(&sim, token0_in, fee) else {
        return (Verdict::NoPrediction, None);
    };
    let state_bps = ((ours_before / node_before).powi(2) - 1.0) * 1e4;
    // With a limit: walk to the limit with unlimited input. Without one: the amount-based move.
    let pinned = limit.and_then(|lm| pool.state_after(intent.token_in, 1e60, Some(lm)));
    let (bps, pred_sqrt, pred_liq) = match pinned {
        Some(mev_dex::PoolState::V3 {
            sqrt_price,
            liquidity,
        }) => (
            ((sqrt_price / sim.sqrt_price).powi(2) - 1.0) * 1e4,
            sqrt_price,
            liquidity,
        ),
        _ => {
            let ours_move = (amount_sqrt / ours_before).powi(2);
            let node_move = (sim.sqrt_price / node_before).powi(2);
            ((ours_move / node_move - 1.0) * 1e4, amount_sqrt, amount_liq)
        }
    };
    let detail = serde_json::json!({
        "router": format!("{to:#x}"),
        "selector": alloy::hex::encode(&input[..4]),
        "pool": format!("{addr:#x}"),
        "dex": pool.info.dex,
        "token_in_is_token0": token0_in,
        "amount_in": f64::from(intent.amount_in),
        "has_limit": limit.is_some(),
        "pinned_to_limit": pinned.is_some(),
        "before_sqrt": ours_before,
        "predicted_sqrt": pred_sqrt,
        "predicted_liq": pred_liq,
        "actual_sqrt": sim.sqrt_price,
        "actual_liq": sim.liquidity,
        "limit_sqrt": limit,
        "fee0": pool.fee0,
        "fee1": pool.fee1,
        "ticks_lo": pool.ticks.as_ref().map(|t| t.lo),
        "ticks_hi": pool.ticks.as_ref().map(|t| t.hi),
        "diff_bps": bps,
        "state_diff_bps": state_bps,
        "amount0": sim.amount0,
        "amount1": sim.amount1,
    });
    let router = if to == ROUTER_A {
        'A'
    } else if to == ROUTER_B {
        'B'
    } else {
        'C'
    };
    (
        Verdict::Compared {
            price_diff_bps: bps,
            router,
        },
        Some(detail),
    )
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
                router: 'A',
            },
            Verdict::Compared {
                price_diff_bps: -0.2,
                router: 'B',
            },
            Verdict::Compared {
                price_diff_bps: 3.0,
                router: 'A',
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
        assert!(t.contains("A=1/0/1") && t.contains("B=0/1/0"), "{t}");
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

    #[test]
    fn the_simulations_start_price_is_recovered_from_its_own_swap() {
        let fee = 0.0005;
        let up = SimSwap {
            amount0: -990.0,
            amount1: 1_000.0,
            sqrt_price: 1.0 + 1_000.0 * (1.0 - fee) / 1e6,
            liquidity: 1e6,
        };
        assert!((implied_start(&up, false, fee).unwrap() - 1.0).abs() < 1e-12);
        let down = SimSwap {
            amount0: 1_000.0,
            amount1: -990.0,
            sqrt_price: 1.0 / (1.0 + 1_000.0 * (1.0 - fee) / 1e6),
            liquidity: 1e6,
        };
        assert!((implied_start(&down, true, fee).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(
            implied_start(
                &SimSwap {
                    liquidity: 0.0,
                    ..up
                },
                false,
                fee
            ),
            None
        );
    }

    #[test]
    fn a_signed_word_is_read_as_twos_complement() {
        let mut w = [0xffu8; 32];
        w[31] = 0xfe; // -2
        assert_eq!(signed(&w), -2.0);
        let mut p = [0u8; 32];
        p[31] = 7;
        assert_eq!(signed(&p), 7.0);
    }
}
