//! Triangular arbitrage: cycles `L -> X -> Y -> L` over three distinct pools, enumerated once and
//! indexed by pool, so a pool update re-prices only the cycles that contain it (no global scans).
//! Pricing uses the f64 pool model as a filter; the executor's on-chain profit check (and, where
//! available, local REVM simulation) is the authority.

use std::collections::HashMap;

use alloy::primitives::U256;
use mev_state::PoolStore;
use mev_types::{Address, Opportunity};

use crate::{SearchParams, hop, size_route};

/// One directed cycle: (pool, token_in, token_out) per leg, starting and ending at `loan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub loan: Address,
    pub legs: [(Address, Address, Address); 3],
}

/// All indexed triangular cycles and, per pool, the cycles that use it.
#[derive(Debug, Default)]
pub struct CycleIndex {
    cycles: Vec<Cycle>,
    by_pool: HashMap<Address, Vec<u32>>,
}

impl CycleIndex {
    /// Enumerate directed triangles starting at each loan token. `max_cycles` is shared equally
    /// across loan tokens (so tokens listed last still get cycles), and within a token the deepest
    /// cycles are kept: ranked by the smaller loan-token reserve of the first and last pool (both in
    /// that token's units). Enumeration stops at 20x the quota per token to bound start-up work.
    pub fn build(store: &PoolStore, loans: &[Address], max_cycles: usize) -> Self {
        // token -> [(pool, other token)]
        let mut adj: HashMap<Address, Vec<(Address, Address)>> = HashMap::new();
        for p in store.pools() {
            let (a, b, pool) = (p.info.token0, p.info.token1, p.info.address);
            adj.entry(a).or_default().push((pool, b));
            adj.entry(b).or_default().push((pool, a));
        }
        let reserve = |pool: &Address, token: Address| {
            store.get(pool).map(|p| p.reserve_of(token)).unwrap_or(0.0)
        };
        let quota = max_cycles / loans.len().max(1);
        let mut idx = CycleIndex::default();
        for &l in loans {
            let Some(first) = adj.get(&l) else { continue };
            let mut found: Vec<(f64, Cycle)> = Vec::new();
            'loan: for &(p1, x) in first {
                if x == l {
                    continue;
                }
                for &(p2, y) in adj.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                    if p2 == p1 || y == l || y == x {
                        continue;
                    }
                    for &(p3, back) in adj.get(&y).map(Vec::as_slice).unwrap_or(&[]) {
                        if back != l || p3 == p1 || p3 == p2 {
                            continue;
                        }
                        if found.len() >= quota.saturating_mul(20) {
                            break 'loan;
                        }
                        let depth = reserve(&p1, l).min(reserve(&p3, l));
                        found.push((
                            depth,
                            Cycle {
                                loan: l,
                                legs: [(p1, l, x), (p2, x, y), (p3, y, l)],
                            },
                        ));
                    }
                }
            }
            found.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (_, c) in found.into_iter().take(quota) {
                let id = idx.cycles.len() as u32;
                for (p, _, _) in c.legs {
                    idx.by_pool.entry(p).or_default().push(id);
                }
                idx.cycles.push(c);
            }
        }
        idx
    }

    pub fn len(&self) -> usize {
        self.cycles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
    }

    /// Cycles that include `pool`.
    pub fn affected(&self, pool: &Address) -> impl Iterator<Item = &Cycle> {
        self.by_pool
            .get(pool)
            .into_iter()
            .flatten()
            .map(|&i| &self.cycles[i as usize])
    }

    /// Price the cycles touching `changed`; profitable ones (est. before gas) best-first.
    pub fn arbs(
        &self,
        store: &PoolStore,
        changed: Address,
        params: &SearchParams,
    ) -> Vec<Opportunity> {
        let mut out = Vec::new();
        for c in self.affected(&changed) {
            let Some(&max_loan) = params.max_loan.get(&c.loan) else {
                continue;
            };
            let Some(usd) = (params.usd_per_raw)(c.loan) else {
                continue;
            };
            let pools: Vec<_> = c.legs.iter().filter_map(|(p, _, _)| store.get(p)).collect();
            if pools.len() != 3 {
                continue;
            }
            // Screen: the marginal round trip must beat 1 after fees.
            let rate: f64 = pools
                .iter()
                .zip(&c.legs)
                .map(|(p, (_, tin, _))| p.marginal_rate(*tin))
                .product();
            if rate <= 1.0 {
                continue;
            }
            // The whole loan range: profit is concave in the input (see `best_size`).
            let cap = max_loan;
            let path = [
                (pools[0], c.legs[0].1),
                (pools[1], c.legs[1].1),
                (pools[2], c.legs[2].1),
            ];
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
                loan_token: c.loan,
                amount_in: U256::from(amount as u128),
                hops: pools
                    .iter()
                    .zip(&c.legs)
                    .map(|(p, (_, tin, tout))| hop(p, *tin, *tout))
                    .collect(),
                est_profit: profit,
                est_profit_usd: profit_usd,
                buy_pool: c.legs[0].0,
                sell_pool: c.legs[2].0,
                exact,
                versions: vec![],
            });
        }
        out.sort_by(|a, b| b.est_profit_usd.total_cmp(&a.est_profit_usd));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_dex::{Pool, PoolState};
    use mev_types::{PoolInfo, PoolKind};

    fn tok(n: u8) -> Address {
        Address::with_last_byte(n)
    }

    fn v2(addr: u8, t0: Address, t1: Address, r0: f64, r1: f64) -> Pool {
        let (token0, token1, reserve0, reserve1) = if t0 < t1 {
            (t0, t1, r0, r1)
        } else {
            (t1, t0, r1, r0)
        };
        Pool::new(
            PoolInfo {
                address: Address::with_last_byte(addr),
                dex: "t".into(),
                kind: PoolKind::UniV2 { fee_bps: 30 },
                token0,
                token1,
            },
            PoolState::V2 { reserve0, reserve1 },
        )
    }

    /// L/X, X/Y, Y/L priced so going L -> X -> Y -> L gains ~5% before fees.
    fn store(mispriced: bool) -> PoolStore {
        let (l, x, y) = (tok(1), tok(2), tok(3));
        let mut s = PoolStore::default();
        s.insert(v2(11, l, x, 1e9, 2e9)); // 1 L = 2 X
        s.insert(v2(12, x, y, 2e9, 2e9)); // 1 X = 1 Y
        let back = if mispriced { 2.1e9 } else { 2e9 }; // 2 Y = 1 L (or 1.05 L when mispriced)
        s.insert(v2(13, y, l, 2e9, back / 2.0 * 1.0));
        s
    }

    fn params<'a>(
        max: &'a HashMap<Address, f64>,
        px: &'a dyn Fn(Address) -> Option<f64>,
    ) -> SearchParams<'a> {
        SearchParams {
            max_loan: max,
            usd_per_raw: px,
            min_est_profit_usd: 0.0,
        }
    }

    #[test]
    fn enumerates_each_directed_triangle_once_and_indexes_by_pool() {
        let s = store(true);
        let idx = CycleIndex::build(&s, &[tok(1)], 100);
        // Two directions around the one triangle.
        assert_eq!(idx.len(), 2);
        for pool in [11, 12, 13] {
            assert_eq!(idx.affected(&Address::with_last_byte(pool)).count(), 2);
        }
        assert_eq!(idx.affected(&Address::with_last_byte(99)).count(), 0);
        assert_eq!(
            CycleIndex::build(&s, &[tok(1)], 1).len(),
            1,
            "cap respected"
        );
    }

    #[test]
    fn cap_is_shared_across_loans_and_keeps_the_deepest_cycles() {
        let (l, x, y) = (tok(1), tok(2), tok(3));
        let mut s = store(true);
        // A second, deeper L/X pool: its cycles must win when the quota is tight.
        s.insert(v2(14, l, x, 1e12, 2e12));
        // Cap 2 shared by two loans: one cycle each, not both for the first-listed loan.
        let idx = CycleIndex::build(&s, &[l, y], 2);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.cycles.iter().filter(|c| c.loan == l).count(), 1);
        assert_eq!(idx.cycles.iter().filter(|c| c.loan == y).count(), 1);
        let deep = Address::with_last_byte(14);
        let c = idx.cycles.iter().find(|c| c.loan == l).unwrap();
        assert!(
            c.legs[0].0 == deep || c.legs[2].0 == deep,
            "deepest L cycle kept"
        );
        let _ = x;
    }

    #[test]
    fn prices_only_the_profitable_direction() {
        let max: HashMap<Address, f64> = [(tok(1), 1e8)].into_iter().collect();
        let px = |_| Some(1.0);
        let idx = CycleIndex::build(&store(true), &[tok(1)], 100);
        let opps = idx.arbs(
            &store(true),
            Address::with_last_byte(12),
            &params(&max, &px),
        );
        assert_eq!(opps.len(), 1, "one direction profits");
        let o = &opps[0];
        assert_eq!(o.hops.len(), 3);
        assert_eq!(o.hops[0].token_in, tok(1));
        assert_eq!(o.hops[2].token_out, tok(1));
        assert!(o.hops.windows(2).all(|w| w[0].token_out == w[1].token_in));
        assert!(o.est_profit > 0.0);
        // Fair prices: fees make every direction unprofitable.
        let fair = CycleIndex::build(&store(false), &[tok(1)], 100);
        assert!(
            fair.arbs(
                &store(false),
                Address::with_last_byte(12),
                &params(&max, &px)
            )
            .is_empty()
        );
    }
}
