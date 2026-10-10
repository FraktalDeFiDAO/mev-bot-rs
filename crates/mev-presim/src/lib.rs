//! Pre-block simulation of sequencer-feed transactions.
//!
//! The sequencer feed shows a transaction ~a block before its logs exist. Running it through
//! `eth_simulateV1` (as its real sender, against the latest state) returns the logs it will emit;
//! pool `Swap`/`Sync` events carry absolute post-trade prices/reserves, so they can update the pool
//! store early. If the real block differs, its own logs overwrite the state a moment later.
//!
//! Measured from the bot's server (2026-10-04): PublicNode answers in ~16 ms on a warm
//! connection (dRPC ~70 ms, arb1 ~120 ms), with the same head block as arb1.

/// The calldata decoder lives in its own crate (`mev-decode`); re-exported here for the callers that used this path.
pub use mev_decode::swap as decode;
pub mod record;

use alloy::{
    consensus::{Transaction, TxEnvelope, transaction::SignerRecoverable},
    primitives::{Address, B256, Bytes},
    providers::{DynProvider, Provider},
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_bindings::{IPancakeV3Pool, IPoolManager, IUniswapV2Pair, IUniswapV3Pool};
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

/// Whether a feed transaction should be simulated: its calldata names a watched pool or token, or the decoder reads a
/// swap, liquidity change or liquidation from it (packed calldata names nothing, yet one such contract alone was 23% of swap transactions on
/// 2026-10-08, invisible to the address filter).
pub fn wants_simulation(
    to: Option<alloy::primitives::Address>,
    input: &[u8],
    watch: &std::collections::HashSet<[u8; 20]>,
) -> bool {
    mentions_any(input, watch)
        || to.is_some_and(|t| !mev_decode::calls::decode_all(t, input).is_empty())
}

/// Topic0 of the pool events the engine consumes (V2 `Sync`, V3/Algebra `Swap`, PancakeV3 `Swap`).
pub fn is_pool_event(topic0: &B256) -> bool {
    *topic0 == IUniswapV2Pair::Sync::SIGNATURE_HASH
        || *topic0 == IUniswapV3Pool::Swap::SIGNATURE_HASH
        || *topic0 == IPancakeV3Pool::Swap::SIGNATURE_HASH
        // Uniswap V4 swaps (from the PoolManager; the pool is named by id in topic 1). Absolute price, liquidity and
        // tick, like the V3 swap, so applying one early and the real log later cannot double count. Mint / Burn /
        // ModifyLiquidity are deltas and are deliberately not forwarded.
        || *topic0 == IPoolManager::Swap::SIGNATURE_HASH
}

/// The pool a forwarded event belongs to, as the pool store names it: the emitting contract, or for a V4 swap the
/// stand-in address derived from the pool id (last 20 bytes of topic 1).
pub fn event_pool(l: &alloy::rpc::types::Log) -> Address {
    if l.topics().first() == Some(&IPoolManager::Swap::SIGNATURE_HASH)
        && let Some(id) = l.topics().get(1)
    {
        return Address::from_slice(&id[12..]);
    }
    l.address()
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
    /// The simulated block's number: one above the block the simulation ran on top of.
    #[serde(default)]
    number: Option<alloy::primitives::U64>,
    calls: Vec<SimCall>,
}

#[derive(Debug, Deserialize)]
struct SimCall {
    status: alloy::primitives::U64,
    #[serde(default)]
    logs: Vec<SimLog>,
}

/// Does a simulation include `tx_block`'s transaction already? A simulation runs as the *next* block on top of
/// some block `n`, so it reports block `n + 1`. If that is **past** the transaction's own block, the state it ran
/// on already contains the transaction and running it again would apply it twice: the pool ends one swap ahead of
/// the chain, a price that never existed (measured 2026-10-08: a landed swap re-simulated on its own block moved the
/// pool 2.89 bps beyond the chain). Equal means the state is the one just before the transaction's block; earlier
/// means a lagging node (slightly stale, but the transaction is not in it).
pub fn already_included(sim_block: Option<u64>, tx_block: u64) -> bool {
    sim_block.is_some_and(|b| b > tx_block)
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
    /// The simulated block's number (see `already_included`).
    pub block: Option<u64>,
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
    #[serde(default)]
    number: Option<alloy::primitives::U64>,
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
    let first_block = blocks.into_iter().next();
    let block = first_block
        .as_ref()
        .and_then(|b| b.number)
        .map(|n| n.to::<u64>());
    let calls = first_block.map(|b| b.calls).unwrap_or_default();
    let [first, second] = calls.as_slice() else {
        anyhow::bail!("expected two call results, got {}", calls.len());
    };
    if first.status.to::<u64>() != 1 {
        return Ok(AfterCall {
            block,
            ok: false,
            return_data: Bytes::new(),
            error: "trigger transaction reverts".into(),
        });
    }
    Ok(AfterCall {
        block,
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
    Ok(simulate_logs_block(p, tx, at).await?.0)
}

/// `simulate_logs_at`, also returning the simulated block's number (see `already_included`).
pub async fn simulate_logs_block(
    p: &DynProvider,
    tx: &TxEnvelope,
    at: Option<u64>,
) -> Result<(Vec<SimLog>, Option<u64>)> {
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
    let number = blocks.first().and_then(|b| b.number).map(|n| n.to::<u64>());
    Ok((parse_result(blocks), number))
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

/// (destination, selector) -> (moved a pool, moved none).
type DestTally = std::collections::HashMap<(alloy::primitives::Address, [u8; 4]), (u64, u64)>;

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
    /// Simulated on a state that already contained the transaction: result discarded (it would double-apply).
    pub already_included: std::sync::atomic::AtomicU64,
    /// Sum of simulate round-trip ms (for the mean).
    pub sim_ms_total: std::sync::atomic::AtomicU64,
    /// Per destination contract and 4-byte selector: (simulated and moved a pool, simulated and moved none). Tells
    /// which routers a local decoder would have to cover first.
    dests: std::sync::Mutex<DestTally>,
}

impl PresimStats {
    fn tally(&self, tx: &TxEnvelope, moved: bool) {
        let (Some(to), input) = (tx.to(), tx.input()) else {
            return;
        };
        let mut sel = [0u8; 4];
        let n = input.len().min(4);
        sel[..n].copy_from_slice(&input[..n]);
        let mut d = self.dests.lock().unwrap();
        if d.len() > 4_096 {
            d.clear();
        }
        let e = d.entry((to, sel)).or_default();
        if moved {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }

    /// The `n` busiest (destination, selector) pairs since the last call, as `to selector moved/none`; resets.
    pub fn take_destinations(&self, n: usize) -> String {
        let mut v: Vec<_> = std::mem::take(&mut *self.dests.lock().unwrap())
            .into_iter()
            .collect();
        v.sort_by_key(|(_, (m, z))| std::cmp::Reverse(m + z));
        v.iter()
            .take(n)
            .map(|((to, sel), (m, z))| format!("{to} 0x{} {m}/{z}", alloy::hex::encode(sel)))
            .collect::<Vec<_>>()
            .join(" | ")
    }

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
        let stale = t(&self.already_included);
        let sims = moved + none;
        let mean = t(&self.sim_ms_total).checked_div(sims).unwrap_or(0);
        format!(
            "feed={feed} unrelated={unrelated} candidates={cand} skipped_busy={busy} moved_pools={moved} no_pool_effect={none} already_included={stale} timeouts={to} errors={err} mean_sim_ms={mean}"
        )
    }
}

/// One call inside a transaction (the transaction itself, or a call within a multicall / Multicall3 / smart account /
/// ERC-4337 bundle) and what the decoder made of it. A call to a contract we have identified that nothing reads is
/// marked `undecoded_known`: those are the gaps in "every action on the watched exchanges is decoded".
pub fn action_json(a: &mev_decode::calls::Action) -> serde_json::Value {
    use mev_decode::Decoded;
    let known = mev_decode::known_contract(a.leaf.to);
    let (kind, detail) = match &a.decoded {
        Some(Decoded::Swap(s)) => (
            Some("swap"),
            serde_json::json!({
                "pool": match &s.pool {
                    decode::PoolRef::Address(p) => format!("{p:#x}"),
                    decode::PoolRef::Pair { token_a, token_b, fee } => format!("{token_a:#x}/{token_b:#x}/{fee}"),
                    decode::PoolRef::PairV2 { token_a, token_b } => format!("v2:{token_a:#x}/{token_b:#x}"),
                    decode::PoolRef::Tokens { token_a, token_b } => format!("any:{token_a:#x}/{token_b:#x}"),
                },
                "token_in": format!("{:#x}", s.token_in),
                "amount_in": s.amount_in.to_string(),
                "limit_x96": s.sqrt_limit_x96.map(|l| l.to_string()),
                "hops": s.hops,
            }),
        ),
        Some(Decoded::Liquidity(l)) => (
            Some("liquidity"),
            serde_json::json!({
                "protocol": l.protocol,
                "action": format!("{:?}", l.action).to_lowercase(),
                "token_id": l.token_id.map(|t| t.to_string()),
                "liquidity": l.liquidity.map(|t| t.to_string()),
                "pool": l.pool.as_ref().map(|p| format!("{p:?}")),
            }),
        ),
        Some(Decoded::Liquidation(l)) => (
            Some("liquidation"),
            serde_json::json!({
                "protocol": l.protocol,
                "borrowers": l.borrowers.iter().map(|b| format!("{b:#x}")).collect::<Vec<_>>(),
            }),
        ),
        None => (None, serde_json::Value::Null),
    };
    serde_json::json!({
        "to": format!("{:#x}", a.leaf.to),
        "sel": a.leaf.selector().map(alloy::hex::encode),
        "via": a.leaf.via,
        "known": known.map(|c| c.name),
        "kind": kind,
        "detail": detail,
        "undecoded_known": kind.is_none() && known.is_some(),
    })
}

/// One analysis record for a feed transaction: what it is, what it pays, what we decoded, and what the simulation said.
fn tx_line(
    st: &SequencedTx,
    outcome: &str,
    sim_ms: Option<u64>,
    moved: &[Address],
) -> serde_json::Value {
    use alloy::consensus::Transaction as _;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let intent = st.to.and_then(|to| {
        mev_decode::decode_call_with_value(
            to,
            &st.input,
            alloy::consensus::Transaction::value(&st.tx),
        )
    });
    let actions = st
        .to
        .map(|to| mev_decode::calls::actions_in(to, &st.input))
        .unwrap_or_default();
    let swap = st.to.and_then(|to| decode::decode(to, &st.input)).map(|s| {
        serde_json::json!({
            "pool": match &s.pool {
                decode::PoolRef::Address(a) => format!("{a:#x}"),
                decode::PoolRef::Pair { token_a, token_b, fee } => format!("{token_a:#x}/{token_b:#x}/{fee}"),
                decode::PoolRef::PairV2 { token_a, token_b } => format!("v2:{token_a:#x}/{token_b:#x}"),
                    decode::PoolRef::Tokens { token_a, token_b } => format!("any:{token_a:#x}/{token_b:#x}"),
            },
            "token_in": format!("{:#x}", s.token_in),
            "amount_in": s.amount_in.to_string(),
            "limit_x96": s.sqrt_limit_x96.map(|l| l.to_string()),
            "hops": s.hops,
        })
    });
    serde_json::json!({
        "t_ms": now.saturating_sub(st.received.elapsed().as_millis() as u64),
        "hash": format!("{:#x}", st.hash),
        "block": st.block(),
        "seq": st.seq,
        "from": st.tx.recover_signer().ok().map(|a| format!("{a:#x}")),
        "to": st.to.map(|a| format!("{a:#x}")),
        "sel": st.selector().map(alloy::hex::encode),
        "len": st.input.len(),
        "nonce": st.tx.nonce(),
        "gas_limit": st.tx.gas_limit(),
        "max_fee": st.tx.max_fee_per_gas().to_string(),
        "tip": st.tx.max_priority_fee_per_gas().map(|t| t.to_string()),
        "value": st.tx.value().to_string(),
        "swap": swap,
        "liquidity": intent.as_ref().and_then(|d| match d {
            mev_decode::Decoded::Liquidity(l) => Some(serde_json::json!({
                "protocol": l.protocol,
                "action": format!("{:?}", l.action).to_lowercase(),
                "token_id": l.token_id.map(|t| t.to_string()),
                "liquidity": l.liquidity.map(|t| t.to_string()),
                "pool": l.pool.as_ref().map(|p| format!("{p:?}")),
            })),
            _ => None,
        }),
        "liquidation": intent.as_ref().and_then(|d| match d {
            mev_decode::Decoded::Liquidation(l) => Some(serde_json::json!({
                "protocol": l.protocol,
                "borrowers": l.borrowers.iter().map(|b| format!("{b:#x}")).collect::<Vec<_>>(),
            })),
            _ => None,
        }),
        "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
        "undecoded_known": actions.iter().filter(|a| a.decoded.is_none() && mev_decode::known_contract(a.leaf.to).is_some()).count(),
        "presim": outcome,
        "sim_ms": sim_ms,
        "pools_moved": moved.len(),
        "moved": moved.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(),
    })
}

/// Pool logs a feed transaction is about to emit, and when the feed delivered it.
#[derive(Debug, Clone)]
pub struct Presimulated {
    pub tx: B256,
    /// The L2 block the transaction was sequenced into.
    pub block: u64,
    pub received: std::time::Instant,
    pub logs: Vec<alloy::rpc::types::Log>,
    /// The pending transaction as an `eth_simulateV1` call (`from`, `to`, `input`, `value`, `gas`), so
    /// a trade it triggers can be verified on top of it. `Null` when it could not be built.
    pub call: serde_json::Value,
}

/// Simulate candidate feed transactions (at most `max_in_flight` at once; extras are skipped,
/// their block logs still arrive later) and forward the pool logs of those that move a pool.
/// Runs until the feed or the receiver closes.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    mut feed: tokio::sync::mpsc::UnboundedReceiver<SequencedTx>,
    p: DynProvider,
    max_in_flight: usize,
    timeout: std::time::Duration,
    out: tokio::sync::mpsc::UnboundedSender<Presimulated>,
    stats: std::sync::Arc<PresimStats>,
    watch: Watchlist,
    rec: Option<std::sync::Arc<record::TxRecorder>>,
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
        // Can't move a watched pool unless it names one of its tokens (or the pool itself), except when the decoder
        // reads a swap from the call: packed calldata (an index into the contract's own pool table) names nothing.
        if !wants_simulation(st.to, &st.input, &watch.read().unwrap()) {
            stats.unrelated.fetch_add(1, Relaxed);
            continue;
        }
        stats.candidates.fetch_add(1, Relaxed);
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            stats.skipped_busy.fetch_add(1, Relaxed);
            if let Some(r) = &rec {
                r.record(&tx_line(&st, "busy", None, &[]));
            }
            continue;
        };
        let (p, out, stats, rec) = (p.clone(), out.clone(), stats.clone(), rec.clone());
        tokio::spawn(async move {
            let t = std::time::Instant::now();
            let res = tokio::time::timeout(timeout, simulate_logs_block(&p, &st.tx, None)).await;
            drop(permit);
            let logs = match res {
                Err(_) => {
                    stats.timeouts.fetch_add(1, Relaxed);
                    if let Some(r) = &rec {
                        r.record(&tx_line(&st, "timeout", None, &[]));
                    }
                    return;
                }
                Ok(Err(_)) => {
                    stats.errors.fetch_add(1, Relaxed);
                    if let Some(r) = &rec {
                        r.record(&tx_line(&st, "error", None, &[]));
                    }
                    return;
                }
                Ok(Ok((logs, sim_block))) => {
                    // The standard feed delivers a transaction after it is sequenced; by the time this RPC
                    // answers, the node's latest block may already contain it.
                    if already_included(sim_block, st.block()) {
                        stats.already_included.fetch_add(1, Relaxed);
                        if let Some(r) = &rec {
                            r.record(&tx_line(&st, "already_included", None, &[]));
                        }
                        return;
                    }
                    logs
                }
            };
            stats
                .sim_ms_total
                .fetch_add(t.elapsed().as_millis() as u64, Relaxed);
            let logs: Vec<_> = pool_events(&logs).map(to_rpc_log).collect();
            stats.tally(&st.tx, !logs.is_empty());
            if let Some(r) = &rec {
                let ms = t.elapsed().as_millis() as u64;
                let outcome = if logs.is_empty() { "none" } else { "moved" };
                let mut moved: Vec<Address> = logs.iter().map(event_pool).collect();
                moved.sort();
                moved.dedup();
                r.record(&tx_line(&st, outcome, Some(ms), &moved));
            }
            if logs.is_empty() {
                stats.no_pool_effect.fetch_add(1, Relaxed);
            } else {
                stats.moved_pools.fetch_add(1, Relaxed);
                let _ = out.send(Presimulated {
                    tx: st.hash,
                    block: st.block(),
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

    #[test]
    fn a_simulation_past_the_transactions_block_already_contains_it() {
        // Ran on top of block 99 -> reports 100: the state before block 100, the transaction is not in it.
        assert!(!already_included(Some(100), 100));
        // Ran on top of block 100 -> reports 101: the transaction (in block 100) is already in the state.
        assert!(already_included(Some(101), 100));
        assert!(already_included(Some(150), 100));
        // A lagging node: older state, transaction not in it (slightly stale, but not double-applied).
        assert!(!already_included(Some(98), 100));
        // Unknown block number: do not discard.
        assert!(!already_included(None, 100));
    }

    #[test]
    fn a_decodable_swap_is_simulated_even_when_it_names_no_watched_address() {
        use alloy::primitives::{Address, U256, address};
        let watch: std::collections::HashSet<[u8; 20]> = [[7u8; 20]].into_iter().collect();
        let b = decode::ROUTER_B;
        let word = |v: U256| v.to_be_bytes::<32>().to_vec();
        let addr_word = |a: Address| {
            let mut o = vec![0u8; 12];
            o.extend_from_slice(a.as_slice());
            o
        };
        let mut call = decode::SELECTOR_B.to_vec();
        call.extend(addr_word(address!(
            "c6962004f452be9203591991d15f6b388e09e8d0"
        )));
        call.extend(word(U256::ZERO));
        call.extend(word(U256::from(1000u64)));
        call.extend(word(U256::from(5u64)));
        call.extend(addr_word(address!(
            "af88d065e77c8cC2239327C5EDb3A432268e5831"
        )));
        assert!(
            !mentions_any(&call, &watch),
            "the address filter alone would drop it"
        );
        assert!(wants_simulation(Some(b), &call, &watch));
        // The same bytes to another contract, or an unrelated call, are still dropped.
        assert!(!wants_simulation(
            Some(Address::with_last_byte(1)),
            &call,
            &watch
        ));
        assert!(!wants_simulation(None, &call, &watch));
        // An address on the watchlist still qualifies without any decoding.
        let mut watched = vec![0u8; 36];
        watched[10..30].copy_from_slice(&[7u8; 20]);
        assert!(wants_simulation(
            Some(Address::with_last_byte(1)),
            &watched,
            &watch
        ));
    }

    #[test]
    fn v4_swaps_are_forwarded_and_the_pool_is_named_by_its_id() {
        use alloy::primitives::{Bytes, LogData};
        assert!(is_pool_event(&IPoolManager::Swap::SIGNATURE_HASH));
        assert!(
            !is_pool_event(&IPoolManager::ModifyLiquidity::SIGNATURE_HASH),
            "liquidity events are deltas"
        );
        assert!(!is_pool_event(&IUniswapV3Pool::Mint::SIGNATURE_HASH));
        let id = B256::repeat_byte(0xab);
        let manager = Address::with_last_byte(0x36);
        let v4 = alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: manager,
                data: LogData::new_unchecked(
                    vec![IPoolManager::Swap::SIGNATURE_HASH, id],
                    Bytes::new(),
                ),
            },
            ..Default::default()
        };
        assert_eq!(event_pool(&v4), Address::from_slice(&id[12..]));
        let v3 = alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: Address::with_last_byte(5),
                data: LogData::new_unchecked(
                    vec![IUniswapV3Pool::Swap::SIGNATURE_HASH],
                    Bytes::new(),
                ),
            },
            ..Default::default()
        };
        assert_eq!(event_pool(&v3), Address::with_last_byte(5));
    }

    #[test]
    fn a_wrapped_swap_makes_the_transaction_a_simulation_candidate_and_is_listed_in_the_record() {
        use alloy::{primitives::U256, sol, sol_types::SolCall};
        sol! {
            struct P { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
            function exactInputSingle(P params) external payable returns (uint256 amountOut);
            struct C3 { address target; bool allowFailure; bytes callData; }
            function aggregate3(C3[] calls) external payable returns (bytes[] returnData);
        }
        let swap = exactInputSingleCall {
            params: P {
                tokenIn: Address::with_last_byte(0xa1),
                tokenOut: Address::with_last_byte(0xb2),
                fee: alloy::primitives::Uint::<24, 1>::from(500u32),
                recipient: Address::with_last_byte(1),
                amountIn: U256::from(5_000u64),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
            },
        }
        .abi_encode();
        let input = aggregate3Call {
            calls: vec![
                C3 {
                    target: alloy::primitives::address!("4752ba5DBc23f44D87826276BF6Fd6b1C372aD24"),
                    allowFailure: true,
                    callData: vec![1, 2, 3, 4].into(),
                },
                C3 {
                    target: mev_decode::calls::MULTICALL3,
                    allowFailure: false,
                    callData: Default::default(),
                },
                C3 {
                    target: mev_protocol_uniswap_router::SWAP_ROUTER_02,
                    allowFailure: false,
                    callData: swap.into(),
                },
            ],
        }
        .abi_encode();
        let to = mev_decode::calls::MULTICALL3;
        // A watch set that matches nothing in the calldata, so only the decoder can make it a candidate.
        let elsewhere: std::collections::HashSet<[u8; 20]> = [[0xee; 20]].into_iter().collect();
        assert!(
            wants_simulation(Some(to), &input, &elsewhere),
            "the swap inside the wrapper is found"
        );
        assert!(
            !wants_simulation(Some(to), &input[..40], &elsewhere),
            "a truncated wrapper decodes nothing"
        );

        let line = tx_line(&seq(Some(to), input), "moved", Some(3), &[]);
        let acts = line["actions"].as_array().unwrap();
        assert_eq!(acts.len(), 3);
        let swaps: Vec<_> = acts.iter().filter(|a| a["kind"] == "swap").collect();
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0]["detail"]["amount_in"], "5000");
        assert_eq!(swaps[0]["via"][0], "multicall3");
        assert!(
            swaps[0]["known"]
                .as_str()
                .unwrap()
                .starts_with("Uniswap SwapRouter02")
        );
        assert_eq!(
            line["undecoded_known"], 1,
            "the Camelot router call is to a known destination nothing decodes yet"
        );
        assert_eq!(acts[0]["undecoded_known"], true);
    }
}
