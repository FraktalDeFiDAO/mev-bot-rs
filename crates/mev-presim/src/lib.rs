//! Pre-block simulation of sequencer-feed transactions.
//!
//! The sequencer feed shows a transaction ~a block before its logs exist. Running it through
//! `eth_simulateV1` (as its real sender, against the latest state) returns the logs it will emit;
//! pool `Swap`/`Sync` events carry absolute post-trade prices/reserves, so they can update the pool
//! store early. If the real block differs, its own logs overwrite the state a moment later.
//!
//! Measured from the bot's server (2026-10-04): PublicNode answers in ~16 ms on a warm
//! connection (dRPC ~70 ms, arb1 ~120 ms), with the same head block as arb1.

use alloy::{
    consensus::{Transaction, TxEnvelope, transaction::SignerRecoverable},
    primitives::{Address, B256, Bytes},
    providers::{DynProvider, Provider},
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::{IPancakeV3Pool, IUniswapV2Pair, IUniswapV3Pool};
use mev_sequencer::SequencedTx;
use serde::Deserialize;

/// Selectors of calls that can't move a pool: ERC-20 transfer/approve/transferFrom, Chainlink
/// forwarder reports, OCR transmits.
const INERT_SELECTORS: [[u8; 4]; 6] = [
    [0xa9, 0x05, 0x9c, 0xbb], // transfer
    [0x09, 0x5e, 0xa7, 0xb3], // approve
    [0x23, 0xb8, 0x72, 0xdd], // transferFrom
    [0x6f, 0xad, 0xcf, 0x72], // forward (Chainlink)
    [0xb1, 0xdc, 0x65, 0xa4], // transmit (OCR2)
    [0xc9, 0x80, 0x75, 0x39], // transmit (OCR1)
];

/// Worth simulating: a contract call that isn't obviously inert. Cheap (no I/O).
pub fn is_candidate(st: &SequencedTx) -> bool {
    st.to.is_some() && st.input.len() >= 4 && !INERT_SELECTORS.iter().any(|s| st.input[..4] == *s)
}

/// 20-byte addresses (watched pools and their tokens) a feed tx must mention to be simulated.
pub type Watchlist = std::sync::Arc<std::sync::RwLock<std::collections::HashSet<[u8; 20]>>>;

/// Whether `input` contains any address in `watch` — as a 32-byte ABI word or packed (Uniswap V3
/// paths), a raw 20-byte match finds both. An empty watchlist matches everything.
pub fn mentions_any(input: &[u8], watch: &std::collections::HashSet<[u8; 20]>) -> bool {
    if watch.is_empty() {
        return true;
    }
    input
        .windows(20)
        .any(|w| watch.contains(<&[u8; 20]>::try_from(w).unwrap()))
}

/// Topic0 of the pool events the engine consumes (V2 `Sync`, V3/Algebra `Swap`, PancakeV3 `Swap`).
pub fn is_pool_event(topic0: &B256) -> bool {
    *topic0 == IUniswapV2Pair::Sync::SIGNATURE_HASH
        || *topic0 == IUniswapV3Pool::Swap::SIGNATURE_HASH
        || *topic0 == IPancakeV3Pool::Swap::SIGNATURE_HASH
}

/// A log the simulated transaction would emit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SimLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
}

#[derive(Debug, Deserialize)]
struct SimBlock {
    calls: Vec<SimCall>,
}

#[derive(Debug, Deserialize)]
struct SimCall {
    status: alloy::primitives::U64,
    #[serde(default)]
    logs: Vec<SimLog>,
}

/// Run `tx` as its sender on top of the latest block; the logs it emits (empty if it reverts).
/// `tx` as an `eth_simulateV1` call by its sender.
pub fn call_json(tx: &TxEnvelope) -> Result<serde_json::Value> {
    let from = tx.recover_signer().context("recover sender")?;
    Ok(serde_json::json!({
        "from": from,
        "to": tx.to(),
        "input": tx.input(),
        "value": tx.value(),
        "gas": format!("{:#x}", tx.gas_limit()),
    }))
}

