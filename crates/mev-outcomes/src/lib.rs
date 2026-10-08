//! Chain-side ground truth for what the sequencer feed showed us.
//!
//! The feed carries signed transaction bodies only: no success or revert, no gas used, no events. This crate
//! subscribes to new blocks, and for every `every_n`-th block fetches all receipts in one `eth_getBlockReceipts`
//! call. It writes `chaintx-DATE.jsonl` lines for
//!  - every transaction the feed recorder saw (so revert rates, gas and block position can be joined on the hash), and
//!  - every transaction that emitted a swap, liquidity or liquidation event, even if the feed recorder did not see it
//!    as a candidate (these are the decoder's coverage gaps: group them by `to`).
//!
//! Sampling keeps the RPC cost at `4 / every_n` calls a second (Arbitrum makes about 4 blocks a second).

// `sol!` generates a constructor-like function per event; Morpho's `Liquidate` has 8 fields.
#![allow(clippy::too_many_arguments)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    primitives::B256,
    providers::{DynProvider, Provider},
    rpc::types::{Log, TransactionReceipt},
    sol,
    sol_types::SolEvent,
};
use futures::StreamExt;
use mev_bindings::{IPancakeV3Pool, IPoolManager, IUniswapV2Pair, IUniswapV3Pool};
use mev_presim::record::TxRecorder;
use tracing::{info, warn};

// Event shapes not in `mev-bindings`, each in its own interface so the event is named exactly `Swap`/`Mint`/...
sol! {
    interface IV2Events {
        event Swap(address indexed sender, uint256 amount0In, uint256 amount1In, uint256 amount0Out, uint256 amount1Out, address indexed to);
        event Mint(address indexed sender, uint256 amount0, uint256 amount1);
        event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    }
    interface IAaveEvents {
        event LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken);
    }
    interface IMorphoEvents {
        event Liquidate(bytes32 indexed id, address indexed caller, address indexed borrower, uint256 repaidAssets, uint256 repaidShares, uint256 seizedAssets, uint256 badDebtAssets, uint256 badDebtShares);
    }
    interface ICometEvents {
        event AbsorbCollateral(address indexed absorber, address indexed borrower, address indexed asset, uint256 collateralAbsorbed, uint256 usdValue);
        event AbsorbDebt(address indexed absorber, address indexed borrower, uint256 basePaidOut, uint256 usdValue);
    }
    interface IBalancerEvents {
        event Swap(bytes32 indexed poolId, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut);
    }
    interface INftManagerEvents {
        event IncreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
        event DecreaseLiquidity(uint256 indexed tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);
    }
}

/// Event kinds worth keeping, by topic0. "swap"/"liquidity"/"liquidation" is the class a decoder must cover.
pub fn kinds() -> &'static [(B256, &'static str, &'static str)] {
    static T: std::sync::OnceLock<Vec<(B256, &'static str, &'static str)>> =
        std::sync::OnceLock::new();
    T.get_or_init(|| {
        vec![
            (IV2Events::Swap::SIGNATURE_HASH, "swap_v2", "swap"),
            (IUniswapV2Pair::Sync::SIGNATURE_HASH, "sync_v2", "state"),
            (IV2Events::Mint::SIGNATURE_HASH, "mint_v2", "liquidity"),
            (IV2Events::Burn::SIGNATURE_HASH, "burn_v2", "liquidity"),
            (IUniswapV3Pool::Swap::SIGNATURE_HASH, "swap_v3", "swap"),
            (
                IPancakeV3Pool::Swap::SIGNATURE_HASH,
                "swap_v3_pancake",
                "swap",
            ),
            (IUniswapV3Pool::Mint::SIGNATURE_HASH, "mint_v3", "liquidity"),
            (IUniswapV3Pool::Burn::SIGNATURE_HASH, "burn_v3", "liquidity"),
            (
                INftManagerEvents::IncreaseLiquidity::SIGNATURE_HASH,
                "increase_liquidity_nft",
                "liquidity",
            ),
            (
                INftManagerEvents::DecreaseLiquidity::SIGNATURE_HASH,
                "decrease_liquidity_nft",
                "liquidity",
            ),
            (IPoolManager::Swap::SIGNATURE_HASH, "swap_v4", "swap"),
            (
                IPoolManager::ModifyLiquidity::SIGNATURE_HASH,
                "modify_liquidity_v4",
                "liquidity",
            ),
            (
                IBalancerEvents::Swap::SIGNATURE_HASH,
                "swap_balancer",
                "swap",
            ),
            (
                IAaveEvents::LiquidationCall::SIGNATURE_HASH,
                "liquidation_aave",
                "liquidation",
            ),
            (
                IMorphoEvents::Liquidate::SIGNATURE_HASH,
                "liquidation_morpho",
                "liquidation",
            ),
            (
                ICometEvents::AbsorbCollateral::SIGNATURE_HASH,
                "absorb_collateral_compound",
                "liquidation",
            ),
            (
                ICometEvents::AbsorbDebt::SIGNATURE_HASH,
                "absorb_debt_compound",
                "liquidation",
            ),
        ]
    })
}

