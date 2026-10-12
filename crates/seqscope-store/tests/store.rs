use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use alloy::primitives::B256;
use seqscope_core::Row;
use seqscope_store::{Query, Reader, SCHEMA_VERSION, Writer, WriterConfig, disk_bytes, prune};

static N: AtomicU32 = AtomicU32::new(0);

#[test]
fn sql_limits_bound_values_work_and_leave_the_connection_usable() {
    let db = Db::new();
    drop(rusqlite::Connection::open(&db.0).unwrap());
    let r = Reader::open(&db.0).unwrap();
    assert!(r.sql("SELECT hex(zeroblob(3000000))", 1).is_err());
    assert!(
        r.sql(
            "WITH RECURSIVE t(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM t) SELECT sum(x) FROM t",
            1
        )
        .is_err()
    );
    assert_eq!(r.sql("SELECT 1", 1).unwrap().rows[0][0], 1);
}

/// A fresh database path that is removed when dropped.
struct Db(PathBuf);
impl Db {
    fn new() -> Db {
        let p = std::env::temp_dir().join(format!(
            "seqscope-test-{}-{}.sqlite",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        Db(p)
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", self.0.display(), suffix));
        }
    }
}

fn row(i: u64) -> Row {
    Row {
        t_ms: 1_700_000_000_000 + i,
        seq: i,
        hash: B256::from(alloy::primitives::U256::from(i + 1)),
        from: Some(format!("0x{:040x}", 0xf0)),
        tx_to: Some(format!("0x{:040x}", 0x10 + i % 3)),
        to: Some(format!("0x{:040x}", 0x20 + i % 5)),
        selector: Some(format!("0x{:08x}", 0xa415bcad_u32 + (i % 4) as u32)),
        via: if i.is_multiple_of(2) {
            vec![]
        } else {
            vec!["multicall3".into(), "self_multicall".into()]
        },
        protocol: Some(["aave_v3", "uniswap_v3"][(i % 2) as usize].into()),
        known: Some("Some Router".into()),
        kind: ["swap", "liquidity", "unknown"][(i % 3) as usize].into(),
        status: "pending".into(),
        fields: BTreeMap::from([("swap.amount_in".to_string(), (i * 1000).to_string())]),
        profile: "p".into(),
        rule: format!("r{}", i % 2),
        tag: (i.is_multiple_of(4)).then(|| "t".to_string()),
        alert: i.is_multiple_of(7),
        len: 68,
        input: (i.is_multiple_of(5)).then(|| "0xdeadbeef".to_string()),
    }
}

fn bare(i: u64) -> Row {
    Row {
        from: None,
        tx_to: None,
        to: None,
        selector: None,
        via: vec![],
        protocol: None,
        known: None,
        fields: BTreeMap::new(),
        tag: None,
        alert: false,
        input: None,
        len: 0,
        ..row(i)
    }
}

fn write(db: &Db, rows: &[Row]) {
    let w = Writer::spawn(&db.0, WriterConfig::default()).unwrap();
    let ing = w.ingest();
    for r in rows {
        assert!(ing.send(r.clone()));
    }
    drop(ing);
    let s = w.shutdown();
    assert_eq!(
        (s.written as usize, s.dropped, s.failed),
        (rows.len(), 0, 0),
        "{s:?}"
    );
}

#[test]
fn rows_round_trip_exactly_including_missing_values() {
    let db = Db::new();
    let rows: Vec<Row> = (0..40).map(row).chain((40..50).map(bare)).collect();
    write(&db, &rows);
    let r = Reader::open(&db.0).unwrap();
    assert_eq!(r.count().unwrap(), 50);
    let mut got = r
        .query(&Query {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    got.sort_by_key(|x| x.seq);
    assert_eq!(got, rows);
}

#[test]
fn queries_filter_order_and_limit() {
    let db = Db::new();
    let rows: Vec<Row> = (0..60).map(row).collect();
    write(&db, &rows);
    let r = Reader::open(&db.0).unwrap();
    let newest = r
        .query(&Query {
            limit: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        newest.iter().map(|x| x.seq).collect::<Vec<_>>(),
        vec![59, 58, 57],
        "newest first"
    );
    let q = |f: &dyn Fn(&mut Query)| {
        let mut q = Query {
            limit: Some(1000),
            ..Default::default()
        };
        f(&mut q);
        r.query(&q).unwrap()
    };
    assert_eq!(q(&|q| q.kind = Some("swap".into())).len(), 20);
    assert_eq!(q(&|q| q.protocol = Some("aave_v3".into())).len(), 30);
    assert_eq!(
        q(&|q| {
            q.protocol = Some("aave_v3".into());
            q.kind = Some("swap".into())
        })
        .len(),
        10
    );
    assert_eq!(q(&|q| q.selector = Some("0xa415bcad".into())).len(), 15);
    assert_eq!(q(&|q| q.to = Some(format!("0x{:040x}", 0x20))).len(), 12);
    assert_eq!(q(&|q| q.rule = Some("r1".into())).len(), 30);
    assert_eq!(q(&|q| q.tag = Some("t".into())).len(), 15);
    assert_eq!(q(&|q| q.alert_only = true).len(), 9);
    assert_eq!(
        q(&|q| {
            q.since_ms = Some(1_700_000_000_010);
            q.until_ms = Some(1_700_000_000_019)
        })
        .len(),
        10
    );
    assert!(
        Reader::open(&db.0)
            .unwrap()
            .query(&Query {
                selector: Some("nonsense".into()),
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn free_form_sql_reads_but_never_writes() {
    let db = Db::new();
    write(&db, &(0..30).map(row).collect::<Vec<_>>());
    let r = Reader::open(&db.0).unwrap();
    let res = r
        .sql(
            "SELECT kind, count(*) AS n FROM rows GROUP BY kind ORDER BY kind",
            100,
        )
        .unwrap();
    assert_eq!(res.columns, vec!["kind", "n"]);
    assert_eq!(res.rows.len(), 3);
    assert_eq!(res.rows[0][1], 10);
    let v = r
        .sql("SELECT to_addr, selector, hash FROM v_rows LIMIT 1", 10)
        .unwrap();
    assert!(
        v.rows[0][0].as_str().unwrap().starts_with("0x")
            && v.rows[0][1].as_str().unwrap().starts_with("0x")
    );
    let cap = r.sql("SELECT id FROM rows", 5).unwrap();
    assert!((cap.rows.len(), cap.truncated) == (5, true));
    for bad in [
        "DELETE FROM rows",
        "DROP TABLE rows",
        "INSERT INTO rows(t_ms) VALUES (1)",
        "PRAGMA user_version = 99",
    ] {
        let _ = r.sql(bad, 10); // may error or do nothing, but must not change data
    }
    assert_eq!(r.count().unwrap(), 30, "the data is untouched");
}

#[test]
fn the_feed_side_never_waits_and_every_row_is_accounted_for() {
    let db = Db::new();
    let w = Writer::spawn(
        &db.0,
        WriterConfig {
            queue: 4,
            batch: 2,
            flush: Duration::from_millis(5),
        },
    )
    .unwrap();
    let ing = w.ingest();
    let sent = 50_000u64;
    let t = Instant::now();
    let accepted = (0..sent).filter(|i| ing.send(row(*i))).count() as u64;
    let took = t.elapsed();
    drop(ing);
    let s = w.shutdown();
    assert!(
        took < Duration::from_secs(5),
        "sending took {took:?}: it must not wait for the disk"
    );
    assert!(
        s.dropped > 0,
        "a queue of 4 cannot keep up with a tight loop: {s:?}"
    );
    assert_eq!(s.written + s.dropped + s.failed, sent, "{s:?}");
    assert_eq!(s.written, accepted);
    assert_eq!(
        Reader::open(&db.0).unwrap().count().unwrap(),
        accepted,
        "what was accepted is what is stored"
    );
}

#[test]
fn rows_survive_closing_and_reopening_and_writes_append() {
    let db = Db::new();
    write(&db, &(0..10).map(row).collect::<Vec<_>>());
    write(&db, &(10..25).map(row).collect::<Vec<_>>());
    assert_eq!(Reader::open(&db.0).unwrap().count().unwrap(), 25);
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let db = Db::new();
    write(&db, &[row(1)]);
    {
        let c = rusqlite::Connection::open(&db.0).unwrap();
        c.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
            .unwrap();
    }
    match Writer::spawn(&db.0, WriterConfig::default()) {
        Ok(_) => panic!("opened a newer schema"),
        Err(e) => assert!(e.to_string().contains("newer than this build"), "{e}"),
    }
}

#[test]
fn pruning_removes_old_rows_only() {
    let db = Db::new();
    write(&db, &(0..100).map(row).collect::<Vec<_>>());
    let n = prune(&db.0, 1_700_000_000_040).unwrap();
    assert_eq!(n, 40);
    let r = Reader::open(&db.0).unwrap();
    assert_eq!(r.count().unwrap(), 60);
    assert!(
        r.query(&Query {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap()
        .iter()
        .all(|x| x.seq >= 40)
    );
    assert!(disk_bytes(&db.0) > 0);
}

#[test]
fn readers_work_while_the_writer_writes() {
    let db = Db::new();
    let w = Writer::spawn(
        &db.0,
        WriterConfig {
            flush: Duration::from_millis(5),
            ..Default::default()
        },
    )
    .unwrap();
    let ing = w.ingest();
    assert!(ing.send(row(0)));
    std::thread::sleep(Duration::from_millis(50));
    let path = db.0.clone();
    let reader = std::thread::spawn(move || {
        let r = Reader::open(&path).unwrap();
        let mut last = 0;
        for _ in 0..200 {
            let c = r.count().unwrap();
            assert!(c >= last, "the count never goes backwards");
            last = c;
            r.query(&Query {
                kind: Some("swap".into()),
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        }
    });
    for i in 1..5_000 {
        ing.send(row(i));
    }
    reader.join().unwrap();
    drop(ing);
    let s = w.shutdown();
    assert_eq!(s.failed, 0, "{s:?}");
}

#[test]
fn the_engine_feeds_the_store_end_to_end() {
    use alloy::primitives::{Address, Bytes, U256};
    use seqscope_core::{ActionKind, Engine, Mode, Profile, Rule, RuleSet, Tx};
    let set = RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "gaps".into(),
            enabled: true,
            mode: Mode::All,
            rule: vec![Rule {
                name: "unknown".into(),
                enabled: true,
                when: "kind == unknown && len >= 4".into(),
                action: ActionKind::Store,
                tag: Some("gap".into()),
                sample: 1,
            }],
        }],
    };
    let engine = Engine::compile(&set).unwrap();
    let tx = |h: u8, input: Vec<u8>| Tx {
        hash: B256::repeat_byte(h),
        seq: u64::from(h),
        t_ms: 1_700_000_000_000 + u64::from(h),
        from: Some(Address::with_last_byte(1)),
        to: Some(Address::with_last_byte(0x77)),
        tx_type: 2,
        value: U256::ZERO,
        gas_limit: 1,
        max_fee: 1,
        tip: None,
        nonce: 0,
        input: Bytes::from(input),
    };
    let db = Db::new();
    let w = Writer::spawn(&db.0, WriterConfig::default()).unwrap();
    let ing = w.ingest();
    for h in 1..=20u8 {
        for r in engine.process(&tx(
            h,
            if h % 2 == 0 {
                vec![0xaa, 0xbb, 0xcc, 0xdd, 1]
            } else {
                vec![]
            },
        )) {
            ing.send(r);
        }
    }
    drop(ing);
    assert_eq!(w.shutdown().written, 10);
    let r = Reader::open(&db.0).unwrap();
    let rows = r
        .query(&Query {
            tag: Some("gap".into()),
            selector: Some("0xaabbccdd".into()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 10);
    assert!(
        rows.iter()
            .all(|x| x.kind == "unknown" && x.status == "pending" && x.profile == "gaps")
    );
}

#[test]
fn shutdown_does_not_wait_for_other_holders_of_the_sender_and_still_accounts_for_every_row() {
    let db = Db::new();
    let w = Writer::spawn(
        &db.0,
        WriterConfig {
            flush: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    let held_elsewhere = w.ingest(); // like the server's shared state: it is never dropped before shutdown
    let sent = 3_000u64;
    let accepted = (0..sent).filter(|i| held_elsewhere.send(row(*i))).count() as u64;
    // Run the shutdown on another thread so a regression fails the test instead of hanging it.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(w.shutdown());
    });
    let s = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("shutdown waited for another holder of the sender");
    assert_eq!(s.written, accepted, "{s:?}");
    // After the stop, nothing more is accepted and every refusal is counted.
    assert!(!held_elsewhere.send(row(1)));
    assert_eq!(
        held_elsewhere.stats().snapshot().dropped,
        sent - accepted + 1
    );
    assert_eq!(Reader::open(&db.0).unwrap().count().unwrap(), accepted);
}
