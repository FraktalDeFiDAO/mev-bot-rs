//! Price-feed resolution and matching.
//!
//! Lending protocols read prices through proxies and adapters; the contract that actually receives
//! the price-update transaction is the underlying Chainlink aggregator. `resolve_aggregators`
//! follows sources down to those aggregators so a sequencer-feed transaction can be matched to the
//! assets whose price it moves.

use std::collections::{HashMap, HashSet};

use alloy::{primitives::Address, providers::DynProvider};
use anyhow::Result;
use mev_bindings::IPriceSource;
use mev_rpc::{call, decode, multicall};
use tracing::debug;

/// How deep to follow adapter -> proxy -> aggregator chains.
const MAX_DEPTH: usize = 4;

/// Map each underlying aggregator to the original sources (e.g. Aave oracle sources) it feeds.
pub async fn resolve_aggregators(
    p: &DynProvider,
    sources: &[Address],
) -> Result<HashMap<Address, Vec<Address>>> {
    let mut out: HashMap<Address, Vec<Address>> = HashMap::new();
    // (current contract, original source)
    let mut frontier: Vec<(Address, Address)> = sources.iter().map(|s| (*s, *s)).collect();
    let mut seen: HashSet<(Address, Address)> = HashSet::new();
    for _ in 0..MAX_DEPTH {
        frontier.retain(|x| seen.insert(*x));
        if frontier.is_empty() {
            break;
        }
        let mut calls = Vec::with_capacity(frontier.len() * 3);
        for (c, _) in &frontier {
            calls.push(call(*c, IPriceSource::aggregatorCall {}));
            calls.push(call(*c, IPriceSource::ASSET_TO_USD_AGGREGATORCall {}));
            calls.push(call(*c, IPriceSource::BASE_TO_USD_AGGREGATORCall {}));
        }
        let res = multicall(p, &calls, 200).await?;
        let mut next = Vec::new();
        for (i, (c, origin)) in frontier.iter().enumerate() {
            let agg = decode::<IPriceSource::aggregatorCall>(&res[3 * i]).filter(|a| !a.is_zero());
            let asset_usd = decode::<IPriceSource::ASSET_TO_USD_AGGREGATORCall>(&res[3 * i + 1])
                .filter(|a| !a.is_zero());
            let base_usd = decode::<IPriceSource::BASE_TO_USD_AGGREGATORCall>(&res[3 * i + 2])
                .filter(|a| !a.is_zero());
            match (agg, asset_usd, base_usd) {
                // A Chainlink proxy: `aggregator()` is the contract receiving price updates.
                (Some(a), _, _) => out.entry(a).or_default().push(*origin),
                // Adapters: follow to the proxies they wrap.
                (None, a, b) if a.is_some() || b.is_some() => {
                    next.extend(a.into_iter().chain(b).map(|x| (x, *origin)));
                }
                // Neither: treat the contract itself as the feed (custom oracle) — harmless if unused.
                _ => {
                    debug!(source = %c, "price source has no known aggregator getter; watching it directly");
                    out.entry(*c).or_default().push(*origin);
                }
            }
        }
        frontier = next;
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(out)
}

/// Matches transaction targets against watched feed contracts.
#[derive(Debug, Default, Clone)]
pub struct FeedWatcher<T: Clone> {
    by_contract: HashMap<Address, Vec<T>>,
}

impl<T: Clone> FeedWatcher<T> {
    pub fn watch(&mut self, contract: Address, tag: T) {
        self.by_contract.entry(contract).or_default().push(tag);
    }

    /// Tags interested in a transaction sent to `to`.
    pub fn matches(&self, to: Option<Address>) -> &[T] {
        to.and_then(|a| self.by_contract.get(&a))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.by_contract.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_contract.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_matches_only_watched_targets() {
        let feed = Address::with_last_byte(1);
        let mut w = FeedWatcher::default();
        w.watch(feed, 7usize);
        w.watch(feed, 9usize);
        assert_eq!(w.matches(Some(feed)), &[7, 9]);
        assert!(w.matches(Some(Address::with_last_byte(2))).is_empty());
        assert!(w.matches(None).is_empty());
        assert_eq!(w.len(), 1);
    }
}
