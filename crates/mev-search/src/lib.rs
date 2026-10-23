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
                // Search the whole loan range: profit is concave in the input, so the
                // ternary search finds the true optimum (no guessed bound on the size).
                let cap = max_loan;
                let path = [(*buy, loan), (*sell, other)];
                let min_raw = if usd > 0.0 {
                    params.min_est_profit_usd / usd
                } else {
                    0.0
                };
                let (amount, profit, exact) = size_route(&path, cap, min_raw);
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
                    exact,
                    versions: vec![],
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

/// Does `amount` through `path` ((pool, token in) per hop) stay where the model is exact (every V3
/// hop inside its tick-spacing bucket)?
pub(crate) fn exact_path(path: &[(&Pool, Address)], amount: f64) -> bool {
    let mut x = amount;
    for (p, tin) in path {
        if x > p.max_exact_in(*tin) {
            return false;
        }
        x = p.amount_out(*tin, x);
    }
    true
}

/// Largest input in `[0, cap]` for which `path` stays exact (monotone: amounts only grow along
/// the route, so the exact region is `[0, a_max]`). Binary search.
pub(crate) fn exact_cap(path: &[(&Pool, Address)], cap: f64) -> f64 {
    if exact_path(path, cap) {
        return cap;
    }
    let (mut lo, mut hi) = (0.0f64, cap);
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if exact_path(path, mid) {
            lo = mid
        } else {
            hi = mid
        }
    }
    lo
}

/// Size a route: the profit-maximising input up to `cap`, constrained to the region where the
/// model is exact when that still clears `min_profit` (raw loan units). Returns
/// (amount, profit, exact). Exact routes send at once (with a bid); inexact ones are verified
/// first, and on crossed ticks they usually fail that check (DECISIONS #48, #60).
pub(crate) fn size_route(path: &[(&Pool, Address)], cap: f64, min_profit: f64) -> (f64, f64, bool) {
    let f = |a: f64| {
        let mut x = a;
        for (p, tin) in path {
            x = p.amount_out(*tin, x);
        }
        x - a
    };
    let (amount, profit) = best_size(cap, f);
    if exact_path(path, amount) {
        return (amount, profit, true);
    }
    let a_max = exact_cap(path, cap);
    if a_max >= 1.0 {
        let (ea, ep) = best_size(a_max, f);
        if ep >= min_profit && ea >= 1.0 {
            return (ea, ep, true);
        }
    }
    (amount, profit, false)
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
            // The executor rebuilds a V4 pool key from the LP fee and the spacing.
            PoolKind::UniV4 { fee, .. } => u16::try_from(fee).unwrap_or(0),
            _ => 0,
        },
        spacing: match pool.info.kind {
            PoolKind::UniV4 { tick_spacing, .. } => tick_spacing,
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

    #[test]
    fn size_route_prefers_the_exact_region_when_it_still_clears_the_floor() {
        use mev_dex::{Pool, PoolState};
        use mev_types::{PoolInfo, PoolKind};
        let (a, b) = (Address::with_last_byte(1), Address::with_last_byte(2));
        let v3 = |addr: u8, sqrt: f64, liq: f64| {
            Pool::new(
                PoolInfo {
                    address: Address::with_last_byte(addr),
                    dex: "t".into(),
                    kind: PoolKind::UniV3 { fee: 100 }, // spacing 1: tiny exact buckets
                    token0: a,
                    token1: b,
                },
                PoolState::V3 {
                    sqrt_price: sqrt,
                    liquidity: liq,
                },
            )
        };
        // a cheap in the first pool, dear in the second: sell a->b in p1? Use price gap 1 %.
        let p1 = v3(10, 1.0f64.sqrt(), 1e15);
        let p2 = v3(11, 1.01f64.sqrt(), 1e15);
        // Route: borrow b, buy a on p1 (b->a), sell a on p2 (a->b).
        let path = [(&p1, b), (&p2, a)];
        let (amt_full, _, exact_full) = size_route(&path, 1e15, f64::INFINITY);
        assert!(!exact_full, "the unconstrained optimum crosses ticks");
        let (amt, profit, exact) = size_route(&path, 1e15, 0.0);
        assert!(exact && profit > 0.0 && amt < amt_full);
        assert!(exact_path(&path, amt));
    }
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
            // Ten times the reserve: the search range no longer stops at half the pool.
            let cap = r * 10.0;
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
