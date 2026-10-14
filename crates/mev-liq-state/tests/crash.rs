//! Real crash test: a child process applies scan batches in a loop and is SIGKILLed at random
//! points; after every kill the database must show the cursor and the marks of exactly the same
//! committed batches (atomic checkpoint), and the next child must resume from it.

use std::{path::Path, process::Command, time::Duration};

use alloy::primitives::{Address, B256};
use mev_liq_state::*;

const BASE: u64 = 1_000;
const CHAIN: u64 = 42161;

fn pool() -> Address {
    Address::repeat_byte(0xaa)
}

fn borrower(i: u64) -> Address {
    let mut b = [0u8; 20];
    b[12..].copy_from_slice(&i.to_be_bytes());
    Address::from(b)
}

/// Child entry point (a no-op unless the parent sets the env var).
#[test]
fn crash_child() {
    let Ok(path) = std::env::var("LIQ_CRASH_DB") else {
        return;
    };
    let r = SnapshotRepository::open(Path::new(&path)).unwrap();
    r.init_scan_cursor(
        "n",
        ScanCursor {
            next_block: BASE,
            tip: None,
            back_block: BASE,
        },
    )
    .unwrap();
    loop {
        let c = r.scan_cursor("n").unwrap().unwrap();
        let i = c.next_block - BASE;
        let at = ChainPoint {
            block_number: c.next_block,
            block_hash: B256::repeat_byte(7),
        };
        let mut b = ScanBatch::new("n", CHAIN, pool(), i + 1, 100);
        // Several effects per batch so a torn write would be visible.
        for k in 0..5 {
            b.effects.push(Effect::Touch {
                borrower: borrower(i * 5 + k),
                reason: DirtyReason::BorrowerEvent,
                at,
                register: Some(at),
                stale_deps: false,
            });
        }
        b.next_block = Some(c.next_block + 1);
        b.tip = Some(at);
        b.ancestry = vec![at];
        r.apply_scan(&b).unwrap();
    }
}

#[test]
fn sigkill_between_and_during_batches_never_tears_cursor_and_marks() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("s.db");
    let exe = std::env::current_exe().unwrap();
    let mut progressed = 0u64;
    for round in 0..6u64 {
        let mut child = Command::new(&exe)
            .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
            .env("LIQ_CRASH_DB", &db)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(150 + 53 * round));
        child.kill().unwrap(); // SIGKILL: no destructors, no flush
        child.wait().unwrap();

        let r = SnapshotRepository::open(&db).unwrap();
        let Some(c) = r.scan_cursor("n").unwrap() else {
            continue; // killed before the first commit
        };
        let batches = c.next_block - BASE;
        let regs = r.active_borrowers(CHAIN, pool()).unwrap().len() as u64;
        assert_eq!(
            regs,
            batches * 5,
            "round {round}: cursor ahead of/behind the registry"
        );
        let dirty = r.dirty_entries().unwrap();
        assert_eq!(
            dirty.len() as u64,
            batches * 5,
            "round {round}: marks match cursor"
        );
        assert!(dirty.iter().all(|e| e.reasons.len() == 2));
        assert_eq!(
            c.tip.map(|t| t.block_number),
            (batches > 0).then_some(c.next_block - 1)
        );
        assert!(batches >= progressed, "progress never goes backwards");
        progressed = batches;
    }
    assert!(
        progressed > 0,
        "the child committed at least one batch across the rounds"
    );
}
