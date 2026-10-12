//! Protocol-exact re-quote of a route from the integer pool state (invariant 1). The search runs on
//! the `f64` model; before a route can be simulated or sent it is priced again here with the
//! contracts' own integer math. No I/O, no floating point.

use alloy::primitives::U256;
use mev_state::PoolStore;
use mev_types::Opportunity;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Requote {
    /// Output of the whole route in loan-token units (profit = output - input), exact on every hop.
    Output(U256),
    /// A guaranteed **lower bound** on the output: at least one hop (Algebra) was priced at its maximum fee.
    LowerBound(U256),
    /// A hop's exact state is unknown or unmodelled (V4 hooks, stable pairs, unloaded window).
    Unavailable,
}

pub(crate) fn requote(store: &PoolStore, opp: &Opportunity) -> Requote {
    let mut amount = opp.amount_in;
    let mut exact = true;
    for hop in &opp.hops {
        let Some(pool) = store.get(&hop.pool) else {
            return Requote::Unavailable;
        };
        match pool.exact_amount_out_bound(hop.token_in, amount) {
            Some((out, is_exact)) => {
                amount = out;
                exact &= is_exact;
            }
            None => return Requote::Unavailable,
        }
    }
    if exact {
        Requote::Output(amount)
    } else {
        Requote::LowerBound(amount)
    }
}

/// Exact profit in loan-token units, `None` if unavailable or only bounded, `Some(0)` when the route loses or breaks even.
pub(crate) fn exact_profit(store: &PoolStore, opp: &Opportunity) -> Option<U256> {
    match requote(store, opp) {
        Requote::Output(out) => Some(out.saturating_sub(opp.amount_in)),
        Requote::LowerBound(_) | Requote::Unavailable => None,
    }
}

/// Profit that is certain whatever fee an adaptive-fee hop ends up charging (`None` unless the route was priced
/// with at least one bounded hop and the bound still clears the input).
pub(crate) fn bound_profit(store: &PoolStore, opp: &Opportunity) -> Option<U256> {
    match requote(store, opp) {
        Requote::LowerBound(out) if out > opp.amount_in => Some(out - opp.amount_in),
        _ => None,
    }
}