/// Result of the last call of a simulated block.
#[derive(Debug, Clone)]
pub struct AfterCall {
    pub ok: bool,
    pub return_data: Bytes,
    /// Revert reason / error message when `!ok`.
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct AfterSimCall {
    status: alloy::primitives::U64,
    #[serde(default, rename = "returnData")]
    return_data: Bytes,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AfterSimBlock {
    calls: Vec<AfterSimCall>,
}

/// Run `trigger` and then `then` in one simulated block on top of the latest, and report `then`'s
/// result: a trade that only exists once a pending transaction has landed can be verified exactly
/// (a plain `eth_call` at the latest block does not contain that transaction). `then` fails to
/// verify (`ok = false`) if the trigger itself reverts, since its effect would then not happen.
pub async fn simulate_after(
    p: &DynProvider,
    trigger: &serde_json::Value,
    then: serde_json::Value,
) -> Result<AfterCall> {
    let params = serde_json::json!([
        { "blockStateCalls": [{ "calls": [trigger, then] }], "validation": false },
        "latest"
    ]);
    let blocks: Vec<AfterSimBlock> = p
        .raw_request("eth_simulateV1".into(), params)
        .await
        .context("eth_simulateV1")?;
    let calls = blocks
        .into_iter()
        .next()
        .map(|b| b.calls)
        .unwrap_or_default();
    let [first, second] = calls.as_slice() else {
        anyhow::bail!("expected two call results, got {}", calls.len());
    };
    if first.status.to::<u64>() != 1 {
        return Ok(AfterCall {
            ok: false,
            return_data: Bytes::new(),
            error: "trigger transaction reverts".into(),
        });
    }
    Ok(AfterCall {
        ok: second.status.to::<u64>() == 1,
        return_data: second.return_data.clone(),
        error: second
            .error
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    })
}

pub async fn simulate_logs(p: &DynProvider, tx: &TxEnvelope) -> Result<Vec<SimLog>> {
    simulate_logs_at(p, tx, None).await
}

/// `simulate_logs` on top of block `at` (`None` = latest). Pinning the block lets a local
/// simulator be compared against the same state it ran on.
pub async fn simulate_logs_at(
    p: &DynProvider,
    tx: &TxEnvelope,
    at: Option<u64>,
) -> Result<Vec<SimLog>> {
    let call = call_json(tx)?;
    // validation=false: no fee/nonce checks, so a tx priced for the next block still runs.
    let params = serde_json::json!([
        { "blockStateCalls": [{ "calls": [call] }], "validation": false },
        at.map_or_else(|| "latest".to_string(), |n| format!("{n:#x}"))
    ]);
    let blocks: Vec<SimBlock> = p
        .raw_request("eth_simulateV1".into(), params)
        .await
        .context("eth_simulateV1")?;
    Ok(parse_result(blocks))
}

fn parse_result(blocks: Vec<SimBlock>) -> Vec<SimLog> {
    blocks
        .into_iter()
        .flat_map(|b| b.calls)
        .filter(|c| c.status.to::<u64>() == 1)
        .flat_map(|c| c.logs)
        .collect()
}

/// Pool events among `logs`.
pub fn pool_events(logs: &[SimLog]) -> impl Iterator<Item = &SimLog> {
    logs.iter()
        .filter(|l| l.topics.first().is_some_and(is_pool_event))
}

/// `SimLog` as the RPC log type the pool store consumes.
pub fn to_rpc_log(l: &SimLog) -> alloy::rpc::types::Log {
    alloy::rpc::types::Log {
        inner: alloy::primitives::Log::new_unchecked(l.address, l.topics.clone(), l.data.clone()),
        ..Default::default()
    }
}

/// What happened to feed transactions (per summary interval): why pool updates were or weren't
/// learned early.
#[derive(Debug, Default)]
pub struct PresimStats {
    pub feed: std::sync::atomic::AtomicU64,
    pub candidates: std::sync::atomic::AtomicU64,
    /// Contract calls that mention no watched pool or token (not simulated).
    pub unrelated: std::sync::atomic::AtomicU64,
    pub skipped_busy: std::sync::atomic::AtomicU64,
    pub moved_pools: std::sync::atomic::AtomicU64,
    pub no_pool_effect: std::sync::atomic::AtomicU64,
    pub timeouts: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    /// Sum of simulate round-trip ms (for the mean).
    pub sim_ms_total: std::sync::atomic::AtomicU64,
}

impl PresimStats {
    /// One-line summary since the last call; resets the counters.
    pub fn take_summary(&self) -> String {
        use std::sync::atomic::Ordering::Relaxed;
        let t = |a: &std::sync::atomic::AtomicU64| a.swap(0, Relaxed);
        let (feed, cand, busy) = (t(&self.feed), t(&self.candidates), t(&self.skipped_busy));
        let unrelated = t(&self.unrelated);
        let (moved, none, to, err) = (
            t(&self.moved_pools),
            t(&self.no_pool_effect),
            t(&self.timeouts),
            t(&self.errors),
        );
        let sims = moved + none;
        let mean = t(&self.sim_ms_total).checked_div(sims).unwrap_or(0);
        format!(
            "feed={feed} unrelated={unrelated} candidates={cand} skipped_busy={busy} moved_pools={moved} no_pool_effect={none} timeouts={to} errors={err} mean_sim_ms={mean}"
        )
    }
}

/// Pool logs a feed transaction is about to emit, and when the feed delivered it.
#[derive(Debug, Clone)]
pub struct Presimulated {
    pub tx: B256,
    pub received: std::time::Instant,
    pub logs: Vec<alloy::rpc::types::Log>,
    /// The pending transaction as an `eth_simulateV1` call (`from`, `to`, `input`, `value`, `gas`), so
    /// a trade it triggers can be verified on top of it. `Null` when it could not be built.
    pub call: serde_json::Value,
}

/// Simulate candidate feed transactions (at most `max_in_flight` at once; extras are skipped,
/// their block logs still arrive later) and forward the pool logs of those that move a pool.
/// Runs until the feed or the receiver closes.
pub async fn run(
    mut feed: tokio::sync::mpsc::UnboundedReceiver<SequencedTx>,
    p: DynProvider,
    max_in_flight: usize,
    timeout: std::time::Duration,
    out: tokio::sync::mpsc::UnboundedSender<Presimulated>,
    stats: std::sync::Arc<PresimStats>,
    watch: Watchlist,
) {
    use std::sync::atomic::Ordering::Relaxed;
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(max_in_flight.max(1)));
    while let Some(st) = feed.recv().await {
        if out.is_closed() {
            return;
        }
        stats.feed.fetch_add(1, Relaxed);
        if !is_candidate(&st) {
            continue;
        }
        // Can't move a watched pool unless it names one of its tokens (or the pool itself).
        if !mentions_any(&st.input, &watch.read().unwrap()) {
            stats.unrelated.fetch_add(1, Relaxed);
            continue;
        }
        stats.candidates.fetch_add(1, Relaxed);
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            stats.skipped_busy.fetch_add(1, Relaxed);
            continue;
        };
        let (p, out, stats) = (p.clone(), out.clone(), stats.clone());
        tokio::spawn(async move {
            let t = std::time::Instant::now();
            let res = tokio::time::timeout(timeout, simulate_logs(&p, &st.tx)).await;
            drop(permit);
            let logs = match res {
                Err(_) => {
                    stats.timeouts.fetch_add(1, Relaxed);
                    return;
                }
                Ok(Err(_)) => {
                    stats.errors.fetch_add(1, Relaxed);
                    return;
                }
                Ok(Ok(logs)) => logs,
            };
            stats
                .sim_ms_total
                .fetch_add(t.elapsed().as_millis() as u64, Relaxed);
            let logs: Vec<_> = pool_events(&logs).map(to_rpc_log).collect();
            if logs.is_empty() {
                stats.no_pool_effect.fetch_add(1, Relaxed);
            } else {
                stats.moved_pools.fetch_add(1, Relaxed);
                let _ = out.send(Presimulated {
                    tx: st.hash,
                    received: st.received,
                    logs,
                    call: call_json(&st.tx).unwrap_or_default(),
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        consensus::{SignableTransaction, TxEip1559},
        network::TxSignerSync,
        primitives::TxKind,
        signers::local::PrivateKeySigner,
    };
    use std::time::Instant;

    fn seq(to: Option<Address>, input: Vec<u8>) -> SequencedTx {
        let signer = PrivateKeySigner::random();
        let mut t = TxEip1559 {
            chain_id: 42161,
            to: to.map(TxKind::Call).unwrap_or(TxKind::Create),
            input: input.clone().into(),
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut t).unwrap();
        let env = TxEnvelope::Eip1559(t.into_signed(sig));
        SequencedTx {
            seq: 1,
            received: Instant::now(),
            hash: *env.tx_hash(),
            to,
            input: input.into(),
            tx: env,
        }
    }

    #[test]
    fn candidates_are_non_inert_contract_calls() {
        let c = Some(Address::with_last_byte(1));
        assert!(is_candidate(&seq(c, vec![0x38, 0xed, 0x17, 0x39, 0]))); // a router swap
        assert!(!is_candidate(&seq(c, vec![0xa9, 0x05, 0x9c, 0xbb, 0]))); // ERC-20 transfer
        assert!(!is_candidate(&seq(c, vec![0x6f, 0xad, 0xcf, 0x72]))); // oracle report
        assert!(!is_candidate(&seq(c, vec![]))); // ETH transfer
        assert!(!is_candidate(&seq(None, vec![1, 2, 3, 4]))); // deployment
    }

    #[test]
    fn parses_simulate_result_and_drops_reverted_calls() {
        let sync = IUniswapV2Pair::Sync::SIGNATURE_HASH;
        let json = format!(
            r#"[{{"calls":[
                {{"status":"0x1","logs":[{{"address":"0x0000000000000000000000000000000000000001","topics":["{sync:?}"],"data":"0x01"}},
                                         {{"address":"0x0000000000000000000000000000000000000002","topics":["0x{}"],"data":"0x"}}]}},
                {{"status":"0x0","logs":[{{"address":"0x0000000000000000000000000000000000000003","topics":["{sync:?}"],"data":"0x"}}]}}
            ]}}]"#,
            "11".repeat(32)
        );
        let logs = parse_result(serde_json::from_str(&json).unwrap());
        assert_eq!(logs.len(), 2, "the reverted call's logs are dropped");
        let pools: Vec<Address> = pool_events(&logs).map(|l| l.address).collect();
        assert_eq!(pools, vec![Address::with_last_byte(1)]);
    }

    #[test]
    fn rpc_log_keeps_address_topics_and_data() {
        let l = SimLog {
            address: Address::with_last_byte(7),
            topics: vec![IUniswapV2Pair::Sync::SIGNATURE_HASH],
            data: vec![1, 2, 3].into(),
        };
        let r = to_rpc_log(&l);
        assert_eq!(r.address(), l.address);
        assert_eq!(r.topics(), l.topics.as_slice());
        assert_eq!(r.data().data, l.data);
    }

    #[test]
    fn watchlist_matches_abi_words_and_packed_paths() {
        let weth = [0x82u8; 20];
        let watch: std::collections::HashSet<[u8; 20]> = [weth].into_iter().collect();
        // ABI word: 12 zero bytes + address.
        let mut abi = vec![0x38, 0xed, 0x17, 0x39];
        abi.extend([0u8; 12]);
        abi.extend(weth);
        assert!(mentions_any(&abi, &watch));
        // Packed V3 path: token(20) | fee(3) | token(20).
        let mut path = vec![0x11u8; 20];
        path.extend([0, 1, 244]);
        path.extend(weth);
        assert!(mentions_any(&path, &watch));
        assert!(!mentions_any(&[0x22u8; 64], &watch));
        assert!(
            mentions_any(&[1, 2, 3], &Default::default()),
            "empty watchlist = no filter"
        );
    }
}
