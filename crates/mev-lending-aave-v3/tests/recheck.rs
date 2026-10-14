//! Batched rechecks against a scripted local JSON-RPC node: one pinned block for the whole batch,
//! input order kept, a failed account call is `None` and never a snapshot.

#[path = "../../mev-lending-index/tests/support/fake_node.rs"]
mod fake_node;

use alloy::{
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    sol_types::SolCall,
};
use fake_node::{FakeNode, NodeState, chain, hash_of};
use mev_bindings::{IAavePool, IMulticall3, MULTICALL3};
use mev_lending_aave_v3::recheck::{AaveAccountRechecker, PIN_LAG_BLOCKS, PROVENANCE_BATCH};
use mev_liq_state::{ChainSource, SnapshotStatus};

const POOL: Address = alloy::primitives::address!("794a61358D6845594F94dc1DB02A252b5b4814aD");

fn account(debt: u64, hf_milli: u64) -> Bytes {
    IAavePool::getUserAccountDataCall::abi_encode_returns(&IAavePool::getUserAccountDataReturn {
        totalCollateralBase: U256::from(2 * debt),
        totalDebtBase: U256::from(debt),
        availableBorrowsBase: U256::from(7u64),
        currentLiquidationThreshold: U256::from(8_250u64),
        ltv: U256::from(8_000u64),
        healthFactor: U256::from(hf_milli) * U256::from(1_000_000_000_000_000u64),
    })
    .into()
}

#[tokio::test]
async fn recheck_many_reads_every_account_at_one_lagged_block() {
    let results = vec![
        IMulticall3::Result {
            success: true,
            returnData: account(5_000_000_000, 1_250),
        },
        IMulticall3::Result {
            success: false,
            returnData: Bytes::new(),
        },
        IMulticall3::Result {
            success: true,
            returnData: account(90_000_000_000, 980),
        },
    ];
    let mut st = NodeState {
        blocks: chain(0, 1, 100),
        ..Default::default()
    };
    st.calls.insert(
        (MULTICALL3, IMulticall3::aggregate3Call::SELECTOR),
        format!(
            "0x{}",
            alloy::hex::encode(IMulticall3::aggregate3Call::abi_encode_returns(&results))
        ),
    );
    let node = FakeNode::start(st).await;
    let provider = ProviderBuilder::new()
        .connect_http(node.url.parse().unwrap())
        .erased();
    assert_eq!(provider.get_block_number().await.unwrap(), 100);

    let rc = AaveAccountRechecker {
        chain_id: 42161,
        pool: POOL,
        provider,
        risk_metadata_version: 3,
    };
    let users = [
        Address::with_last_byte(1),
        Address::with_last_byte(2),
        Address::with_last_byte(3),
    ];
    let (at, snaps) = rc.recheck_many(&users).await.unwrap();

    assert_eq!(at.block_number, 100 - PIN_LAG_BLOCKS);
    assert_eq!(at.block_hash, hash_of(0, 100 - PIN_LAG_BLOCKS));
    assert_eq!(at.source, ChainSource::Provisional);
    assert_eq!(snaps.len(), 3);
    assert!(snaps[1].is_none(), "a failed call is not a snapshot");
    let (a, c) = (snaps[0].as_ref().unwrap(), snaps[2].as_ref().unwrap());
    assert_eq!(a.key.borrower, users[0]);
    assert_eq!(c.key.borrower, users[2]);
    for s in [a, c] {
        assert_eq!(s.chain.block_hash, at.block_hash, "one block for the batch");
        assert_eq!(s.status, SnapshotStatus::Valid);
        assert_eq!(s.provenance, PROVENANCE_BATCH);
        assert_eq!(s.risk_metadata_version, 3);
        assert!(s.digest_matches());
    }
    assert_eq!(a.total_debt_base, U256::from(5_000_000_000u64));
    assert!(!a.eligible());
    assert!(c.eligible(), "HF 0.98 with debt");
}

#[tokio::test]
async fn recheck_many_fails_whole_batch_when_the_request_fails() {
    // No aggregate3 result scripted: the node reverts the request.
    let node = FakeNode::start(NodeState {
        blocks: chain(0, 1, 50),
        ..Default::default()
    })
    .await;
    let rc = AaveAccountRechecker {
        chain_id: 42161,
        pool: POOL,
        provider: ProviderBuilder::new()
            .connect_http(node.url.parse().unwrap())
            .erased(),
        risk_metadata_version: 0,
    };
    assert!(
        rc.recheck_many(&[Address::with_last_byte(1)])
            .await
            .is_err()
    );
}
