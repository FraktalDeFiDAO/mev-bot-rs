//! Initialized-tick loading for Uniswap V3-style pools (Uniswap V3, PancakeV3): the `tickBitmap`
//! words around the current price, then each set tick's `liquidityNet`, so the model can price a
//! swap across tick boundaries (`mev_dex::TickMap`).

use std::collections::HashMap;

use alloy::primitives::{Address, U256};
use anyhow::Result;
use mev_bindings::IUniswapV3Pool;
use mev_dex::{Pool, PoolState, TickMap, tick_spacing};
use mev_rpc::{Rpc, call};
use mev_types::PoolKind;

const MIN_TICK: i32 = -887_272;
const MAX_TICK: i32 = 887_272;

/// The ticks set in one bitmap word: bit `b` of word `w` is tick `(w * 256 + b) * spacing`.
pub fn ticks_in_word(word_pos: i32, bitmap: U256, spacing: i32) -> Vec<i32> {
    (0..256)
        .filter(|b| bitmap.bit(*b))
        .map(|b| (word_pos * 256 + b as i32) * spacing)
        .collect()
}

/// The tick window `[tick - half, tick + half]` for a pool at `tick`, and the bitmap word positions
/// that cover it. A word spans 256 spacings.
pub fn window(tick: i32, spacing: i32, half: i32) -> (Vec<i32>, i32, i32) {
    let lo = (tick - half).max(MIN_TICK);
    let hi = (tick + half).min(MAX_TICK);
    let word_of = |t: i32| t.div_euclid(spacing).div_euclid(256);
    ((word_of(lo)..=word_of(hi)).collect(), lo, hi)
}

/// `liquidityNet` (the second return word, an int128) from a `ticks(int24)` result. The tuple's
/// length differs between Uniswap and Pancake, so the word is read directly.
pub fn liquidity_net(ret: &[u8]) -> Option<f64> {
    let word = ret.get(32..64)?;
    // int128 sign-extended to 256 bits: reinterpret the low 16 bytes.
    let raw = i128::from_be_bytes(word[16..32].try_into().ok()?);
    Some(raw as f64)
}

/// `(sqrt price, tick)` from a `slot0()` result.
pub fn slot0_hint(ret: &[u8]) -> Option<(f64, i32)> {
    let price = U256::from_be_slice(ret.get(..32)?);
    let tick = i32::from_be_bytes(ret.get(60..64)?.try_into().ok()?);
    Some((mev_dex::sqrt_price_from_x96(f64::from(price)), tick))
}

fn current_tick(pool: &Pool) -> Option<i32> {
    let PoolState::V3 { sqrt_price, .. } = pool.state else {
        return None;
    };
    (sqrt_price > 0.0).then(|| ((sqrt_price * sqrt_price).ln() / 1.0001f64.ln()).floor() as i32)
}

