//! Price-feed resolution and matching.
//!
//! Lending protocols read prices through proxies and adapters; the contract that actually receives
//! the price-update transaction is the underlying Chainlink aggregator. `resolve_aggregators`
//! follows sources down to those aggregators so a sequencer-feed transaction can be matched to the
//! assets whose price it moves.

use std::collections::{HashMap, HashSet};

use alloy::{
    primitives::{Address, U256},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::SolCall,
};
use anyhow::Result;
use mev_bindings::IPriceSource;
use mev_rpc::{call, decode, multicall};
use tracing::debug;

/// How deep to follow adapter -> proxy -> aggregator chains.
const MAX_DEPTH: usize = 4;
/// Getter calls per contract per resolution round.
const GETTERS: usize = 8;

mod version {
    alloy::sol! {
        function typeAndVersion() external view returns (string);
    }
}

/// A Chainlink SVR aggregator: a public report it receives is held back (the bonus of the price
/// move is auctioned first), so a transaction sent straight behind it still sees the old price.
pub fn is_held_type(type_and_version: &str) -> bool {
    type_and_version.starts_with("DualAggregator")
}

/// The `aggregators` that hold public updates back (`typeAndVersion()` is a `DualAggregator`).
/// Measured 2026-10-15: SVR auctions took 99% of Arbitrum Aave's liquidation bonus and plain
/// backruns of the public update reverted. Contracts that do not answer are not held.
pub async fn held_aggregators(
    p: &DynProvider,
    aggregators: &[Address],
) -> Result<HashSet<Address>> {
    let calls: Vec<_> = aggregators
        .iter()
        .map(|a| call(*a, version::typeAndVersionCall {}))
        .collect();
    let res = multicall(p, &calls, 200).await?;
    Ok(aggregators
        .iter()
        .zip(&res)
        .filter(|(_, r)| {
            decode::<version::typeAndVersionCall>(r).is_some_and(|tv| is_held_type(&tv))
        })
        .map(|(a, _)| *a)
        .collect())
}

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
            calls.push(call(*c, IPriceSource::RATIO_PROVIDERCall {}));
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
                // Price-cap adapters (wstETH, weETH, ...): the exchange-rate feed also moves the price.
                nz(decode::<IPriceSource::RATIO_PROVIDERCall>(&r[7])),
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
                // Unknown shape (SVR / CAPO wrappers, custom oracles): follow the contracts that
                // `latestRoundData` reads. A leaf (reads only itself) is the aggregator.
                let inner = touched(p, *c).await;
                if inner.is_empty() {
                    debug!(source = %c, "price source reads no other contract; watching it directly");
                    out.entry(*c).or_default().push(*origin);
                } else {
                    next.extend(inner.into_iter().map(|x| (x, *origin)));
                }
            }
        }
        frontier = next;
    }
    // Chains deeper than MAX_DEPTH: watch where resolution stopped.
    for (c, origin) in frontier {
        out.entry(c).or_default().push(origin);
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(out)
}

