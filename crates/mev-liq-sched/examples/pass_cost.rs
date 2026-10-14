//! Cost of one shadow schedule pass at production scale on a synthetic database: wall time and
//! peak RSS, bounded (`load_inputs_bounded`) or unbounded (`load_inputs`).
//!
//! The population mirrors a mainnet Aave sample: `n` registered borrowers, and in the `steady`
//! phase snapshots for all of them (70% zero debt, 19% under $10, 11% hot). In the `warmup` phase
//! no borrower has been read yet.
//!
//!   cargo run --release -p mev-liq-sched --example pass_cost -- <dir> <n> warmup|steady bounded|full [passes]

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256};
use mev_liq_sched::{LoadBounds, PolicyConfig, ShadowParams, run_shadow_pass};
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainRef, ChainSource, SnapshotRepository, SnapshotStatus,
};

const CHAIN: u64 = 42161;
const USD: u64 = 100_000_000;

/// Peak and current resident set (kB) from /proc (Linux only; 0 elsewhere).
fn rss_kb() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (get("VmHWM:"), get("VmRSS:"))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn who(i: u32) -> Address {
    let mut b = [0u8; 20];
    b[16..].copy_from_slice(&i.to_be_bytes());
    Address::from(b)
}

fn snap(pool: Address, i: u32, now: u64) -> BorrowerSnapshot {
    let debt_usd = match i % 100 {
        0..70 => 0,
        70..89 => 3,
        _ => 5_000,
    };
    let at = now - u64::from(i % 300) * 1_000_000_000;
    BorrowerSnapshot {
        key: AccountKey {
            chain_id: CHAIN,
            pool,
            borrower: who(i),
        },
        revision: 0,
        chain: ChainRef {
            block_number: 1_000,
            block_hash: B256::repeat_byte(1),
            source: ChainSource::Provisional,
            observed_at_ns: at,
        },
        health_factor_wad: if debt_usd == 0 {
            U256::MAX
        } else {
            U256::from(1_300_000_000_000_000_000u64)
        },
        total_debt_base: U256::from(debt_usd * USD),
        total_collateral_base: U256::from(2 * debt_usd * USD),
        available_borrows_base: U256::ZERO,
        liquidation_threshold: U256::from(8_250u64),
        ltv: U256::from(8_000u64),
        reserves: vec![],
        risk_metadata_version: 0,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(at),
        status: SnapshotStatus::Valid,
        provenance: "synthetic".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, dir, n, phase, mode, rest @ ..] = args.as_slice() else {
        eprintln!("usage: pass_cost <dir> <n> warmup|steady bounded|full [passes]");
        std::process::exit(2);
    };
    let n: u32 = n.parse().expect("n");
    let passes: usize = rest.first().and_then(|p| p.parse().ok()).unwrap_or(3);
    let pool = Address::repeat_byte(0xaa);
    let path = std::path::Path::new(dir).join(format!("pass-cost-{phase}-{n}.sqlite"));
    let fresh = !path.exists();
    let repo = SnapshotRepository::open(&path).expect("open db");
    if fresh {
        let t = Instant::now();
        let all: Vec<Address> = (0..n).map(who).collect();
        // Discovered (and marked dirty) an hour ago: every later read clears the mark, as in
        // production where the recheck always follows discovery.
        repo.register_borrowers(CHAIN, pool, &all, None, now_ns() - 3_600_000_000_000)
            .expect("register");
        if phase == "steady" {
            let now = now_ns();
            for chunk in (0..n).collect::<Vec<_>>().chunks(5_000) {
                let snaps: Vec<_> = chunk.iter().map(|i| snap(pool, *i, now)).collect();
                repo.store_rechecks(&snaps).expect("store");
            }
        }
        println!("built {} in {} ms", path.display(), t.elapsed().as_millis());
    }
    let p = ShadowParams {
        chain_id: CHAIN,
        pool,
        scan_name: "aave-v3-triggers".into(),
        policy: PolicyConfig::default(),
        max_records: 50,
        bounds: (mode == "bounded").then_some(LoadBounds {
            min_debt_base: U256::from(10 * USD),
            max_unknown: 2000,
        }),
    };
    // Measure from a clean peak: the build above is not part of a pass.
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    let (hwm0, rss0) = rss_kb();
    println!("phase={phase} mode={mode} n={n} rss_kb={rss0} hwm_kb={hwm0}");
    for i in 0..passes {
        let t = Instant::now();
        let r = run_shadow_pass(&repo, &p, now_ns()).expect("pass");
        let (hwm, rss) = rss_kb();
        println!(
            "pass={i} ms={} tracked={} deferred_unknown={} violations={} rss_kb={rss} hwm_kb={hwm}",
            t.elapsed().as_millis(),
            r.counters.tracked,
            r.counters.deferred_unknown,
            r.counters.coverage_violations,
        );
    }
}