/// Tick maps for the V3-style pools in `pools` (others are skipped). A pool whose reads fail is
/// left out and keeps the bucket model.
pub async fn load_ticks(
    rpc: &Rpc,
    pools: &[Pool],
    half_window: i32,
) -> Result<HashMap<Address, TickMap>> {
    struct Plan {
        pool: Address,
        spacing: i32,
        lo: i32,
        hi: i32,
        words: Vec<i32>,
    }
    let mut plans = Vec::new();
    for p in pools {
        if !matches!(
            p.info.kind,
            PoolKind::UniV3 { .. } | PoolKind::PancakeV3 { .. }
        ) {
            continue;
        }
        let Some(tick) = current_tick(p) else {
            continue;
        };
        let spacing = tick_spacing(&p.info.kind) as i32;
        let (words, lo, hi) = window(tick, spacing, half_window);
        plans.push(Plan {
            pool: p.info.address,
            spacing,
            lo,
            hi,
            words,
        });
    }
    // Round 1: every bitmap word.
    let calls: Vec<_> = plans
        .iter()
        .flat_map(|pl| {
            let slot0 = call(pl.pool, IUniswapV3Pool::slot0Call {});
            let words = pl.words.iter().map(|w| {
                call(
                    pl.pool,
                    IUniswapV3Pool::tickBitmapCall {
                        wordPosition: *w as i16,
                    },
                )
            });
            std::iter::once(slot0).chain(words)
        })
        .collect();
    let res = rpc.multicall(&calls).await?;
    // Round 2: `ticks(t)` for every set bit; a pool with any failed bitmap read is dropped.
    let mut i = 0;
    let mut wanted: Vec<(usize, i32)> = Vec::new(); // (plan index, tick)
    let mut broken = vec![false; plans.len()];
    let mut hints: Vec<Option<(f64, i32)>> = vec![None; plans.len()];
    for (pi, pl) in plans.iter().enumerate() {
        // slot0: sqrtPriceX96 (word 0) and the pool's own tick (word 1).
        hints[pi] = res[i].as_ref().and_then(|b| slot0_hint(b));
        broken[pi] = hints[pi].is_none();
        i += 1;
        for w in &pl.words {
            match res[i].as_ref().filter(|b| b.len() >= 32) {
                Some(b) => {
                    for t in ticks_in_word(*w, U256::from_be_slice(&b[..32]), pl.spacing) {
                        if (pl.lo..=pl.hi).contains(&t) {
                            wanted.push((pi, t));
                        }
                    }
                }
                None => broken[pi] = true,
            }
            i += 1;
        }
    }
    wanted.retain(|(pi, _)| !broken[*pi]);
    let calls: Vec<_> = wanted
        .iter()
        .map(|(pi, t)| {
            call(
                plans[*pi].pool,
                IUniswapV3Pool::ticksCall {
                    tick: (*t).try_into().expect("tick fits int24"),
                },
            )
        })
        .collect();
    let res = rpc.multicall(&calls).await?;
    let mut maps: HashMap<usize, Vec<(i32, f64)>> = HashMap::new();
    for ((pi, t), r) in wanted.iter().zip(&res) {
        match r.as_ref().and_then(|b| liquidity_net(b)) {
            Some(net) => maps.entry(*pi).or_default().push((*t, net)),
            None => broken[*pi] = true,
        }
    }
    let mut out = HashMap::new();
    for (pi, pl) in plans.iter().enumerate() {
        if broken[pi] {
            continue;
        }
        let mut ticks = maps.remove(&pi).unwrap_or_default();
        ticks.sort_by_key(|(t, _)| *t);
        out.insert(
            pl.pool,
            TickMap {
                ticks,
                lo: pl.lo,
                hi: pl.hi,
                hint: hints[pi],
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_bits_map_to_ticks_including_negative_words() {
        let bm = U256::from(1u8) | (U256::from(1u8) << 255);
        assert_eq!(ticks_in_word(0, bm, 10), vec![0, 2550]);
        // Word -1 holds ticks -256*spacing .. -1*spacing.
        assert_eq!(ticks_in_word(-1, bm, 10), vec![-2560, -10]);
        assert!(ticks_in_word(3, U256::ZERO, 60).is_empty());
    }

    #[test]
    fn the_window_is_centred_on_the_current_word() {
        // Spacing 10: a word is 2560 ticks. A window of +-800 around tick 25 stays in word 0.
        let (words, lo, hi) = window(25, 10, 800);
        assert_eq!((words, lo, hi), (vec![-1, 0], -775, 825));
        // Around -1 the window straddles words -1 and 0; spacing 1 needs more words for the same span.
        assert_eq!(window(-1, 10, 800).0, vec![-1, 0]);
        assert_eq!(window(0, 1, 800).0, vec![-4, -3, -2, -1, 0, 1, 2, 3]);
        // The ends are clamped to the valid tick range.
        assert_eq!(window(MAX_TICK, 60, 800).2, MAX_TICK);
    }

    #[test]
    fn liquidity_net_reads_the_signed_second_word() {
        let mut ret = vec![0u8; 64];
        ret[63] = 5; // +5
        assert_eq!(liquidity_net(&ret), Some(5.0));
        ret[32..64].copy_from_slice(&[0xff; 32]); // -1 sign-extended
        assert_eq!(liquidity_net(&ret), Some(-1.0));
        assert_eq!(liquidity_net(&[0u8; 40]), None);
    }
}
