//! Route matrix: two-pool and triangular arbitrage over every combination of pool models and token
//! decimals, priced from USD pools in the store exactly as the engine prices them.
//!
//! For every route the search returns, checked against the integer models and the reference
//! transcription of each contract (`mev-dex/tests/support`):
//!
//! - **wiring**: hops chain loan → … → loan, each hop's pool, direction, kind, fee and spacing match;
//! - **cap**: the input never exceeds the loan cap (raw units of the loan token, from its decimals);
//! - **estimate**: the `f64` profit equals the exact chained integer profit, up to the contracts' own
//!   per-step rounding (counted by the reference walker, valued at each hop's token price);
//! - **USD**: the reported USD profit equals the exact profit in whole loan tokens times the loan price,
//!   so a decimal mistake in `usd_per_raw` cannot pass;
//! - **sizing** (two-pool): no input on a grid over the whole loan range makes more exact profit;
//! - **no phantom profit**: identical pools never yield a route, at any decimal pair.

#[path = "../../mev-dex/tests/support/mod.rs"]
mod support;

use std::collections::HashMap;

use alloy::primitives::{U256, U512};
use mev_dex::Pool;
use mev_search::{SearchParams, cycles::CycleIndex, two_pool_arbs};
use mev_state::PoolStore;
use mev_types::{Address, Opportunity, PoolKind};
use support::{MODELS, Model, Reference, build_pool, pow10, quote, to_f64, w};

const USD: Address = Address::with_last_byte(0x01);
const L: Address = Address::with_last_byte(0x30);
const X: Address = Address::with_last_byte(0x40);
const Y: Address = Address::with_last_byte(0x50);
const USD_DECIMALS: u32 = 6;

const DECIMALS: [u32; 8] = [0, 2, 6, 8, 9, 12, 18, 24];
const TRI_DECIMALS: [u32; 5] = [0, 6, 8, 18, 24];
/// Whole-token USD prices `m × 10^e`: a stablecoin-like loan and an ETH-like one; a cheap and a dear
/// intermediate token.
const LOAN_PRICES: [(u128, i32); 2] = [(1, 0), (2_500, 0)];
const X_PRICES: [(u128, i32); 2] = [(37, -2), (61_000, 0)];
const Y_PRICE: (u128, i32) = (17, 0);
/// Each side of every pool holds 10^DEPTH USD; the loan cap is 10^CAP USD.
const DEPTH: u32 = 7;
const CAP_USD: f64 = 50_000.0;
/// Price gap between the two venues of a two-pool route, and around a triangle (parts per million).
const GAP_PPM: u64 = 10_000;

#[derive(Clone, Copy, Debug)]
struct Tok {
    addr: Address,
    dec: u32,
    usd: (u128, i32),
}

/// Raw reserves holding 10^DEPTH USD of each token; `b` scaled by `(1e6 + skew) / 1e6` (positive: `b`
/// is cheaper in this pool). `None` if a side would be dust (< 1e6 raw) or exceed what a pool holds.
fn reserves(a: Tok, b: Tok, skew_ppm: i64) -> Option<(U256, U256)> {
    let raw = |t: Tok| -> U512 {
        let (m, e) = t.usd;
        let mut num = w(pow10(DEPTH + t.dec));
        let mut den = U512::from(m);
        if e < 0 {
            num *= w(pow10((-e) as u32));
        } else {
            den *= w(pow10(e as u32));
        }
        num / den
    };
    let ra = raw(a);
    let rb = raw(b) * U512::from((1_000_000 + skew_ppm) as u64) / U512::from(1_000_000u64);
    let ok = |r: U512| r >= U512::from(1_000_000u64) && r <= w(pow10(32));
    (ok(ra) && ok(rb)).then(|| (U256::from(ra), U256::from(rb)))
}

/// One pool of `model` between `a` and `b` (any order), with `b` cheaper by `skew_ppm`.
fn pool(model: Model, addr: u8, a: Tok, b: Tok, skew_ppm: i64) -> Option<(Pool, Reference)> {
    let (ra, rb) = reserves(a, b, skew_ppm)?;
    let address = Address::with_last_byte(addr);
    if a.addr < b.addr {
        build_pool(model, address, a.addr, b.addr, ra, rb, true)
    } else {
        build_pool(model, address, b.addr, a.addr, rb, ra, true)
    }
}

/// A deep, plain V2 pool pricing `t` in the USD token, as the engine's `usd_per_raw_unit` reads it.
fn usd_pool(t: Tok) -> Option<Pool> {
    let usd = Tok {
        addr: USD,
        dec: USD_DECIMALS,
        usd: (1, 0),
    };
    pool(Model::V2 { fee_bps: 30 }, 0xc1, usd, t, 0).map(|(p, _)| p)
}

