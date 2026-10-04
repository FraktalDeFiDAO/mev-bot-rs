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
/// Getter calls per contract per resolution round.
const GETTERS: usize = 7;

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
        let mut calls = Vec::with_capacity(frontier.len() * GETTERS);
        for (c, _) in &frontier {
            calls.push(call(*c, IPriceSource::aggregatorCall {}));
            calls.push(call(*c, IPriceSource::ASSET_TO_USD_AGGREGATORCall {}));
            calls.push(call(*c, IPriceSource::BASE_TO_USD_AGGREGATORCall {}));
            calls.push(call(*c, IPriceSource::BASE_FEED_1Call {}));
            calls.push(call(*c, IPriceSource::BASE_FEED_2Call {}));
            calls.push(call(*c, IPriceSource::QUOTE_FEED_1Call {}));
            calls.push(call(*c, IPriceSource::QUOTE_FEED_2Call {}));
        }
        let res = multicall(p, &calls, 200).await?;
        let mut next = Vec::new();
        for (i, (c, origin)) in frontier.iter().enumerate() {
            let r = &res[GETTERS * i..GETTERS * (i + 1)];
            let nz = |a: Option<Address>| a.filter(|a| !a.is_zero());
            let agg = nz(decode::<IPriceSource::aggregatorCall>(&r[0]));
            // Adapters / composite oracles: the proxies they wrap.
            let wrapped: Vec<Address> = [
                nz(decode::<IPriceSource::ASSET_TO_USD_AGGREGATORCall>(&r[1])),
                nz(decode::<IPriceSource::BASE_TO_USD_AGGREGATORCall>(&r[2])),
                nz(decode::<IPriceSource::BASE_FEED_1Call>(&r[3])),
                nz(decode::<IPriceSource::BASE_FEED_2Call>(&r[4])),
                nz(decode::<IPriceSource::QUOTE_FEED_1Call>(&r[5])),
                nz(decode::<IPriceSource::QUOTE_FEED_2Call>(&r[6])),
            ]
            .into_iter()
            .flatten()
            .collect();
            if let Some(a) = agg {
                // A Chainlink proxy: `aggregator()` is the contract receiving price updates.
                out.entry(a).or_default().push(*origin);
            } else if !wrapped.is_empty() {
                next.extend(wrapped.into_iter().map(|x| (x, *origin)));
            } else {
                // Unknown shape (custom oracle): watch the contract itself — harmless if unused.
                debug!(source = %c, "price source has no known aggregator getter; watching it directly");
                out.entry(*c).or_default().push(*origin);
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

/// Chainlink node operators submit reports through per-operator `AuthorizedForwarder` contracts:
/// the tx goes to the forwarder, calling `forward(address target, bytes data)` with the aggregator
/// as `target` (verified on Arbitrum ETH/USD, 2026-10-03: every update had a different `to`).
pub const FORWARD_SELECTOR: [u8; 4] = [0x6f, 0xad, 0xcf, 0x72];

/// The contract a transaction ultimately acts on: the forwarded target for forwarder calls,
/// otherwise `to`.
pub fn effective_target(to: Option<Address>, input: &[u8]) -> Option<Address> {
    if input.len() >= 36 && input[..4] == FORWARD_SELECTOR {
        // First ABI word after the selector: left-padded address.
        return Some(Address::from_slice(&input[16..36]));
    }
    to
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

    /// Tags interested in a transaction, looking through Chainlink forwarders.
    pub fn matches_tx(&self, to: Option<Address>, input: &[u8]) -> &[T] {
        let direct = self.matches(to);
        if !direct.is_empty() {
            return direct;
        }
        self.matches(effective_target(to, input))
    }

    /// The watched contract a transaction updates (directly or through a forwarder).
    pub fn matched_contract(&self, to: Option<Address>, input: &[u8]) -> Option<Address> {
        [to, effective_target(to, input)]
            .into_iter()
            .flatten()
            .find(|a| self.by_contract.contains_key(a))
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

    fn forward_call(target: Address) -> Vec<u8> {
        let mut v = FORWARD_SELECTOR.to_vec();
        v.extend([0u8; 12]);
        v.extend(target.as_slice());
        v.extend([0u8; 64]); // bytes offset + length (contents irrelevant)
        v
    }

    #[test]
    fn forwarded_reports_match_their_aggregator() {
        let agg = Address::with_last_byte(0xaa);
        let forwarder = Address::with_last_byte(0xf0);
        let mut w = FeedWatcher::default();
        w.watch(agg, "eth-usd");
        assert_eq!(
            w.matches_tx(Some(forwarder), &forward_call(agg)),
            &["eth-usd"]
        );
        assert!(
            w.matches_tx(Some(forwarder), &forward_call(Address::with_last_byte(1)))
                .is_empty()
        );
        assert_eq!(
            w.matched_contract(Some(forwarder), &forward_call(agg)),
            Some(agg)
        );
        assert_eq!(w.matched_contract(Some(agg), &[]), Some(agg));
        assert_eq!(w.matched_contract(Some(forwarder), &[]), None);
        // Direct transmit to the aggregator still matches.
        assert_eq!(
            w.matches_tx(Some(agg), &[0xb1, 0xdc, 0x65, 0xa4]),
            &["eth-usd"]
        );
        // Unrelated selector / short input: no match.
        assert!(w.matches_tx(Some(forwarder), &[0x6f, 0xad]).is_empty());
    }

    #[test]
    fn effective_target_reads_first_forward_argument() {
        let agg = Address::with_last_byte(0xaa);
        assert_eq!(
            effective_target(Some(Address::ZERO), &forward_call(agg)),
            Some(agg)
        );
        assert_eq!(effective_target(Some(agg), &[1, 2, 3, 4]), Some(agg));
    }
}
