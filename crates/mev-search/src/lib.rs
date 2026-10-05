//! Arbitrage search over the pool store. Pure computation — no I/O.

pub mod cycles;

use std::collections::HashMap;

use alloy::primitives::U256;
use mev_dex::Pool;
use mev_state::PoolStore;
use mev_types::{Address, Hop, Opportunity, PoolKind};

pub struct SearchParams<'a> {
    /// Loan-able tokens and the max loan size for each, raw units.
    pub max_loan: &'a HashMap<Address, f64>,
    /// USD value of one raw unit of a token.
    pub usd_per_raw: &'a dyn Fn(Address) -> Option<f64>,
    /// Skip candidates whose estimated profit is below this (before gas).
    pub min_est_profit_usd: f64,
}

/// Two-pool cycles (borrow L -> buy X on pool A -> sell X for L on pool B) involving `changed`.
pub fn two_pool_arbs(
    store: &PoolStore,
    changed: Address,
    params: &SearchParams,
) -> Vec<Opportunity> {
    let siblings = store.siblings(&changed);
    if siblings.len() < 2 {
        return vec![];
    }
    let mut out = Vec::new();
    for &loan in [siblings[0].info.token0, siblings[0].info.token1].iter() {
        let Some(&max_loan) = params.max_loan.get(&loan) else {
            continue;
        };
        let Some(usd) = (params.usd_per_raw)(loan) else {
            continue;
        };
        let other = siblings[0].info.other(loan);
        for buy in &siblings {
            for sell in &siblings {
                if buy.info.address == sell.info.address
                    || (buy.info.address != changed && sell.info.address != changed)
                {
                    continue;
                }
                // Profitable at the margin only if the round-trip rate after fees exceeds 1.
                if buy.marginal_rate(loan) * sell.marginal_rate(other) <= 1.0 {
                    continue;
                }
                let cap = max_loan.min(buy.reserve_of(loan) * 0.5);
                let (amount, profit) =
                    best_size(cap, |a| sell.amount_out(other, buy.amount_out(loan, a)) - a);
                let profit_usd = profit * usd;
                if profit_usd < params.min_est_profit_usd || amount < 1.0 {
                    continue;
                }
                out.push(Opportunity {
                    loan_token: loan,
                    amount_in: U256::from(amount as u128),
                    hops: vec![hop(buy, loan, other), hop(sell, other, loan)],
                    est_profit: profit,
                    est_profit_usd: profit_usd,
                    buy_pool: buy.info.address,
                    sell_pool: sell.info.address,
                });
            }
        }
    }
    out.sort_by(|a, b| b.est_profit_usd.total_cmp(&a.est_profit_usd));
    out
}

/// Best route to sell `amount_in` of `token_in` for `token_out`: the best single pool, or the best
/// two-hop path through any `via` token. Returns (hops, estimated output).
pub fn best_route(
    store: &PoolStore,
    token_in: Address,
    token_out: Address,
    amount_in: f64,
    via: &[Address],
) -> Option<(Vec<Hop>, f64)> {
    let best_pool = |a: Address, b: Address, amt: f64| -> Option<(&Pool, f64)> {
        store
            .pools_for(a, b)
            .into_iter()
            .map(|p| (p, p.amount_out(a, amt)))
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .filter(|(_, out)| *out > 0.0)
    };
    let mut best: Option<(Vec<Hop>, f64)> = None;
    if let Some((p, out)) = best_pool(token_in, token_out, amount_in) {
        best = Some((vec![hop(p, token_in, token_out)], out));
    }
    for &mid in via {
        if mid == token_in || mid == token_out {
            continue;
        }
        let Some((p1, mid_out)) = best_pool(token_in, mid, amount_in) else {
            continue;
        };
        let Some((p2, out)) = best_pool(mid, token_out, mid_out) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, b)| out > *b) {
            best = Some((vec![hop(p1, token_in, mid), hop(p2, mid, token_out)], out));
        }
    }
    best
}

pub(crate) fn hop(pool: &Pool, token_in: Address, token_out: Address) -> Hop {
    Hop {
        kind: pool.info.kind.hop_kind(),
        pool: pool.info.address,
        token_in,
        token_out,
        zero_for_one: token_in == pool.info.token0,
        fee_bps: match pool.info.kind {
            PoolKind::UniV2 { fee_bps } => fee_bps,
            _ => 0,
        },
    }
}

