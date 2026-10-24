//! Single-swap matrix: every exchange model × token-decimal pair × price × depth × direction × size.
//!
//! Each pool is built from the same human-unit economics (reserves in whole tokens, a price in whole
//! token1 per whole token0) at every decimal pair from 0 to 24, then every swap is checked three ways:
//!
//! 1. **Reference**: the integer output equals an independent re-implementation of the contract's math,
//!    written here in 512-bit integers straight from the Solidity (Uniswap V2 / Camelot / Solidly
//!    `getAmountOut`; `SwapMath.computeSwapStep`, `SqrtPriceMath`, and `nextInitializedTickWithinOneWord`
//!    for every V3-style pool, including the 256-bit overflow branches that extreme decimal gaps reach).
//! 2. **Screening model**: the `f64` model the search uses agrees with the integer output, so a decimal
//!    gap can neither hide an opportunity nor invent one.
//! 3. **Decimal invariance**: the same trade in whole tokens gives the same whole-token output at every
//!    decimal pair, up to one raw unit of the output token's rounding.
//!
//! Ground truth against the deployed contracts is in `mev-discovery/tests/exact_*_live.rs`.

mod support;

use alloy::primitives::{U256, U512};
use mev_dex::Pool;
use mev_types::Address;
use support::{MODELS, Model, Reference, build_pool, pow10, quote, to_f64, w};

const T0: Address = Address::with_last_byte(0x10);
const T1: Address = Address::with_last_byte(0x20);

/// Every decimal count a real token uses on the chains we trade (0: some memecoins, 2: EURS-style,
/// 6: USDC/USDT, 8: WBTC/cbBTC, 9: some bridged, 12, 18: most, 24: a few rebasing/yield tokens).
const DECIMALS: [u32; 8] = [0, 2, 6, 8, 9, 12, 18, 24];
/// Human price of token0 in token1, as `m × 10^e`: 1e-8, 0.025, 1, 3127.45, 1e6.
const PRICES: [(u128, i32); 5] = [(1, -8), (25, -3), (1, 0), (312_745, -2), (1, 6)];
/// Human depth: whole token0 in the pool.
const DEPTHS: [u32; 2] = [4, 8];
/// Trade size as a fraction of the input-side reserve, `n / 10^7`.
const FRACTIONS: [u64; 6] = [1, 1_000, 10_000, 100_000, 1_000_000, 5_000_000];
/// Raw reserves stay within what a pool can hold (Uniswap V2 reserves are uint112 ≈ 5.2e33) and above dust.
const MIN_RAW_EXP: i32 = 6;
const MAX_RAW_EXP: i32 = 32;

fn log10(m: u128) -> i32 {
    (m as f64).log10().floor() as i32
}

/// A market: raw reserves of token0 and token1 for `10^depth` whole token0 at `price`.
#[derive(Clone, Copy, Debug)]
struct Market {
    d0: u32,
    d1: u32,
    price: (u128, i32),
    depth: u32,
    r0: U256,
    r1: U256,
}

fn market(d0: u32, d1: u32, price: (u128, i32), depth: u32) -> Option<Market> {
    let (m, e) = price;
    let e0 = depth as i32 + d0 as i32;
    let e1 = depth as i32 + d1 as i32 + e;
    let top1 = e1 + log10(m);
    if e1 < 0 || e0 < MIN_RAW_EXP || top1 < MIN_RAW_EXP || e0 > MAX_RAW_EXP || top1 > MAX_RAW_EXP {
        return None; // such a pool cannot exist with sane depth: not a market
    }
    Some(Market {
        d0,
        d1,
        price,
        depth,
        r0: pow10(e0 as u32),
        r1: U256::from(m) * pow10(e1 as u32),
    })
}

fn amounts(mk: &Market, tin: Address) -> Vec<U256> {
    let (rin, din) = if tin == T0 {
        (mk.r0, mk.d0)
    } else {
        (mk.r1, mk.d1)
    };
    let mut v: Vec<U256> = FRACTIONS
        .iter()
        .map(|f| rin * U256::from(*f) / U256::from(10_000_000u64))
        .collect();
    v.push(U256::from(1u8)); // one raw unit
    v.push(pow10(din)); // one whole token
    v.retain(|a| !a.is_zero());
    v
}

const POOL: Address = Address::with_last_byte(0xee);

fn build(model: Model, mk: &Market, positions: bool) -> Option<(Pool, Reference)> {
    build_pool(model, POOL, T0, T1, mk.r0, mk.r1, positions)
}

fn markets() -> Vec<Market> {
    let mut v = Vec::new();
    for d0 in DECIMALS {
        for d1 in DECIMALS {
            for price in PRICES {
                for depth in DEPTHS {
                    if let Some(m) = market(d0, d1, price, depth) {
                        v.push(m);
                    }
                }
            }
        }
    }
    v
}

#[test]
fn the_matrix_covers_every_decimal_pair() {
    let ms = markets();
    for d0 in DECIMALS {
        for d1 in DECIMALS {
            assert!(
                ms.iter().any(|m| m.d0 == d0 && m.d1 == d1),
                "no feasible market for decimals ({d0}, {d1})"
            );
        }
    }
}