/// Latest answers of Chainlink aggregators, by direct `eth_call` per aggregator (concurrent).
/// Not via Multicall3: access-controlled aggregators only serve callers where
/// `msg.sender == tx.origin`. Unreadable aggregators are left out.
pub async fn latest_answers(p: &DynProvider, aggregators: &[Address]) -> HashMap<Address, f64> {
    use futures::StreamExt;
    futures::stream::iter(aggregators.iter().copied())
        .map(|a| async move {
            let tx = TransactionRequest::default()
                .to(a)
                .input(IPriceSource::latestAnswerCall {}.abi_encode().into());
            let raw = p.call(tx).await.ok()?;
            let v = IPriceSource::latestAnswerCall::abi_decode_returns(&raw).ok()?;
            Some((a, v.to_string().parse::<f64>().ok()?))
        })
        .buffer_unordered(8)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

/// `latest_answers` as exact integers; aggregators with a negative or unreadable answer are left
/// out.
pub async fn latest_answers_exact(
    p: &DynProvider,
    aggregators: &[Address],
) -> HashMap<Address, U256> {
    use futures::StreamExt;
    futures::stream::iter(aggregators.iter().copied())
        .map(|a| async move {
            let tx = TransactionRequest::default()
                .to(a)
                .input(IPriceSource::latestAnswerCall {}.abi_encode().into());
            let raw = p.call(tx).await.ok()?;
            let v = IPriceSource::latestAnswerCall::abi_decode_returns(&raw).ok()?;
            (!v.is_negative()).then(|| (a, U256::from(v.unsigned_abs())))
        })
        .buffer_unordered(8)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

/// Contracts other than `c` that `c.latestRoundData()` reads (`eth_createAccessList`). Empty if
/// none or if the endpoint doesn't serve access lists.
async fn touched(p: &DynProvider, c: Address) -> Vec<Address> {
    // Explicit gas limit: some endpoints otherwise charge the call the block gas cap and reject it
    // for insufficient funds (and a zero gas price is rejected post-London).
    let tx = TransactionRequest::default()
        .to(c)
        .gas_limit(2_000_000)
        .input(IPriceSource::latestRoundDataCall {}.abi_encode().into());
    match p.create_access_list(&tx).await {
        Ok(r) => r
            .access_list
            .0
            .into_iter()
            .map(|i| i.address)
            .filter(|a| *a != c)
            .collect(),
        Err(e) => {
            debug!(source = %c, error = %e, "access list unavailable");
            vec![]
        }
    }
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

mod report {
    alloy::sol! {
        function forward(address to, bytes data);
        /// OCR2 (incl. `DualAggregator`): report = (uint32 ts, bytes32 observers, int192[] obs, int192 juels).
        function transmit(bytes32[3] reportContext, bytes report, bytes32[] rs, bytes32[] ss, bytes32 rawVs);
        /// OCR1 `OffchainAggregator`: report = (bytes32 context, bytes32 observers, int192[] obs).
        function transmit(bytes report, bytes32[] rs, bytes32[] ss, bytes32 rawVs);
    }
}

/// The new answer a Chainlink report transaction will write, read from its calldata before it is
/// in a block: `(aggregator, answer)`. The answer is the median observation, exactly what the
/// aggregator stores (verified against `NewTransmission` on Arbitrum ETH/USD). Handles direct
/// `transmit` calls and `forward(aggregator, transmit(...))` via an operator's forwarder.
pub fn report_answer(to: Option<Address>, input: &[u8]) -> Option<(Address, f64)> {
    report_answer_raw(to, input).and_then(|(a, v)| Some((a, v.to_string().parse().ok()?)))
}

/// `report_answer` as the exact integer the aggregator will store (the median observation).
/// `None` for a negative median: Morpho's and Aave's oracles reject negative answers.
pub fn report_answer_exact(to: Option<Address>, input: &[u8]) -> Option<(Address, U256)> {
    let (a, v) = report_answer_raw(to, input)?;
    (!v.is_negative()).then(|| (a, U256::from(v.unsigned_abs())))
}

fn report_answer_raw(
    to: Option<Address>,
    input: &[u8],
) -> Option<(Address, alloy::primitives::aliases::I192)> {
    use alloy::{
        primitives::{B256, aliases::I192},
        sol_types::{SolCall, SolValue},
    };
    let (aggregator, inner) = match report::forwardCall::abi_decode(input) {
        Ok(f) => (f.to, f.data.to_vec()),
        Err(_) => (to?, input.to_vec()),
    };
    let observations: Vec<I192> = if let Ok(t) = report::transmit_0Call::abi_decode(&inner) {
        <(u32, B256, Vec<I192>, I192)>::abi_decode_params(&t.report)
            .ok()?
            .2
    } else if let Ok(t) = report::transmit_1Call::abi_decode(&inner) {
        <(B256, B256, Vec<I192>)>::abi_decode_params(&t.report)
            .ok()?
            .2
    } else {
        return None;
    };
    // Observations arrive sorted; the aggregator takes the middle one.
    let median = observations.get(observations.len() / 2)?;
    Some((aggregator, *median))
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

    /// Tags watching exactly `contract` (no forwarder unwrapping).
    pub fn slots_for(&self, contract: Address) -> &[T] {
        self.matches(Some(contract))
    }

    /// Every watched contract.
    pub fn contracts(&self) -> Vec<Address> {
        self.by_contract.keys().copied().collect()
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

    #[test]
    fn only_dual_aggregators_hold_updates() {
        assert!(is_held_type("DualAggregator 1.0.0"));
        assert!(!is_held_type("AccessControlledOCR2Aggregator 1.0.0"));
        assert!(!is_held_type("OffchainAggregator 4.0.0"));
        assert!(!is_held_type(""));
    }

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
    fn report_answer_matches_the_on_chain_answer() {
        // Real Arbitrum ETH/USD update (block 511618219, tx 0x1bfa…2707) sent via a forwarder.
        let hex = include_str!("../tests/fixtures/eth_usd_forward_transmit.hex");
        let input = alloy::hex::decode(hex.trim()).unwrap();
        let forwarder = Address::with_last_byte(0xf0);
        let (agg, answer) = report_answer(Some(forwarder), &input).unwrap();
        assert_eq!(
            agg,
            alloy::primitives::address!("a5E1a36938769cbd5a26f5e19D8FCB379f597c83")
        );
        // NewTransmission.answer emitted by that tx.
        assert_eq!(answer, 269_591_416_460.0);
        // The same transmit sent directly to the aggregator decodes too.
        let inner = {
            use alloy::sol_types::SolCall;
            report::forwardCall::abi_decode(&input).unwrap().data
        };
        assert_eq!(report_answer(Some(agg), &inner), Some((agg, answer)));
        assert_eq!(report_answer(Some(agg), &[0xde, 0xad, 0xbe, 0xef]), None);
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
