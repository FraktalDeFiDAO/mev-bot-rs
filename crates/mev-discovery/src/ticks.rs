//! Initialized-tick loading for Uniswap V3-style pools (Uniswap V3, PancakeV3): the `tickBitmap`
//! words around the current price, then each set tick's `liquidityNet`, so the model can price a
//! swap across tick boundaries (`mev_dex::TickMap`).

use std::collections::HashMap;

use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use anyhow::Result;
use mev_bindings::{IPoolManager, IUniswapV3Pool, decode_v4_slot0, v4_slot_plus, v4_state_slot};
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
    liquidity_net_exact(ret).map(|n| n as f64)
}

/// The same word as an exact `i128`.
pub fn liquidity_net_exact(ret: &[u8]) -> Option<i128> {
    let word = ret.get(32..64)?;
    // int128 sign-extended to 256 bits: reinterpret the low 16 bytes.
    Some(i128::from_be_bytes(word[16..32].try_into().ok()?))
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

/// A mapping slot: `keccak256(key (sign-extended to 32 bytes) ++ slot)`.
pub fn mapping_slot(key: i32, slot: B256) -> B256 {
    let mut buf = [if key < 0 { 0xff } else { 0 }; 64];
    buf[28..32].copy_from_slice(&key.to_be_bytes());
    buf[32..].copy_from_slice(slot.as_slice());
    keccak256(buf)
}

/// `liquidityNet` of a V4 `TickInfo` first word: `liquidityGross` in the low 128 bits, the signed
/// `liquidityNet` in the high 128.
pub fn v4_liquidity_net(word: &[u8]) -> Option<f64> {
    v4_liquidity_net_exact(word).map(|n| n as f64)
}

pub fn v4_liquidity_net_exact(word: &[u8]) -> Option<i128> {
    Some(i128::from_be_bytes(word.get(..16)?.try_into().ok()?))
}

/// How a pool's ticks are read: its own contract (V3-style) or the V4 PoolManager's storage.
#[derive(Clone, Copy)]
enum Style {
    V3,
    /// Camelot's Algebra pool: `globalState`/`tickTable`/`ticks`; the table is indexed by raw tick (spacing 1).
    Algebra,
    V4 {
        manager: Address,
        base: B256,
        flipped: bool,
    },
}

struct Plan {
    pool: Address,
    style: Style,
    spacing: i32,
    lo: i32,
    hi: i32,
    words: Vec<i32>,
}

impl Plan {
    fn slot0_call(&self) -> (Address, Bytes) {
        match self.style {
            Style::V3 => call(self.pool, IUniswapV3Pool::slot0Call {}),
            Style::Algebra => call(self.pool, mev_bindings::IAlgebraPool::globalStateCall {}),
            Style::V4 { manager, base, .. } => {
                call(manager, IPoolManager::extsloadCall { slot: base })
            }
        }
    }

    fn bitmap_call(&self, word: i32) -> (Address, Bytes) {
        match self.style {
            Style::V3 => call(
                self.pool,
                IUniswapV3Pool::tickBitmapCall {
                    wordPosition: word as i16,
                },
            ),
            Style::Algebra => call(
                self.pool,
                mev_bindings::IAlgebraPool::tickTableCall { row: word as i16 },
            ),
            Style::V4 { manager, base, .. } => call(
                manager,
                IPoolManager::extsloadCall {
                    slot: mapping_slot(word, v4_slot_plus(base, 5)),
                },
            ),
        }
    }

    fn tick_call(&self, tick: i32) -> (Address, Bytes) {
        match self.style {
            Style::V3 => call(
                self.pool,
                IUniswapV3Pool::ticksCall {
                    tick: tick.try_into().expect("tick fits int24"),
                },
            ),
            Style::Algebra => call(
                self.pool,
                mev_bindings::IAlgebraPool::ticksCall {
                    tick: tick.try_into().expect("tick fits int24"),
                },
            ),
            Style::V4 { manager, base, .. } => call(
                manager,
                IPoolManager::extsloadCall {
                    slot: mapping_slot(tick, v4_slot_plus(base, 4)),
                },
            ),
        }
    }

    /// `(sqrt price, tick)` as the pool reports them, in the pool's own orientation.
    fn parse_slot0(&self, ret: &[u8]) -> Option<(f64, i32)> {
        match self.style {
            Style::V3 | Style::Algebra => slot0_hint(ret),
            Style::V4 { .. } => {
                let (sqrt, tick, _, _) = decode_v4_slot0(B256::try_from(ret.get(..32)?).ok()?);
                Some((mev_dex::sqrt_price_from_x96(f64::from(sqrt)), tick))
            }
        }
    }

    fn parse_net(&self, ret: &[u8]) -> Option<i128> {
        match self.style {
            Style::V3 | Style::Algebra => liquidity_net_exact(ret),
            Style::V4 { .. } => v4_liquidity_net_exact(ret),
        }
    }

    /// Our model's view of what the pool reported: V4 pools whose token order we flipped are mirrored
    /// (price inverted, `tick -> -tick`, `liquidityNet -> -liquidityNet`; the price sits in
    /// `(b^-(t+1), b^-t]` so the tick containing it is `-t - 1`).
    fn finish(&self, exact: Vec<(i32, i128)>, hint: (f64, i32)) -> TickMap {
        let flipped = matches!(self.style, Style::V4 { flipped: true, .. });
        let float = |v: &[(i32, i128)]| v.iter().map(|&(t, n)| (t, n as f64)).collect();
        if !flipped {
            return TickMap {
                ticks: float(&exact),
                exact,
                lo: self.lo,
                hi: self.hi,
                hint: Some(hint),
            };
        }
        let mut exact: Vec<(i32, i128)> = exact
            .into_iter()
            .map(|(t, n)| (-t, n.saturating_neg()))
            .collect();
        exact.sort_by_key(|(t, _)| *t);
        TickMap {
            ticks: float(&exact),
            exact,
            lo: -self.hi,
            hi: -self.lo,
            hint: Some((1.0 / hint.0, -hint.1 - 1)),
        }
    }
}

/// Tick maps for the concentrated-liquidity pools in `pools`: V3-style (Uniswap V3, PancakeV3) and
/// Uniswap V4. Others are skipped; a pool whose reads fail is left out and keeps the bucket model.
pub async fn load_ticks(
    rpc: &Rpc,
    pools: &[Pool],
    half_window: i32,
) -> Result<HashMap<Address, TickMap>> {
    let (block, _) = mev_rpc::snapshot_block(&rpc.http).await?;
    load_ticks_at(rpc, pools, half_window, block).await
}

pub async fn load_ticks_at(
    rpc: &Rpc,
    pools: &[Pool],
    half_window: i32,
    block: alloy::eips::BlockId,
) -> Result<HashMap<Address, TickMap>> {
    let mut plans = Vec::new();
    for p in pools {
        let (style, tick) = match p.info.kind {
            PoolKind::UniV3 { .. } | PoolKind::PancakeV3 { .. } => match current_tick(p) {
                Some(t) => (Style::V3, t),
                None => continue,
            },
            PoolKind::AlgebraV3 => match current_tick(p) {
                Some(t) => (Style::Algebra, t),
                None => continue,
            },
            // The V4 tick comes from its own slot0 below; this one only places the window, in the
            // pool's orientation (mirrored back for flipped pools).
            PoolKind::UniV4 {
                id,
                manager,
                flipped,
                ..
            } => match current_tick(p) {
                Some(t) => (
                    Style::V4 {
                        manager,
                        base: v4_state_slot(id),
                        flipped,
                    },
                    if flipped { -t } else { t },
                ),
                None => continue,
            },
            _ => continue,
        };
        // Algebra's table has no spacing compression: a row is 256 consecutive ticks.
        let spacing = if matches!(style, Style::Algebra) {
            1
        } else {
            tick_spacing(&p.info.kind) as i32
        };
        let (words, lo, hi) = window(tick, spacing, half_window);
        plans.push(Plan {
            pool: p.info.address,
            style,
            spacing,
            lo,
            hi,
            words,
        });
    }
    // All rounds use the same chain snapshot.
    // Round 1: slot0 and every bitmap word.
    let calls: Vec<_> = plans
        .iter()
        .flat_map(|pl| {
            std::iter::once(pl.slot0_call()).chain(pl.words.iter().map(|w| pl.bitmap_call(*w)))
        })
        .collect();
    let res = rpc.multicall_at(&calls, block).await?;
    // Round 2: the tick info of every set bit; a pool with any failed read is dropped.
    let mut i = 0;
    let mut wanted: Vec<(usize, i32)> = Vec::new();
    let mut broken = vec![false; plans.len()];
    let mut hints: Vec<Option<(f64, i32)>> = vec![None; plans.len()];
    for (pi, pl) in plans.iter().enumerate() {
        hints[pi] = res[i].as_ref().and_then(|b| pl.parse_slot0(b));
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
        .map(|(pi, t)| plans[*pi].tick_call(*t))
        .collect();
    let res = rpc.multicall_at(&calls, block).await?;
    let mut maps: HashMap<usize, Vec<(i32, i128)>> = HashMap::new();
    for ((pi, t), r) in wanted.iter().zip(&res) {
        match r.as_ref().and_then(|b| plans[*pi].parse_net(b)) {
            Some(net) => maps.entry(*pi).or_default().push((*t, net)),
            None => broken[*pi] = true,
        }
    }
    let mut out = HashMap::new();
    for (pi, pl) in plans.iter().enumerate() {
        let (false, Some(hint)) = (broken[pi], hints[pi]) else {
            continue;
        };
        let mut ticks = maps.remove(&pi).unwrap_or_default();
        ticks.sort_by_key(|(t, _)| *t);
        out.insert(pl.pool, pl.finish(ticks, hint));
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

    #[test]
    fn mapping_slots_sign_extend_negative_keys() {
        let base = B256::repeat_byte(7);
        // keccak(key ++ slot) with key = -1 as 32 bytes of 0xff.
        let mut buf = [0xffu8; 64];
        buf[32..].copy_from_slice(base.as_slice());
        assert_eq!(mapping_slot(-1, base), keccak256(buf));
        let mut buf = [0u8; 64];
        buf[31] = 5;
        buf[32..].copy_from_slice(base.as_slice());
        assert_eq!(mapping_slot(5, base), keccak256(buf));
    }

    #[test]
    fn v4_liquidity_net_is_the_signed_high_half() {
        let mut w = [0u8; 32];
        w[..16].copy_from_slice(&(-7i128).to_be_bytes());
        w[16..].copy_from_slice(&9u128.to_be_bytes()); // gross
        assert_eq!(v4_liquidity_net(&w), Some(-7.0));
    }

    #[test]
    fn a_flipped_pool_is_mirrored() {
        let pl = Plan {
            pool: Address::ZERO,
            style: Style::V4 {
                manager: Address::ZERO,
                base: B256::ZERO,
                flipped: true,
            },
            spacing: 10,
            lo: -100,
            hi: 300,
            words: vec![],
        };
        // Position [-20, 60) with L = 5 (+5 at -20, -5 at 60); the pool's price is 2.0 at tick 40.
        let m = pl.finish(vec![(-20, 5), (60, -5)], (2.0, 40));
        assert_eq!(m.ticks, vec![(-60, 5.0), (20, -5.0)]);
        assert_eq!(m.exact, vec![(-60, 5), (20, -5)]);
        assert_eq!((m.lo, m.hi), (-300, 100));
        assert_eq!(m.hint, Some((0.5, -41)));
    }
}