/// Check 1: the integer quote equals the reference transcription of the contract, to the unit.
#[test]
fn every_model_matches_the_reference_at_every_decimal_pair() {
    let mut checked = 0usize;
    let mut refused = 0usize;
    for model in MODELS {
        for mk in markets() {
            let Some((pool, reference)) = build(model, &mk, true) else {
                continue;
            };
            for tin in [T0, T1] {
                for a in amounts(&mk, tin) {
                    let mine = quote(model, &pool, tin, a);
                    let want = reference(tin, a).map(|(o, _)| o);
                    assert_eq!(
                        mine,
                        want,
                        "{model:?} decimals ({}, {}) price {:?} depth 1e{} in {} amount {a}",
                        mk.d0,
                        mk.d1,
                        mk.price,
                        mk.depth,
                        if tin == T0 { "token0" } else { "token1" },
                    );
                    if mine.is_some() {
                        checked += 1;
                    } else {
                        refused += 1;
                    }
                }
            }
        }
    }
    eprintln!("reference matrix: {checked} quotes equal to the unit, {refused} refused by both");
    assert!(
        checked > 10_000,
        "the matrix must actually quote: {checked}"
    );
}

/// Check 2: the `f64` screening model agrees with the integer quote wherever the integer quote exists.
#[test]
fn the_screening_model_agrees_with_the_integer_quote() {
    let mut worst = 0f64;
    for model in MODELS {
        for mk in markets() {
            let Some((pool, reference)) = build(model, &mk, true) else {
                continue;
            };
            for tin in [T0, T1] {
                for a in amounts(&mk, tin) {
                    let Some(exact_out) = quote(model, &pool, tin, a) else {
                        continue;
                    };
                    let steps = reference(tin, a).map_or(1, |(_, k)| k) as f64;
                    let e = to_f64(exact_out);
                    let f = pool.amount_out(tin, to_f64(a));
                    // The contracts round in integers: every swap step floors its input after fee and rounds
                    // its fee up, at most two raw input units per step at the marginal rate, plus a raw output
                    // unit per step; beyond that the two must agree to 1e-9.
                    let err = (f - e).abs();
                    let rel = if e > 0.0 { err / e } else { 0.0 };
                    let one_input_unit = pool.amount_out(tin, 1.0).max(e / to_f64(a));
                    if err > 4.0 * steps + 2.0 * steps * one_input_unit {
                        worst = worst.max(rel);
                        assert!(
                            rel < 1e-9,
                            "{model:?} decimals ({}, {}) price {:?} in {} amount {a}: f64 {f} vs exact {e} (rel {rel:e})",
                            mk.d0,
                            mk.d1,
                            mk.price,
                            if tin == T0 { "token0" } else { "token1" },
                        );
                    }
                }
            }
        }
    }
    eprintln!("screening model: worst relative error {worst:e}");
}

/// Check 3, decimals are only units: the same trade in whole tokens gives the same whole-token output at every
/// decimal pair. Compared against 18/18, allowing the output token's raw rounding plus the reference
/// pool's own relative rounding.
#[test]
fn whole_token_results_do_not_depend_on_decimals() {
    let mut compared = 0usize;
    for model in MODELS {
        for price in PRICES {
            for depth in DEPTHS {
                let Some(base) = market(18, 18, price, depth) else {
                    continue;
                };
                let Some((base_pool, base_ref)) = build(model, &base, false) else {
                    continue;
                };
                for d0 in DECIMALS {
                    for d1 in DECIMALS {
                        let Some(mk) = market(d0, d1, price, depth) else {
                            continue;
                        };
                        let Some((pool, reference)) = build(model, &mk, false) else {
                            continue;
                        };
                        for zfo in [true, false] {
                            let (tin, din, dout) = if zfo { (T0, d0, d1) } else { (T1, d1, d0) };
                            let rin = if zfo { mk.r0 } else { mk.r1 };
                            let base_rin = if zfo { base.r0 } else { base.r1 };
                            for f in FRACTIONS {
                                let a = rin * U256::from(f) / U256::from(10_000_000u64);
                                let base_a = base_rin * U256::from(f) / U256::from(10_000_000u64);
                                if a.is_zero() {
                                    continue;
                                }
                                let (Some(out), Some(base_out)) = (
                                    quote(model, &pool, tin, a),
                                    quote(model, &base_pool, tin, base_a),
                                ) else {
                                    continue;
                                };
                                // Whole tokens, compared in exact rationals: out / 10^dout vs base_out / 10^18.
                                let lhs = w(out) * w(pow10(18));
                                let rhs = w(base_out) * w(pow10(dout));
                                let diff = if lhs > rhs { lhs - rhs } else { rhs - lhs };
                                // Each pool rounds per swap step: up to two raw input units (input after fee
                                // floored, fee rounded up) and a raw output unit. In the common scale a raw
                                // output unit is 10^18 on the left and 10^dout on the right; a raw input unit
                                // is worth lhs / a and rhs / base_a. The input amount itself is floored at
                                // `din` decimals (one more input unit), and the sqrt price is an integer
                                // square root (relative 1e-9 covers it at every depth in the matrix).
                                let steps =
                                    U512::from(reference(tin, a).map_or(1, |(_, k)| k) as u64 + 1);
                                let base_steps = U512::from(
                                    base_ref(tin, base_a).map_or(1, |(_, k)| k) as u64 + 1,
                                );
                                let two = U512::from(2u8);
                                let out_units = steps * w(pow10(18)) + base_steps * w(pow10(dout));
                                let in_units =
                                    two * steps * lhs / w(a) + two * base_steps * rhs / w(base_a);
                                let slack =
                                    out_units + in_units + rhs / U512::from(1_000_000_000u64);
                                assert!(
                                    diff <= slack,
                                    "{model:?} ({d0},{d1}) vs (18,18) price {price:?} depth 1e{depth} zfo {zfo} frac {f}: {out} @{dout} vs {base_out} @18 (in decimals {din})",
                                );
                                compared += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("decimal invariance: {compared} trades compared against 18/18");
    assert!(compared > 5_000);
}
