//! Live read-only check (ignored): the factory creation events of every configured exchange decode into pools
//! that really exist (their token0/token1 match the pool contract). Scans a recent range of Arbitrum.
//!   ARBITRUM_LOG_RPC=https://arb1.arbitrum.io/rpc cargo test -p mev-discovery --test created_live -- --ignored --nocapture

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    rpc::types::Filter,
    sol,
};
use mev_discovery::created;

sol! {
    #[sol(rpc)]
    interface IPoolMeta { function token0() external view returns (address); function token1() external view returns (address); }
}

#[tokio::test]
#[ignore = "network"]
async fn recent_creation_events_decode_to_real_pools() {
    let url =
        std::env::var("ARBITRUM_LOG_RPC").unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".into());
    let p = ProviderBuilder::new().connect_http(url.parse().unwrap());
    let cfg =
        mev_config::ChainConfig::load(std::path::Path::new("../../config/arbitrum.toml")).unwrap();
    let head = p.get_block_number().await.unwrap();
    let mut by_dex = std::collections::BTreeMap::<String, usize>::new();
    let mut checked = 0;
    let span = 1_000_000u64; // ~3 days of Arbitrum blocks, in the 100,000-block slices public endpoints allow
    let mut logs = Vec::new();
    for start in (head - span..head).step_by(100_000) {
        let filter = Filter::new()
            .address(created::factories(&cfg.dexes))
            .event_signature(created::creation_topics())
            .from_block(start)
            .to_block((start + 99_999).min(head));
        logs.extend(
            p.get_logs(&filter)
                .await
                .unwrap_or_else(|e| panic!("get_logs: {e}")),
        );
    }
    for log in &logs {
        let c = created::parse(log, &cfg.dexes)
            .unwrap_or_else(|| panic!("undecodable creation log {:?}", log.transaction_hash));
        *by_dex.entry(c.info.dex.clone()).or_default() += 1;
        if checked < 12 {
            let pool = IPoolMeta::new(c.info.address, &p);
            let (t0, t1): (Address, Address) = (
                pool.token0().call().await.unwrap(),
                pool.token1().call().await.unwrap(),
            );
            assert_eq!(
                (t0, t1),
                (c.info.token0, c.info.token1),
                "pool {} tokens differ from the event",
                c.info.address
            );
            checked += 1;
        }
    }
    println!(
        "{} creations in the last {span} blocks: {by_dex:?}; {checked} verified against the pool contracts",
        logs.len()
    );
    assert!(
        !logs.is_empty(),
        "no creation events found; widen the range"
    );
}