/// One raw unit of `t` in raw units of `loan`, at their USD prices.
fn raw_in_loan(t: Tok, loan: Tok) -> f64 {
    let px = |t: Tok| t.usd.0 as f64 * 10f64.powi(t.usd.1) / 10f64.powi(t.dec as i32);
    px(t) / px(loan)
}

struct Checked {
    routes: usize,
    unquotable: usize,
}

/// Check one route the search returned (see the module docs). `refs`: reference quote per pool.
#[allow(clippy::too_many_arguments)]
fn check_route(
    o: &Opportunity,
    store: &PoolStore,
    refs: &HashMap<Address, (Model, &Reference)>,
    toks: &HashMap<Address, Tok>,
    loan: Tok,
    max_loan: f64,
    usd_per_raw: f64,
    label: &str,
) -> Option<i128> {
    // Wiring.
    assert_eq!(o.loan_token, loan.addr, "{label}");
    assert_eq!(o.hops.first().unwrap().token_in, loan.addr, "{label}");
    assert_eq!(o.hops.last().unwrap().token_out, loan.addr, "{label}");
    for pair in o.hops.windows(2) {
        assert_eq!(
            pair[0].token_out, pair[1].token_in,
            "{label}: hops must chain"
        );
    }
    for h in &o.hops {
        let p = store
            .get(&h.pool)
            .unwrap_or_else(|| panic!("{label}: unknown pool"));
        assert!(
            p.info.has(h.token_in) && p.info.has(h.token_out) && h.token_in != h.token_out,
            "{label}"
        );
        assert_eq!(
            h.zero_for_one,
            h.token_in == p.info.token0,
            "{label}: direction"
        );
        assert_eq!(h.kind, p.info.kind.hop_kind(), "{label}: hop kind");
        match p.info.kind {
            PoolKind::UniV2 { fee_bps } => assert_eq!(h.fee_bps, fee_bps, "{label}"),
            PoolKind::UniV4 {
                fee, tick_spacing, ..
            } => {
                assert_eq!(u32::from(h.fee_bps), fee, "{label}: V4 key fee");
                assert_eq!(h.spacing, tick_spacing, "{label}: V4 key spacing");
            }
            _ => assert_eq!((h.fee_bps, h.spacing), (0, 0), "{label}"),
        }
    }
    // Cap.
    assert!(!o.amount_in.is_zero(), "{label}");
    assert!(
        to_f64(o.amount_in) <= max_loan,
        "{label}: {} over the cap {max_loan}",
        o.amount_in
    );
    // Exact chained output, with the contracts' rounding allowance along the way.
    let mut amount = o.amount_in;
    let mut slack = 1.0 + 1e-9 * to_f64(o.amount_in);
    for h in &o.hops {
        let (model, reference) = refs[&h.pool];
        let pool = store.get(&h.pool).unwrap();
        let out = quote(model, pool, h.token_in, amount)?;
        let steps = reference(h.token_in, amount).map_or(1, |(_, k)| k) as f64 + 1.0;
        slack += steps
            * (2.0 * raw_in_loan(toks[&h.token_in], loan) + raw_in_loan(toks[&h.token_out], loan));
        amount = out;
    }
    let exact_profit = i128::try_from(amount).unwrap() - i128::try_from(o.amount_in).unwrap();
    // Estimate.
    let err = (o.est_profit - exact_profit as f64).abs();
    assert!(
        err <= slack * 1.05,
        "{label}: f64 profit {} vs exact {exact_profit} (slack {slack}, amount {})",
        o.est_profit,
        o.amount_in
    );
    // USD: exact profit in whole loan tokens times the loan's USD price.
    let price = loan.usd.0 as f64 * 10f64.powi(loan.usd.1);
    let want_usd = exact_profit as f64 / 10f64.powi(loan.dec as i32) * price;
    let usd_err = (o.est_profit_usd - want_usd).abs();
    assert!(
        usd_err <= 1e-6 * want_usd.abs() + slack * usd_per_raw * 1.05 + 1e-9,
        "{label}: USD profit {} vs exact {want_usd} (usd per raw {usd_per_raw:e})",
        o.est_profit_usd
    );
    Some(exact_profit)
}

fn params<'a>(
    max_loan: &'a HashMap<Address, f64>,
    usd: &'a dyn Fn(Address) -> Option<f64>,
) -> SearchParams<'a> {
    SearchParams {
        max_loan,
        usd_per_raw: usd,
        min_est_profit_usd: 0.0,
    }
}

