//! EventIndex against a scripted local JSON-RPC node over a real socket: rate limits, coverage
//! gaps, reorg rewind and resume after a crash between steps.

#[path = "support/fake_node.rs"]
mod fake_node;

use std::time::Duration;

use alloy::primitives::{Address, B256};
use fake_node::{FakeNode, NodeState, chain, hash_of};
use mev_lending_index::{EventIndex, IndexConfig, RetryPolicy, market_key, topic_address};
use serde_json::{Value, json};

const POOL: Address = Address::repeat_byte(0xaa);

fn topic0() -> B256 {
    B256::repeat_byte(0x11)
}

fn acct(n: u8) -> Address {
    Address::repeat_byte(n)
}

fn raw_log(fork: u8, block: u64, who: Address, log_index: u64) -> Value {
    json!({
        "address": format!("{POOL:#x}"),
        "topics": [format!("{:#x}", topic0()), format!("{:#x}", B256::ZERO), format!("{:#x}", who.into_word())],
        "data": "0x",
        "blockHash": format!("{:#x}", hash_of(fork, block)),
        "blockNumber": format!("0x{block:x}"),
        "blockTimestamp": "0x0",
        "transactionHash": format!("{:#x}", B256::repeat_byte(0x22)),
        "transactionIndex": "0x0",
        "logIndex": format!("0x{log_index:x}"),
        "removed": false
    })
}

fn cfg(dir: &std::path::Path, step: u64, max_requests: usize) -> IndexConfig {
    IndexConfig {
        addresses: vec![POOL],
        topics: vec![topic0()],
        from_block: 1,
        step,
        max_requests,
        throttle: Duration::ZERO,
        cache_path: dir.join("idx.json"),
        name: "fake",
    }
}

fn fast_retry(attempts: u32) -> RetryPolicy {
    RetryPolicy {
        attempts,
        max_delay: Duration::from_millis(20),
    }
}

fn extract(l: &alloy::rpc::types::Log) -> Vec<(B256, Address)> {
    topic_address(l, 2)
        .map(|u| vec![(market_key(POOL), u)])
        .unwrap_or_default()
}

#[tokio::test]
async fn rate_limited_scan_retries_with_backoff_and_succeeds() {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs: vec![raw_log(1, 100, acct(1), 0), raw_log(1, 60, acct(2), 0)],
        rate_limit_logs: 2,
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = EventIndex::open(cfg(dir.path(), 50, 20)).with_retry(fast_retry(4));
    let done = idx.step(&p, extract).await.unwrap();
    assert!(done, "history complete after retries");
    assert!(idx.gaps().is_empty());
    assert_eq!(idx.len(), 2);
    assert_eq!(node.state.lock().unwrap().rate_limited, 2);
}

#[tokio::test]
async fn exhausted_retries_record_gaps_instead_of_aborting_and_later_steps_heal_them() {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs: vec![raw_log(1, 100, acct(1), 0), raw_log(1, 20, acct(2), 0)],
        rate_limit_logs: 1000,
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = EventIndex::open(cfg(dir.path(), 50, 40)).with_retry(fast_retry(1));
    let done = idx.step(&p, extract).await.unwrap();
    assert!(!done, "an index with open gaps is not backfilled");
    assert_eq!(idx.gaps().len(), 2, "both history ranges recorded as gaps");
    assert_eq!(idx.len(), 0);
    assert!(idx.gaps()[0].reason.contains("429") || idx.gaps()[0].reason.contains("rate"));

    // Persisted: a reopened index still knows its gaps.
    drop(idx);
    let mut idx = EventIndex::open(cfg(dir.path(), 50, 40)).with_retry(fast_retry(1));
    assert_eq!(idx.gaps().len(), 2);

    node.state.lock().unwrap().rate_limit_logs = 0;
    let done = idx.step(&p, extract).await.unwrap();
    assert!(done);
    assert!(idx.gaps().is_empty());
    assert_eq!(idx.len(), 2, "accounts inside the gaps were recovered");
}

#[tokio::test]
async fn provider_range_limit_shrinks_without_creating_gaps() {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs: vec![raw_log(1, 3, acct(7), 0)],
        max_log_range: Some(8),
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = EventIndex::open(cfg(dir.path(), 100, 200)).with_retry(fast_retry(2));
    assert!(idx.step(&p, extract).await.unwrap());
    assert!(idx.gaps().is_empty());
    assert_eq!(idx.len(), 1);
}