/// Maximize a concave profit function on [0, cap] by ternary search. Returns (amount, profit).
pub(crate) fn best_size(cap: f64, f: impl Fn(f64) -> f64) -> (f64, f64) {
    let (mut lo, mut hi) = (0.0, cap);
    for _ in 0..100 {
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        if f(m1) < f(m2) { lo = m1 } else { hi = m2 }
    }
    let a = (lo + hi) / 2.0;
    (a, f(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use mev_dex::PoolState;
    use mev_types::PoolInfo;

    const A: Address = address!("0000000000000000000000000000000000000001");
    const B: Address = address!("0000000000000000000000000000000000000002");

    fn v2(addr: u8, r0: f64, r1: f64) -> Pool {
        let info = PoolInfo {
            address: Address::with_last_byte(addr),
            dex: "t".into(),
            kind: PoolKind::UniV2 { fee_bps: 30 },
            token0: A,
            token1: B,
        };
        Pool::new(
            info,
            PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
        )
    }

    fn run(store: &PoolStore, changed: u8) -> Vec<Opportunity> {
        let max_loan = HashMap::from([(A, 1e9), (B, 1e9)]);
        let usd = |_: Address| Some(1.0);
        let params = SearchParams {
            max_loan: &max_loan,
            usd_per_raw: &usd,
            min_est_profit_usd: 0.0,
        };
        two_pool_arbs(store, Address::with_last_byte(changed), &params)
    }

    #[test]
    fn finds_arb_when_prices_diverge() {
        let mut s = PoolStore::default();
        s.insert(v2(10, 1_000_000.0, 2_000_000.0)); // 1 A = 2 B
        s.insert(v2(11, 1_000_000.0, 2_200_000.0)); // 1 A = 2.2 B
        let opps = run(&s, 10);
        assert!(!opps.is_empty());
        let best = &opps[0];
        assert!(best.est_profit > 0.0);
        // Buy B cheaply... the best route borrows A on the pool that gives more B (11) and sells B on 10.
        assert_eq!(best.hops.len(), 2);
        assert_eq!(best.hops[0].token_in, best.hops[1].token_out);
    }

    #[test]
    fn best_route_prefers_deeper_pool() {
        let mut s = PoolStore::default();
        s.insert(v2(10, 1_000.0, 2_000.0));
        s.insert(v2(11, 1_000_000.0, 2_000_000.0));
        let (hops, out) = best_route(&s, A, B, 100.0, &[]).unwrap();
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].pool, Address::with_last_byte(11));
        assert!(out > 199.0 && out < 200.0);
    }

    #[test]
    fn no_arb_when_prices_match() {
        let mut s = PoolStore::default();
        s.insert(v2(10, 1_000_000.0, 2_000_000.0));
        s.insert(v2(11, 1_000_000.0, 2_000_000.0));
        assert!(run(&s, 10).is_empty());
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use alloy::primitives::address;
    use mev_dex::PoolState;
    use mev_types::PoolInfo;
    use proptest::prelude::*;

    const A: Address = address!("0000000000000000000000000000000000000001");
    const B: Address = address!("0000000000000000000000000000000000000002");

    fn v2(addr: u8, r0: f64, r1: f64) -> Pool {
        let info = PoolInfo {
            address: Address::with_last_byte(addr),
            dex: "p".into(),
            kind: PoolKind::UniV2 { fee_bps: 30 },
            token0: A,
            token1: B,
        };
        Pool::new(
            info,
            PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
        )
    }

    proptest! {
        #[test]
        fn ternary_search_beats_a_brute_force_grid(r in 1e6f64..1e12, p1 in 1.0f64..3.0, gap in 1.0f64..1.3) {
            let (buy, sell) = (v2(1, r, r * p1 * gap), v2(2, r, r * p1));
            let f = |x: f64| sell.amount_out(B, buy.amount_out(A, x)) - x;
            let cap = r * 0.5;
            let (_, best) = best_size(cap, f);
            let grid_best = (0..=2_000).map(|i| f(cap * i as f64 / 2_000.0)).fold(f64::MIN, f64::max);
            prop_assert!(best >= grid_best - grid_best.abs() * 1e-6 - 1e-6);
        }

        #[test]
        fn identical_pools_never_yield_an_opportunity(r0 in 1e6f64..1e12, ratio in 0.01f64..100.0) {
            let mut s = PoolStore::default();
            s.insert(v2(10, r0, r0 * ratio));
            s.insert(v2(11, r0, r0 * ratio));
            let max_loan = HashMap::from([(A, 1e15), (B, 1e15)]);
            let usd = |_: Address| Some(1.0);
            let params = SearchParams { max_loan: &max_loan, usd_per_raw: &usd, min_est_profit_usd: 0.0 };
            prop_assert!(two_pool_arbs(&s, Address::with_last_byte(10), &params).is_empty());
        }

        #[test]
        fn opportunities_respect_the_loan_cap(r in 1e6f64..1e12, gap in 1.01f64..1.5, cap in 1.0f64..1e9) {
            let mut s = PoolStore::default();
            s.insert(v2(10, r, r * 2.0));
            s.insert(v2(11, r, r * 2.0 * gap));
            let max_loan = HashMap::from([(A, cap), (B, cap)]);
            let usd = |_: Address| Some(1.0);
            let params = SearchParams { max_loan: &max_loan, usd_per_raw: &usd, min_est_profit_usd: 0.0 };
            for o in two_pool_arbs(&s, Address::with_last_byte(10), &params) {
                prop_assert!(f64::from(o.amount_in) <= cap + 1.0);
                prop_assert!(o.est_profit >= 0.0);
            }
        }
    }
}