fn exact_route_profit(
    store: &PoolStore,
    refs: &HashMap<Address, (Model, &Reference)>,
    o: &Opportunity,
    a: U256,
) -> Option<i128> {
    let mut x = a;
    for h in &o.hops {
        x = quote(refs[&h.pool].0, store.get(&h.pool)?, h.token_in, x)?;
    }
    Some(i128::try_from(x).ok()? - i128::try_from(a).ok()?)
}

#[test]
fn two_pool_routes_over_every_model_pair_and_decimal_pair() {
    let mut c = Checked {
        routes: 0,
        unquotable: 0,
    };
    let mut missed = Vec::new();
    for ma in MODELS {
        for mb in MODELS {
            for dl in DECIMALS {
                for dx in DECIMALS {
                    for lp in LOAN_PRICES {
                        for xp in X_PRICES {
                            let loan = Tok {
                                addr: L,
                                dec: dl,
                                usd: lp,
                            };
                            let x = Tok {
                                addr: X,
                                dec: dx,
                                usd: xp,
                            };
                            let label =
                                format!("{ma:?} -> {mb:?} loan {dl}dp ${lp:?} X {dx}dp ${xp:?}");
                            // X is cheap on A (buy there) and dear on B (sell there).
                            let half = (GAP_PPM / 2) as i64;
                            let (Some((pa, ra)), Some((pb, rb)), Some(up)) = (
                                pool(ma, 0xa1, loan, x, half),
                                pool(mb, 0xb1, loan, x, -half),
                                usd_pool(loan),
                            ) else {
                                continue;
                            };
                            let mut store = PoolStore::default();
                            store.insert(pa);
                            store.insert(pb);
                            store.insert(up);
                            let usd_per_raw = store
                                .usd_per_raw_unit(L, USD, USD_DECIMALS as u8, L)
                                .expect("loan priced from its USD pool");
                            let max_loan = HashMap::from([(L, CAP_USD / usd_per_raw)]);
                            let usd = |t: Address| (t == L).then_some(usd_per_raw);
                            let p = params(&max_loan, &usd);
                            let mut opps = two_pool_arbs(&store, Address::with_last_byte(0xa1), &p);
                            opps.extend(two_pool_arbs(&store, Address::with_last_byte(0xb1), &p));
                            let refs = HashMap::from([
                                (Address::with_last_byte(0xa1), (ma, &ra)),
                                (Address::with_last_byte(0xb1), (mb, &rb)),
                            ]);
                            let toks = HashMap::from([(L, loan), (X, x)]);
                            let Some(best) = opps.first() else {
                                missed.push(label);
                                continue;
                            };
                            for o in &opps {
                                match check_route(
                                    o,
                                    &store,
                                    &refs,
                                    &toks,
                                    loan,
                                    max_loan[&L],
                                    usd_per_raw,
                                    &label,
                                ) {
                                    Some(_) => c.routes += 1,
                                    None => c.unquotable += 1,
                                }
                            }
                            // Sizing: nothing on a grid over the whole loan range beats the chosen input.
                            if let Some(chosen) =
                                exact_route_profit(&store, &refs, best, best.amount_in)
                            {
                                let cap = max_loan[&L];
                                let grid_best = (1..=40)
                                    .map(|i| {
                                        U256::from((cap * f64::from(i) / 40.0).max(1.0) as u128)
                                    })
                                    .filter_map(|a| exact_route_profit(&store, &refs, best, a))
                                    .max()
                                    .unwrap_or(i128::MIN);
                                let tol = (grid_best.unsigned_abs() as f64 * 1e-6) as i128
                                    + 64 * (1.0 + raw_in_loan(x, loan)).ceil() as i128;
                                assert!(
                                    chosen + tol >= grid_best,
                                    "{label}: chosen input {} makes {chosen}, a grid input makes {grid_best}",
                                    best.amount_in
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "two-pool: {} routes checked, {} beyond the loaded ticks, {} combinations found nothing",
        c.routes,
        c.unquotable,
        missed.len()
    );
    assert!(
        missed.is_empty(),
        "a 1% gap each way must be found: {:#?}",
        &missed[..missed.len().min(10)]
    );
    assert!(c.routes > 10_000, "{}", c.routes);
}

#[test]
fn identical_pools_never_make_a_route_at_any_decimal_pair() {
    for ma in MODELS {
        for mb in MODELS {
            for dl in DECIMALS {
                for dx in DECIMALS {
                    let loan = Tok {
                        addr: L,
                        dec: dl,
                        usd: LOAN_PRICES[1],
                    };
                    let x = Tok {
                        addr: X,
                        dec: dx,
                        usd: X_PRICES[0],
                    };
                    let (Some((pa, _)), Some((pb, _)), Some(up)) = (
                        pool(ma, 0xa1, loan, x, 0),
                        pool(mb, 0xb1, loan, x, 0),
                        usd_pool(loan),
                    ) else {
                        continue;
                    };
                    let mut store = PoolStore::default();
                    store.insert(pa);
                    store.insert(pb);
                    store.insert(up);
                    let usd_per_raw = store
                        .usd_per_raw_unit(L, USD, USD_DECIMALS as u8, L)
                        .unwrap();
                    let max_loan = HashMap::from([(L, CAP_USD / usd_per_raw)]);
                    let usd = |t: Address| (t == L).then_some(usd_per_raw);
                    let opps = two_pool_arbs(
                        &store,
                        Address::with_last_byte(0xa1),
                        &params(&max_loan, &usd),
                    );
                    assert!(
                        opps.is_empty(),
                        "{ma:?}/{mb:?} loan {dl}dp X {dx}dp: same price on both pools, yet {:?}",
                        opps.iter().map(|o| o.est_profit_usd).collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}

#[test]
fn triangles_over_every_model_triple_and_decimal_triple() {
    let mut c = Checked {
        routes: 0,
        unquotable: 0,
    };
    let mut missed = Vec::new();
    let third = (GAP_PPM / 3) as i64;
    for m1 in MODELS {
        for m2 in MODELS {
            for m3 in MODELS {
                for dl in TRI_DECIMALS {
                    for dx in TRI_DECIMALS {
                        for dy in TRI_DECIMALS {
                            let loan = Tok {
                                addr: L,
                                dec: dl,
                                usd: LOAN_PRICES[(dl as usize) % 2],
                            };
                            let x = Tok {
                                addr: X,
                                dec: dx,
                                usd: X_PRICES[(dx as usize / 6) % 2],
                            };
                            let y = Tok {
                                addr: Y,
                                dec: dy,
                                usd: Y_PRICE,
                            };
                            let label =
                                format!("{m1:?} / {m2:?} / {m3:?} loan {dl}dp X {dx}dp Y {dy}dp");
                            // L -> X -> Y -> L, each leg ~0.33% cheaper than fair.
                            let (Some((p1, r1)), Some((p2, r2)), Some((p3, r3)), Some(up)) = (
                                pool(m1, 0xa1, loan, x, third),
                                pool(m2, 0xa2, x, y, third),
                                pool(m3, 0xa3, y, loan, third),
                                usd_pool(loan),
                            ) else {
                                continue;
                            };
                            let mut store = PoolStore::default();
                            for p in [p1, p2, p3, up] {
                                store.insert(p);
                            }
                            let usd_per_raw = store
                                .usd_per_raw_unit(L, USD, USD_DECIMALS as u8, L)
                                .unwrap();
                            let max_loan = HashMap::from([(L, CAP_USD / usd_per_raw)]);
                            let usd = |t: Address| (t == L).then_some(usd_per_raw);
                            let index = CycleIndex::build(&store, &[L], 64);
                            let opps = index.arbs(
                                &store,
                                Address::with_last_byte(0xa2),
                                &params(&max_loan, &usd),
                            );
                            let refs = HashMap::from([
                                (Address::with_last_byte(0xa1), (m1, &r1)),
                                (Address::with_last_byte(0xa2), (m2, &r2)),
                                (Address::with_last_byte(0xa3), (m3, &r3)),
                            ]);
                            let toks = HashMap::from([(L, loan), (X, x), (Y, y)]);
                            let profitable: Vec<&Opportunity> = opps
                                .iter()
                                .filter(|o| o.hops.first().map(|h| h.token_out) == Some(X))
                                .collect();
                            if profitable.is_empty() {
                                missed.push(label);
                                continue;
                            }
                            for o in &opps {
                                assert_eq!(o.hops.len(), 3, "{label}");
                                match check_route(
                                    o,
                                    &store,
                                    &refs,
                                    &toks,
                                    loan,
                                    max_loan[&L],
                                    usd_per_raw,
                                    &label,
                                ) {
                                    Some(_) => c.routes += 1,
                                    None => c.unquotable += 1,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "triangles: {} routes checked, {} beyond the loaded ticks, {} combinations found nothing",
        c.routes,
        c.unquotable,
        missed.len()
    );
    assert!(
        missed.is_empty(),
        "a 1% triangle gap must be found: {:#?}",
        &missed[..missed.len().min(10)]
    );
    assert!(c.routes > 50_000, "{}", c.routes);
}