/// `(kind, class)` of a log, if it is one we keep.
pub fn classify(log: &Log) -> Option<(&'static str, &'static str)> {
    let t0 = log.topics().first()?;
    kinds()
        .iter()
        .find(|(h, _, _)| h == t0)
        .map(|(_, k, c)| (*k, *c))
}

/// Logs worth keeping from a receipt, as JSON, with the classes present.
fn interesting(logs: &[Log]) -> (Vec<serde_json::Value>, Vec<&'static str>) {
    let mut out = Vec::new();
    let mut classes: Vec<&'static str> = Vec::new();
    for l in logs {
        let Some((kind, class)) = classify(l) else {
            continue;
        };
        if !classes.contains(&class) {
            classes.push(class);
        }
        let data = l.data().data.as_ref();
        out.push(serde_json::json!({
            "addr": format!("{:#x}", l.address()),
            "kind": kind,
            "topics": l.topics().iter().map(|t| format!("{t:#x}")).collect::<Vec<_>>(),
            "data": alloy::hex::encode(&data[..data.len().min(256)]),
        }));
    }
    (out, classes)
}

/// The line for a receipt, or `None` if it is neither a recorded feed transaction nor an event we keep.
pub fn outcome_line(r: &TransactionReceipt, feed: &TxRecorder) -> Option<serde_json::Value> {
    let hash = format!("{:#x}", r.transaction_hash);
    let was_candidate = feed.was_recorded(&hash);
    let (logs, classes) = interesting(r.logs());
    if !was_candidate && logs.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "hash": hash,
        "block": r.block_number,
        "index": r.transaction_index,
        "from": format!("{:#x}", r.from),
        "to": r.to.map(|a| format!("{a:#x}")),
        "status": r.status(),
        "gas_used": r.gas_used,
        "gas_price": r.effective_gas_price.to_string(),
        "n_logs": r.logs().len(),
        "candidate": was_candidate,
        "classes": classes,
        "logs": logs,
    }))
}

#[derive(Default)]
struct Counters {
    blocks: AtomicU64,
    txs: AtomicU64,
    recorded: AtomicU64,
    reverted: AtomicU64,
    empty: AtomicU64,
}

