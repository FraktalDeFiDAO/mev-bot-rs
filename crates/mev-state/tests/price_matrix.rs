//! Price matrix: the USD price of a raw token unit and the on-chain profit floor derived from it, for every
//! token decimal count, both USD-token decimal counts, every pool model the price can come from, and both
//! price paths (a direct USD pool, or through the native token).
//!
//! These are the numbers decimals can break silently: the profit floor (`minProfit`, raw loan units) is
//! `ceil(cost / price_per_raw)`; a price off by 10^k makes the floor 10^k too small (a trade that pays gas
//! for nothing passes) or too large (real profit is refused). Checked:
//!
//! - **exact**: `usd_price_integer` equals the price implied by the pools' own integer state, as an exact
//!   rational, so the floor equals `ceil(cost × 10^d / price)` to the unit, and is the least raw amount
//!   whose value covers the cost;
//! - **economic**: one whole token is worth its USD price (to the pool's rounding), whatever the decimals;
//! - **float path**: `usd_per_raw_unit` (search and loan caps) agrees with the integer price to 1e-9, and
//!   the loan cap in whole tokens is the USD cap divided by the price.

#[path = "../../mev-dex/tests/support/mod.rs"]
mod support;

use alloy::primitives::{U256, U512};
use mev_state::PoolStore;
use mev_types::{Address, price::Ratio};
use support::{MODELS, Model, build_pool, pow10, w};

const USD: Address = Address::with_last_byte(0x01);
const T: Address = Address::with_last_byte(0x30);
const NATIVE: Address = Address::with_last_byte(0x40);

const DECIMALS: [u32; 8] = [0, 2, 6, 8, 9, 12, 18, 24];
const USD_DECIMALS: [u32; 2] = [6, 18];
/// Whole-token USD prices `m × 10^e`.
const PRICES: [(u128, i32); 5] = [(1, -6), (37, -2), (1, 0), (2_500, 0), (61_000, 0)];
const NATIVE_PRICE: (u128, i32) = (2_500, 0);
const NATIVE_DECIMALS: u32 = 18;
/// Each side of a pool holds 10^DEPTH USD.
const DEPTH: u32 = 8;
/// Costs to cover, in micro-USD: 1 µ$, 1¢, $1, $5, $1,000.
const COSTS: [u64; 5] = [1, 10_000, 1_000_000, 5_000_000, 1_000_000_000];

/// Raw units of a token worth 10^DEPTH USD.
fn raw_for_depth(dec: u32, (m, e): (u128, i32)) -> Option<U256> {
    let mut num = w(pow10(DEPTH + dec));
    let mut den = U512::from(m);
    if e < 0 {
        num *= w(pow10((-e) as u32));
    } else {
        den *= w(pow10(e as u32));
    }
    let r = num / den;
    (r >= U512::from(1_000_000u64) && r <= w(pow10(32))).then(|| U256::from(r))
}

/// The exact price the pool's integer state implies: raw `b` per raw `a`.
fn implied(
    model: Model,
    a: Address,
    ra: U256,
    b: Address,
    rb: U256,
) -> Option<(mev_dex::Pool, Ratio)> {
    let (t0, t1, r0, r1) = if a < b {
        (a, b, ra, rb)
    } else {
        (b, a, rb, ra)
    };
    let (pool, _) = build_pool(
        model,
        Address::with_last_byte(0xa0 ^ a.0[19] ^ b.0[19]),
        t0,
        t1,
        r0,
        r1,
        true,
    )?;
    // The pool prices `a` in `b` from its own integer state (reserves, or sqrt price²).
    let (rate, _) = pool.integer_rate(a)?;
    Some((pool, rate))
}

fn ratio_eq(x: &Ratio, y: &Ratio) -> bool {
    // a/b == c/d  <=>  a·d == c·b; read through `value` on a common large amount to stay in the public API.
    let probe = U256::from(1u8) << 200;
    x.value(probe, false) == y.value(probe, false) && x.value(probe, true) == y.value(probe, true)
}

