//! Market analytics over the pool store: how far each pair is from a profitable round trip.

use mev_dex::Pool;
use mev_state::PoolStore;
use mev_types::Address;

#[derive(Debug, Clone)]
pub struct PairSpread {
    pub token0: Address,
    pub token1: Address,
    pub pools: usize,
    /// Best round-trip rate after fees minus 1, in basis points. > 0 means a marginal arb exists.
    pub best_edge_bps: f64,
    pub buy: Address,
    pub sell: Address,
}

/// For every pair with 2+ pools, the best marginal round-trip edge across ordered pool pairs.
pub fn pair_spreads(store: &PoolStore) -> Vec<PairSpread> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in store.pools() {
        let key = (p.info.token0, p.info.token1);
        if !seen.insert(key) {
            continue;
        }
        let sibs: Vec<&Pool> = store.pools_for(key.0, key.1);
        if sibs.len() < 2 {
            continue;
        }
        let mut best = (f64::MIN, Address::ZERO, Address::ZERO);
        for loan in [key.0, key.1] {
            let other = if loan == key.0 { key.1 } else { key.0 };
            for a in &sibs {
                for b in &sibs {
                    if a.info.address == b.info.address {
                        continue;
                    }
                    let edge = (a.marginal_rate(loan) * b.marginal_rate(other) - 1.0) * 10_000.0;
                    if edge > best.0 {
                        best = (edge, a.info.address, b.info.address);
                    }
                }
            }
        }
        out.push(PairSpread {
            token0: key.0,
            token1: key.1,
            pools: sibs.len(),
            best_edge_bps: best.0,
            buy: best.1,
            sell: best.2,
        });
    }
    out.sort_by(|a, b| b.best_edge_bps.total_cmp(&a.best_edge_bps));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_dex::PoolState;
    use mev_types::{PoolInfo, PoolKind};

    fn v2(addr: u8, r0: f64, r1: f64) -> Pool {
        let info = PoolInfo {
            address: Address::with_last_byte(addr),
            dex: "t".into(),
            kind: PoolKind::UniV2 { fee_bps: 30 },
            token0: Address::with_last_byte(1),
            token1: Address::with_last_byte(2),
        };
        Pool::new(
            info,
            PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
        )
    }

    #[test]
    fn detects_positive_and_negative_edges() {
        let mut s = PoolStore::default();
        s.insert(v2(10, 1e6, 2e6));
        s.insert(v2(11, 1e6, 2.2e6)); // 10% apart: well above 2 × 0.3% fees
        let spreads = pair_spreads(&s);
        assert_eq!(spreads.len(), 1);
        assert!(spreads[0].best_edge_bps > 0.0);
        assert_eq!(spreads[0].pools, 2);

        let mut t = PoolStore::default();
        t.insert(v2(10, 1e6, 2e6));
        t.insert(v2(11, 1e6, 2e6)); // identical prices: fees make every round trip negative
        assert!(pair_spreads(&t)[0].best_edge_bps < 0.0);
    }
}