#[tokio::test]
async fn reorg_rewinds_the_cursor_to_the_common_ancestor_and_rescans() {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = EventIndex::open(cfg(dir.path(), 50, 40)).with_retry(fast_retry(2));
    idx.step(&p, extract).await.unwrap(); // initialises at head 100, history is empty

    // Chain grows to 110 with a Borrow at 109 that will be orphaned.
    {
        let mut s = node.state.lock().unwrap();
        for n in 101..=110 {
            s.blocks.insert(n, hash_of(1, n));
        }
        s.logs.push(raw_log(1, 109, acct(5), 0));
    }
    idx.step(&p, extract).await.unwrap();
    assert!(idx.accounts().any(|(_, a)| *a == acct(5)));

    // Reorg from 108: fork 2 up to 115 carries a different Borrow at 112.
    node.reorg(108, 115, 2, vec![raw_log(2, 112, acct(6), 0)]);
    node.state.lock().unwrap().log_requests.clear();
    idx.step(&p, extract).await.unwrap();
    let reqs = node.state.lock().unwrap().log_requests.clone();
    assert!(
        reqs.iter().any(|(f, t)| *f <= 108 && *t >= 115),
        "forward scan restarted at or below the fork point: {reqs:?}"
    );
    assert!(
        idx.accounts().any(|(_, a)| *a == acct(6)),
        "fork-2 borrower found"
    );

    // Once consistent again nothing is rescanned.
    node.state.lock().unwrap().log_requests.clear();
    idx.step(&p, extract).await.unwrap();
    assert!(node.state.lock().unwrap().log_requests.is_empty());
}

#[tokio::test]
async fn resume_after_a_crash_between_range_steps_has_no_gap_or_duplicate() {
    let logs: Vec<Value> = (1..=100u64)
        .step_by(7)
        .enumerate()
        .map(|(i, b)| raw_log(1, b, acct(i as u8 + 1), 0))
        .collect();
    let expected = logs.len();
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs,
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();

    // Process "A": three requests per step, then it dies (state dropped without a clean exit).
    let mut a = EventIndex::open(cfg(dir.path(), 10, 3));
    assert!(!a.step(&p, extract).await.unwrap());
    let after_a = a.len();
    drop(a);

    // Process "B" resumes from the persisted cursor.
    let mut steps = 0;
    loop {
        let mut b = EventIndex::open(cfg(dir.path(), 10, 3));
        steps += 1;
        if b.step(&p, extract).await.unwrap() {
            assert!(b.gaps().is_empty());
            assert_eq!(b.len(), expected, "every borrower exactly once");
            break;
        }
        assert!(steps < 20, "resume must converge");
    }
    assert!(after_a < expected);

    // The ranges requested over the whole run tile 1..=100 exactly once.
    let mut reqs = node.state.lock().unwrap().log_requests.clone();
    reqs.sort_unstable();
    let mut next = 1;
    for (f, t) in reqs {
        assert_eq!(f, next, "contiguous, no overlap, no hole");
        next = t + 1;
    }
    assert_eq!(next, 101);
}

#[tokio::test]
async fn a_lagging_backend_that_lacks_the_tip_does_not_trigger_a_rewind() {
    let node = FakeNode::start(NodeState {
        blocks: chain(1, 1, 100),
        logs: vec![raw_log(1, 50, acct(3), 0)],
        ..Default::default()
    })
    .await;
    let p = mev_rpc::http_provider(&node.url).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = EventIndex::open(cfg(dir.path(), 50, 40)).with_retry(fast_retry(2));
    assert!(idx.step(&p, extract).await.unwrap());
    // The next request lands on a backend that is behind: it does not know block 100.
    node.state.lock().unwrap().blocks.retain(|n, _| *n < 100);
    node.state.lock().unwrap().log_requests.clear();
    idx.step(&p, extract).await.unwrap();
    assert!(
        node.state.lock().unwrap().log_requests.is_empty(),
        "no rescan"
    );
    assert_eq!(idx.len(), 1);
}
