//! Live read-only check (ignored): re-simulating a transaction on a state that already contains it applies it
//! twice, and `already_included` recognises that state from the simulated block number.
//! Needs ARBITRUM_HTTP_URL pointing at an archive-capable node with `eth_simulateV1` (the public arb1 RPC works).
//! The swap is the one measured on 2026-10-08: pool 0x68eb... (Uniswap V3 0.3%), block 512764217.

use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256, address, b256},
    providers::{Provider, ProviderBuilder},
    rpc::types::Filter,
};
use mev_presim::{already_included, simulate_logs_block};

const POOL: Address = address!("68ebadf62ee5acce5f8d64211d24b4710eeb2029");
const SWAP_TOPIC: B256 = b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67");
const BLOCK: u64 = 512_764_217;

fn sqrt_price_after(logs: &[mev_presim::SimLog]) -> Option<f64> {
    let l = logs
        .iter()
        .find(|l| l.address == POOL && l.topics.first() == Some(&SWAP_TOPIC))?;
    // Swap data: amount0, amount1, sqrtPriceX96, liquidity, tick (32-byte words).
    let word = &l.data[64..96];
    let mut v = 0f64;
    for b in word {
        v = v * 256.0 + f64::from(*b);
    }
    Some(v / 2f64.powi(96))
}

#[tokio::test]
#[ignore = "network"]
async fn a_landed_swap_resimulated_on_its_own_block_is_applied_twice_and_detected() {
    let url = std::env::var("ARBITRUM_HTTP_URL").expect("ARBITRUM_HTTP_URL");
    let p = ProviderBuilder::new()
        .connect_http(url.parse().unwrap())
        .erased();
    let log = p
        .get_logs(
            &Filter::new()
                .address(POOL)
                .event_signature(SWAP_TOPIC)
                .from_block(BLOCK)
                .to_block(BLOCK),
        )
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the swap is in that block");
    let tx = p
        .get_transaction_by_hash(log.transaction_hash.unwrap())
        .await
        .unwrap()
        .expect("transaction")
        .inner
        .into_inner();
    let _ = BlockNumberOrTag::Latest;

    // On the state BEFORE the swap's block: the simulation reports the swap's own block, and matches the chain.
    let (before_logs, before_block) = simulate_logs_block(&p, &tx, Some(BLOCK - 1)).await.unwrap();
    assert_eq!(before_block, Some(BLOCK));
    assert!(
        !already_included(before_block, BLOCK),
        "valid: the swap is not in that state"
    );
    let chain = sqrt_price_after(&before_logs).expect("a swap log on the pool");

    // On the swap's own block: the state already contains it; the simulation reports the next block.
    let (after_logs, after_block) = simulate_logs_block(&p, &tx, Some(BLOCK)).await.unwrap();
    assert_eq!(after_block, Some(BLOCK + 1));
    assert!(
        already_included(after_block, BLOCK),
        "must be discarded by presim"
    );
    let doubled = sqrt_price_after(&after_logs).expect("a swap log on the pool");

    let bps = (doubled / chain - 1.0) * 2e4; // price = sqrt^2
    println!("chain {chain:.9e}, applied twice {doubled:.9e}: {bps:+.2} bps beyond the chain");
    assert!(
        bps.abs() > 0.5,
        "applying it twice visibly moves the price: {bps}"
    );
}