async fn fetch(ws: &DynProvider, n: u64) -> Option<Vec<TransactionReceipt>> {
    for attempt in 0..4 {
        match ws
            .get_block_receipts(BlockId::Number(BlockNumberOrTag::Number(n)))
            .await
        {
            Ok(Some(r)) => return Some(r),
            Ok(None) => {} // not served yet
            Err(e) => {
                if attempt == 3 {
                    warn!(error = %e, block = n, "block receipts failed");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

/// Follow new blocks forever (reconnecting), sampling every `every_n`-th block into `out`.
pub async fn run(ws: DynProvider, feed: Arc<TxRecorder>, out: Arc<TxRecorder>, every_n: u64) {
    let every_n = every_n.max(1);
    let c = Arc::new(Counters::default());
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        let sub = match ws.subscribe_blocks().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "block subscription failed; retrying");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut stream = sub.into_stream();
        info!(every_n, "chain outcome sampling on");
        loop {
            tokio::select! {
                h = stream.next() => {
                    let Some(h) = h else { break };
                    if h.number % every_n != 0 {
                        continue;
                    }
                    let (ws, feed, out, c) = (ws.clone(), feed.clone(), out.clone(), c.clone());
                    tokio::spawn(async move {
                        let Some(receipts) = fetch(&ws, h.number).await else {
                            c.empty.fetch_add(1, Relaxed);
                            return;
                        };
                        c.blocks.fetch_add(1, Relaxed);
                        c.txs.fetch_add(receipts.len() as u64, Relaxed);
                        for r in &receipts {
                            if !r.status() {
                                c.reverted.fetch_add(1, Relaxed);
                            }
                            if let Some(line) = outcome_line(r, &feed) {
                                c.recorded.fetch_add(1, Relaxed);
                                out.record(&line);
                            }
                        }
                    });
                }
                _ = tick.tick() => {
                    info!(
                        blocks = c.blocks.swap(0, Relaxed),
                        txs = c.txs.swap(0, Relaxed),
                        recorded = c.recorded.swap(0, Relaxed),
                        reverted = c.reverted.swap(0, Relaxed),
                        fetch_failed = c.empty.swap(0, Relaxed),
                        dropped = out.dropped(),
                        "chain outcomes (last minute)"
                    );
                }
            }
        }
        warn!("block subscription ended; resubscribing");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, Bytes, LogData, b256};

    fn log(topic0: B256, topics_extra: usize) -> Log {
        let mut topics = vec![topic0];
        topics.extend(std::iter::repeat_n(B256::ZERO, topics_extra));
        Log {
            inner: alloy::primitives::Log {
                address: Address::with_last_byte(7),
                data: LogData::new_unchecked(topics, Bytes::from(vec![1u8; 32])),
            },
            ..Default::default()
        }
    }

    #[test]
    fn well_known_event_hashes_match() {
        // Known mainnet topic0 values, so a typo in an event definition cannot go unnoticed.
        let known = [
            (
                "swap_v3",
                b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"),
            ),
            (
                "swap_v2",
                b256!("d78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822"),
            ),
            (
                "sync_v2",
                b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"),
            ),
            (
                "liquidation_aave",
                b256!("e413a321e8681d831f4dbccbca790d2952b56f977908e45be37335533e005286"),
            ),
        ];
        for (name, hash) in known {
            let found = kinds()
                .iter()
                .find(|(_, k, _)| *k == name)
                .map(|(h, _, _)| *h);
            assert_eq!(found, Some(hash), "{name}");
        }
    }

    #[test]
    fn every_kind_has_a_distinct_topic() {
        let mut seen = std::collections::HashSet::new();
        for (h, k, _) in kinds() {
            assert!(seen.insert(*h), "duplicate topic for {k}");
        }
        assert!(kinds().len() >= 17);
    }

    #[test]
    fn logs_are_classified_and_unknown_ones_ignored() {
        let swap = log(IUniswapV3Pool::Swap::SIGNATURE_HASH, 2);
        let junk = log(B256::repeat_byte(0x11), 0);
        assert_eq!(classify(&swap), Some(("swap_v3", "swap")));
        assert_eq!(classify(&junk), None);
        let (kept, classes) = interesting(&[swap, junk]);
        assert_eq!(kept.len(), 1);
        assert_eq!(classes, vec!["swap"]);
        assert_eq!(kept[0]["kind"], "swap_v3");
    }
}