#[test]
fn usd_prices_and_profit_floors_are_exact_at_every_decimal_count() {
    let mut checked = 0usize;
    let mut covered = std::collections::BTreeSet::new();
    for model in MODELS {
        for dec in DECIMALS {
            for ud in USD_DECIMALS {
                for price in PRICES {
                    for via_native in [false, true] {
                        let label = format!(
                            "{model:?} token {dec}dp ${price:?} usd {ud}dp via native {via_native}"
                        );
                        let Some(r_tok) = raw_for_depth(dec, price) else {
                            continue;
                        };
                        covered.insert(dec);
                        let Some(r_usd) = raw_for_depth(ud, (1, 0)) else {
                            continue;
                        };
                        let mut store = PoolStore::default();
                        // The token's price: `model` pool to USD, or `model` pool to native plus a V2 native/USD pool.
                        let expected = if via_native {
                            let r_nat = raw_for_depth(NATIVE_DECIMALS, NATIVE_PRICE).unwrap();
                            let Some((p1, tok_in_native)) = implied(model, T, r_tok, NATIVE, r_nat)
                            else {
                                continue;
                            };
                            let (p2, native_in_usd) =
                                implied(Model::V2 { fee_bps: 30 }, NATIVE, r_nat, USD, r_usd)
                                    .unwrap();
                            store.insert(p1);
                            store.insert(p2);
                            tok_in_native.multiply(&native_in_usd).unwrap()
                        } else {
                            let Some((p, tok_in_usd)) = implied(model, T, r_tok, USD, r_usd) else {
                                continue;
                            };
                            store.insert(p);
                            tok_in_usd
                        };
                        // Micro-USD per raw token unit: raw USD per raw token × 1e6 / 10^ud.
                        let usd_unit = Ratio::new(U512::from(1_000_000u64), w(pow10(ud))).unwrap();
                        let expected = expected.multiply(&usd_unit).unwrap();
                        let got = store
                            .usd_price_integer(T, USD, ud as u8, NATIVE)
                            .unwrap_or_else(|| panic!("{label}: no integer price"));
                        assert!(
                            ratio_eq(&got, &expected),
                            "{label}: integer price differs from the pools' own"
                        );

                        // Economic: one whole token is worth its USD price, to the pools' rounding.
                        let whole = got.value(pow10(dec), false).unwrap();
                        let want = U256::from(price.0)
                            * pow10(6)
                            * if price.1 >= 0 {
                                pow10(price.1 as u32)
                            } else {
                                U256::from(1u8)
                            }
                            / if price.1 < 0 {
                                pow10((-price.1) as u32)
                            } else {
                                U256::from(1u8)
                            };
                        let diff = if whole > want {
                            whole - want
                        } else {
                            want - whole
                        };
                        assert!(
                            diff <= want / U256::from(100_000u32) + U256::from(1u8),
                            "{label}: one whole token is worth {whole} µ$, expected {want} µ$"
                        );

                        // Profit floors: the least raw amount whose value covers the cost. `value(raw, false)` is
                        // floor(raw × price), and floor(x) ≥ cost ⟺ x ≥ cost for an integer cost: an exact test.
                        for cost in COSTS {
                            let floor = got.raw_for_cost(cost).unwrap();
                            let covers = |raw: U256| {
                                got.value(raw, false).is_some_and(|v| v >= U256::from(cost))
                            };
                            assert!(
                                covers(floor),
                                "{label}: floor {floor} does not cover {cost} µ$"
                            );
                            assert!(
                                floor.is_zero() || !covers(floor - U256::from(1u8)),
                                "{label}: floor {floor} is not the least amount covering {cost} µ$"
                            );
                            // In whole tokens the floor is cost / price, whatever the decimals.
                            let human = f64::from(floor) / 10f64.powi(dec as i32);
                            let px = price.0 as f64 * 10f64.powi(price.1);
                            let want = cost as f64 / 1e6 / px;
                            let one_raw = 1.0 / 10f64.powi(dec as i32);
                            assert!(
                                (human - want).abs() <= want * 1e-5 + one_raw,
                                "{label}: floor for {cost} µ$ is {human} tokens, expected {want}"
                            );
                            checked += 1;
                        }

                        // Float path: search pricing and loan caps.
                        let f = store.usd_per_raw_unit(T, USD, ud as u8, NATIVE).unwrap();
                        let fi = f64::from(got.value(pow10(30), false).unwrap()) / 1e30 / 1e6;
                        assert!(
                            (f - fi).abs() <= fi * 1e-9,
                            "{label}: f64 {f:e} vs integer {fi:e} USD per raw"
                        );
                        let cap_raw = mev_risk::loan_cap(50_000.0, f, f64::MAX, 1.0);
                        let cap_usd = cap_raw / 10f64.powi(dec as i32)
                            * (price.0 as f64 * 10f64.powi(price.1));
                        assert!(
                            (cap_usd - 50_000.0).abs() <= 50_000.0 * 1e-5,
                            "{label}: loan cap worth ${cap_usd}"
                        );
                    }
                }
            }
        }
    }
    eprintln!("price matrix: {checked} profit floors checked");
    // Some (decimals, price) pairs cannot form a pool (a 0-decimal token at $61,000 is 1,639 raw units
    // for $100M): those are skipped, but every decimal count must still be priced.
    assert_eq!(covered.into_iter().collect::<Vec<_>>(), DECIMALS.to_vec());
    assert!(checked >= 8_000, "{checked}");
}