/// Integer search for the input that maximises the exact profit, within `[1, amount]` of the
/// float search's choice. Profit is concave in the input for constant-product hops and piecewise so
/// across V3 ticks; a ternary search can stall on a flat step, so the original amount is kept unless the
/// refined one is strictly better. Returns `(amount, profit)`; `None` if the route has no exact quote.
pub(crate) fn refine_amount(store: &PoolStore, opp: &Opportunity) -> Option<(U256, U256)> {
    let profit_at = |amount: U256| -> U256 {
        let mut o = opp.clone();
        o.amount_in = amount;
        exact_profit(store, &o).unwrap_or(U256::ZERO)
    };
    let base = exact_profit(store, opp)?;
    if opp.amount_in.is_zero() {
        return None;
    }
    // The original candidate carries the loan cap. Refinement may reduce it, never expand it.
    let (mut lo, mut hi) = (U256::from(1u8), opp.amount_in);
    let three = U256::from(3u8);
    while hi - lo > three {
        let third = (hi - lo) / three;
        let (m1, m2) = (lo + third, hi - third);
        if profit_at(m1) < profit_at(m2) {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    let (mut best, mut best_p) = (opp.amount_in, base);
    let mut a = lo;
    while a <= hi {
        let p = profit_at(a);
        if p > best_p {
            (best, best_p) = (a, p);
        }
        if a == hi {
            break;
        }
        a = a.checked_add(U256::from(1u8))?;
    }
    Some((best, best_p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_dex::{ExactState, Pool, PoolState};
    use mev_types::{Address, Hop, PoolInfo, PoolKind};

    const A: Address = Address::with_last_byte(0xa);
    const B: Address = Address::with_last_byte(0xb);

    fn v2(addr: u8, r0: u128, r1: u128) -> Pool {
        Pool::new(
            PoolInfo {
                address: Address::with_last_byte(addr),
                dex: "v2".into(),
                kind: PoolKind::UniV2 { fee_bps: 30 },
                token0: A,
                token1: B,
            },
            PoolState::V2 {
                reserve0: r0 as f64,
                reserve1: r1 as f64,
            },
        )
        .with_exact(ExactState::V2 {
            reserve0: U256::from(r0),
            reserve1: U256::from(r1),
        })
    }

    fn opp(amount: u64) -> Opportunity {
        let hop = |pool, token_in, token_out, zero_for_one| Hop {
            kind: 0,
            pool: Address::with_last_byte(pool),
            token_in,
            token_out,
            zero_for_one,
            fee_bps: 30,
            spacing: 0,
        };
        Opportunity {
            loan_token: A,
            amount_in: U256::from(amount),
            hops: vec![hop(1, A, B, true), hop(2, B, A, false)],
            est_profit: 0.0,
            est_profit_usd: 0.0,
            buy_pool: Address::with_last_byte(1),
            sell_pool: Address::with_last_byte(2),
            exact: true,
            versions: vec![],
        }
    }

    #[test]
    fn a_mispriced_pair_has_an_exact_profit_and_a_matched_pair_does_not() {
        let mut s = PoolStore::default();
        s.insert(v2(1, 1_000_000, 1_000_000));
        s.insert(v2(2, 1_000_000, 2_000_000));
        // A -> B in pool 1 yields ~1 B per A; B -> A in pool 2 yields ~0.5 A per B: a loss.
        assert_eq!(exact_profit(&s, &opp(1_000)), Some(U256::ZERO));
        let mut s = PoolStore::default();
        s.insert(v2(1, 1_000_000, 2_000_000)); // A -> B: ~2 B per A
        s.insert(v2(2, 1_000_000, 1_000_000)); // B -> A: ~1 A per B
        let p = exact_profit(&s, &opp(1_000)).unwrap();
        assert!(p > U256::from(900u64), "{p}");
    }

    #[test]
    fn refinement_never_does_worse_and_finds_the_exact_peak() {
        let mut s = PoolStore::default();
        s.insert(v2(1, 1_000_000, 2_000_000));
        s.insert(v2(2, 1_000_000, 1_000_000));
        // A deliberately poor float-search answer, far above the optimum.
        let o = opp(1_000_000);
        let before = exact_profit(&s, &o).unwrap();
        let (amount, profit) = refine_amount(&s, &o).unwrap();
        assert!(profit >= before);
        assert!(amount <= o.amount_in);
        // No neighbour beats the refined choice.
        for d in [1u64, 2, 10] {
            for a in [amount - U256::from(d), amount + U256::from(d)] {
                let mut x = o.clone();
                x.amount_in = a;
                assert!(
                    exact_profit(&s, &x).unwrap() <= profit,
                    "{a} beats {amount}"
                );
            }
        }
    }

    #[test]
    fn refinement_cannot_expand_the_candidates_loan_cap() {
        let mut s = PoolStore::default();
        s.insert(v2(1, 1_000_000, 2_000_000));
        s.insert(v2(2, 1_000_000, 1_000_000));
        let o = opp(100_000);
        let (amount, _) = refine_amount(&s, &o).unwrap();
        assert!(amount <= o.amount_in);
    }

    #[test]
    fn a_bounded_hop_never_rejects_and_only_certain_profit_is_reported() {
        use mev_dex::ExactState;
        let mut s = PoolStore::default();
        s.insert(v2(1, 1_000_000, 2_000_000));
        // Second hop is an Algebra pool: quoted at its maximum fee it is a lower bound, not an exact output.
        let mut alg = Pool::new(
            PoolInfo {
                address: Address::with_last_byte(2),
                dex: "alg".into(),
                kind: PoolKind::AlgebraV3,
                token0: A,
                token1: B,
            },
            PoolState::V3 {
                sqrt_price: 1.0,
                liquidity: 1e12,
            },
        )
        .with_exact(ExactState::Algebra {
            sqrt_price: U256::from(1u8) << 96,
            tick: 0,
            liquidity: 1_000_000_000_000,
            max_fee_zero_for_one: 5_000,
            max_fee_one_for_zero: 5_000,
        });
        alg.ticks = Some(std::sync::Arc::new(mev_dex::TickMap {
            ticks: vec![],
            exact: vec![],
            lo: -2000,
            hi: 2000,
            hint: None,
        }));
        s.insert(alg);
        assert_eq!(exact_profit(&s, &opp(1_000)), None, "not exact");
        let p =
            bound_profit(&s, &opp(1_000)).expect("A->B doubles, B->A at <=0.5% fee returns ~1990");
        assert!(p > U256::from(900u64) && p < U256::from(1_000u64), "{p}");
    }

    #[test]
    fn an_unknown_exact_state_is_reported_not_guessed() {
        let mut s = PoolStore::default();
        let mut p = v2(1, 1_000_000, 1_000_000);
        p.exact = None;
        s.insert(p);
        s.insert(v2(2, 1_000_000, 1_000_000));
        assert_eq!(requote(&s, &opp(1_000)), Requote::Unavailable);
    }
}
