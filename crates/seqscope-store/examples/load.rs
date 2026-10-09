//! Engine + store under load: `cargo run --release -p seqscope-store --example load [seconds] [tx_per_second]`.
//! Feeds synthetic transactions through the rule engine into the writer at a fixed rate (0 = as fast as possible) and
//! reports what the feed side experienced: send latency, drops, and whether every row is accounted for.

use std::time::{Duration, Instant};

use alloy::primitives::{Address, B256, Bytes, U256};
use seqscope_core::{ActionKind, Engine, Mode, Profile, Rule, RuleSet, Tx};
use seqscope_store::{Reader, Writer, WriterConfig};

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let rate: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let path = std::env::temp_dir().join(format!("seqscope-load-{}.sqlite", std::process::id()));
    let set = RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "all".into(),
            enabled: true,
            mode: Mode::All,
            rule: vec![Rule {
                name: "everything".into(),
                enabled: true,
                when: "true".into(),
                action: ActionKind::Store,
                tag: None,
                sample: 1,
            }],
        }],
    };
    let engine = Engine::compile(&set).unwrap();
    let w = Writer::spawn(&path, WriterConfig::default()).unwrap();
    let ing = w.ingest();
    let mut lat: Vec<u32> = Vec::new();
    let start = Instant::now();
    let mut i: u64 = 0;
    let mut produced = 0u64;
    while start.elapsed() < Duration::from_secs(secs) {
        if rate > 0 {
            let due = (start.elapsed().as_secs_f64() * rate as f64) as u64;
            if i >= due {
                std::hint::spin_loop();
                continue;
            }
        }
        let mut h = [0u8; 32];
        h[..8].copy_from_slice(&i.to_be_bytes());
        let tx = Tx {
            hash: B256::from(h),
            seq: i,
            t_ms: 1_700_000_000_000 + i / 10,
            from: Some(Address::with_last_byte(1)),
            to: Some(Address::with_last_byte((i % 200) as u8)),
            tx_type: 2,
            value: U256::ZERO,
            gas_limit: 300_000,
            max_fee: 1,
            tip: Some(1),
            nonce: i,
            input: Bytes::from(vec![0xa9, 0x05, 0x9c, 0xbb, (i % 251) as u8, 0, 0, 0]),
        };
        for r in engine.process(&tx) {
            let t = Instant::now();
            ing.send(r);
            lat.push(t.elapsed().as_nanos().min(u128::from(u32::MAX)) as u32);
            produced += 1;
        }
        i += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    drop(ing);
    let s = w.shutdown();
    lat.sort_unstable();
    let p = |q: f64| f64::from(lat[((lat.len() as f64 - 1.0) * q) as usize]) / 1000.0;
    println!(
        "offered {:.0} tx/s ({} rows) for {:.1}s",
        i as f64 / elapsed,
        produced,
        elapsed
    );
    println!(
        "send latency us: p50 {:.2}  p99 {:.2}  p99.99 {:.2}  max {:.2}",
        p(0.5),
        p(0.99),
        p(0.9999),
        f64::from(*lat.last().unwrap()) / 1000.0
    );
    println!(
        "writer: written {}  dropped {}  failed {}  batches {}  slowest batch {:.1} ms",
        s.written,
        s.dropped,
        s.failed,
        s.batches,
        s.max_batch_us as f64 / 1000.0
    );
    println!(
        "accounted: {} of {} rows ({})",
        s.written + s.dropped + s.failed,
        produced,
        if s.written + s.dropped + s.failed == produced {
            "ok"
        } else {
            "MISMATCH"
        }
    );
    let stored = Reader::open(&path).unwrap().count().unwrap();
    println!(
        "rows in the database: {stored} ({})",
        if stored == s.written {
            "matches written"
        } else {
            "MISMATCH"
        }
    );
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
}
