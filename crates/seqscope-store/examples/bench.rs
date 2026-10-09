//! Storage comparison on synthetic rows shaped like the engine's: `cargo run --release -p seqscope-store --example bench`.
//! Engines: SQLite (WAL, indexed), redb (key-value with two manual secondary indexes), and an append-only JSONL log
//! (the zero-dependency baseline; queries are scans). Measures batched ingest, then four typical queries.

use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    time::Instant,
};

use rusqlite::{Connection, params};

const N: usize = 1_000_000;
const BATCH: usize = 1_000;

#[derive(Clone)]
struct R {
    t_ms: u64,
    seq: u64,
    hash: [u8; 32],
    to: [u8; 20],
    sel: u32,
    protocol: &'static str,
    kind: &'static str,
    fields: String,
}

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn synth(n: usize) -> Vec<R> {
    let protos = [
        "uniswap_v3",
        "uniswap_v4",
        "aave_v3",
        "uniswap_router",
        "pancake_v3",
        "morpho_blue",
        "unidentified",
    ];
    let kinds = [
        "swap",
        "swap",
        "swap",
        "liquidity",
        "undecoded_known",
        "unknown",
    ];
    let mut s = 0x9E3779B97F4A7C15u64;
    (0..n)
        .map(|i| {
            let r = rng(&mut s);
            let mut to = [0u8; 20];
            to[..8].copy_from_slice(&(r % 300).to_be_bytes()); // ~300 distinct destinations
            let mut hash = [0u8; 32];
            hash[..8].copy_from_slice(&rng(&mut s).to_be_bytes());
            hash[8..16].copy_from_slice(&(i as u64).to_be_bytes());
            R {
                t_ms: 1_700_000_000_000 + (i as u64) * 2, // 500 rows/s of feed time
                seq: i as u64,
                hash,
                to,
                sel: 0xa000_0000 + (rng(&mut s) % 60) as u32,
                protocol: protos[(r >> 20) as usize % protos.len()],
                kind: kinds[(r >> 30) as usize % kinds.len()],
                fields: format!(
                    "{{\"swap.pool\":\"0x{:040x}\",\"swap.token_in\":\"0x{:040x}\",\"swap.amount_in\":\"{}\",\"swap.hops\":\"1\"}}",
                    r % 5000,
                    (r >> 8) % 400,
                    rng(&mut s) % 1_000_000_000_000
                ),
            }
        })
        .collect()
}

fn line(r: &R) -> String {
    format!(
        "{{\"t_ms\":{},\"seq\":{},\"hash\":\"0x{}\",\"to\":\"0x{}\",\"selector\":\"0x{:08x}\",\"protocol\":\"{}\",\"kind\":\"{}\",\"status\":\"pending\",\"fields\":{}}}\n",
        r.t_ms,
        r.seq,
        alloy::hex::encode(r.hash),
        alloy::hex::encode(r.to),
        r.sel,
        r.protocol,
        r.kind,
        r.fields
    )
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed().as_secs_f64() * 1000.0)
}

fn report(name: &str, ingest_s: f64, size: u64, q: [f64; 4]) {
    println!(
        "{name:8} ingest {:>9.0} rows/s  size {:>5.0} MB | tail100 {:>7.2} ms  by-selector(1h) {:>8.2} ms  by-dest count {:>8.2} ms  proto+kind count {:>8.2} ms",
        N as f64 / ingest_s,
        size as f64 / 1e6,
        q[0],
        q[1],
        q[2],
        q[3]
    );
}

fn sqlite(rows: &[R], dir: &str) {
    let path = format!("{dir}/b.sqlite");
    let _ = fs::remove_file(&path);
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
         CREATE TABLE rows(id INTEGER PRIMARY KEY, t_ms INTEGER NOT NULL, seq INTEGER NOT NULL, hash BLOB, to_addr BLOB, selector INTEGER, protocol TEXT, kind TEXT, fields TEXT);
         CREATE INDEX r_t ON rows(t_ms); CREATE INDEX r_sel ON rows(selector, t_ms); CREATE INDEX r_to ON rows(to_addr, t_ms); CREATE INDEX r_pk ON rows(protocol, kind, t_ms);",
    )
    .unwrap();
    let (_, ms) = timed(|| {
        for chunk in rows.chunks(BATCH) {
            let tx = c.unchecked_transaction().unwrap();
            {
                let mut st = tx.prepare_cached("INSERT INTO rows(t_ms,seq,hash,to_addr,selector,protocol,kind,fields) VALUES (?,?,?,?,?,?,?,?)").unwrap();
                for r in chunk {
                    st.execute(params![
                        r.t_ms as i64,
                        r.seq as i64,
                        &r.hash[..],
                        &r.to[..],
                        r.sel as i64,
                        r.protocol,
                        r.kind,
                        r.fields
                    ])
                    .unwrap();
                }
            }
            tx.commit().unwrap();
        }
    });
    let last_t = rows.last().unwrap().t_ms as i64;
    let to0 = rows[0].to;
    let (_, q0) = timed(|| {
        c.prepare("SELECT * FROM rows ORDER BY t_ms DESC LIMIT 100")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .count()
    });
    let (_, q1) = timed(|| {
        c.prepare("SELECT * FROM rows WHERE selector=?1 AND t_ms>?2")
            .unwrap()
            .query_map(params![0xa000_0007i64, last_t - 3_600_000], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
            .count()
    });
    let (_, q2) = timed(|| {
        c.query_row(
            "SELECT count(*) FROM rows WHERE to_addr=?1",
            params![&to0[..]],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    });
    let (_, q3) = timed(|| {
        c.query_row(
            "SELECT count(*) FROM rows WHERE protocol='uniswap_v3' AND kind='swap'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    });
    let size = fs::metadata(&path).unwrap().len()
        + fs::metadata(format!("{path}-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
    report("sqlite", ms / 1000.0, size, [q0, q1, q2, q3]);
}

fn redb_bench(rows: &[R], dir: &str) {
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
    const MAIN: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("rows");
    const BY_SEL: TableDefinition<(u32, u64, u64), ()> = TableDefinition::new("by_sel");
    const BY_TO: TableDefinition<([u8; 20], u64, u64), ()> = TableDefinition::new("by_to");
    let path = format!("{dir}/b.redb");
    let _ = fs::remove_file(&path);
    let db = Database::create(&path).unwrap();
    let (_, ms) = timed(|| {
        for chunk in rows.chunks(BATCH) {
            let w = db.begin_write().unwrap();
            {
                let mut m = w.open_table(MAIN).unwrap();
                let mut a = w.open_table(BY_SEL).unwrap();
                let mut b = w.open_table(BY_TO).unwrap();
                for r in chunk {
                    let v = line(r);
                    m.insert((r.t_ms, r.seq), v.as_bytes()).unwrap();
                    a.insert((r.sel, r.t_ms, r.seq), ()).unwrap();
                    b.insert((r.to, r.t_ms, r.seq), ()).unwrap();
                }
            }
            w.commit().unwrap();
        }
    });
    let last_t = rows.last().unwrap().t_ms;
    let to0 = rows[0].to;
    let rd = db.begin_read().unwrap();
    let m = rd.open_table(MAIN).unwrap();
    let a = rd.open_table(BY_SEL).unwrap();
    let b = rd.open_table(BY_TO).unwrap();
    let (_, q0) = timed(|| m.iter().unwrap().rev().take(100).count());
    let (_, q1) = timed(|| {
        a.range((0xa000_0007u32, last_t - 3_600_000, 0)..=(0xa000_0007u32, u64::MAX, u64::MAX))
            .unwrap()
            .filter_map(|e| {
                let k = e.unwrap().0.value();
                m.get((k.1, k.2)).unwrap().map(|v| v.value().len())
            })
            .count()
    });
    let (_, q2) = timed(|| {
        b.range((to0, 0, 0)..=(to0, u64::MAX, u64::MAX))
            .unwrap()
            .count()
    });
    // proto+kind has no index in this layout: a full scan of the main table, parsing a field of each value.
    let (_, q3) = timed(|| {
        m.iter()
            .unwrap()
            .filter(|e| {
                let v = e.as_ref().unwrap().1.value();
                let s = std::str::from_utf8(v).unwrap();
                s.contains("\"uniswap_v3\"") && s.contains("\"swap\"")
            })
            .count()
    });
    report(
        "redb",
        ms / 1000.0,
        fs::metadata(&path).unwrap().len(),
        [q0, q1, q2, q3],
    );
}

fn log_bench(rows: &[R], dir: &str) {
    let path = format!("{dir}/b.jsonl");
    let _ = fs::remove_file(&path);
    let (_, ms) = timed(|| {
        let mut w = BufWriter::new(File::create(&path).unwrap());
        for chunk in rows.chunks(BATCH) {
            for r in chunk {
                w.write_all(line(r).as_bytes()).unwrap();
            }
            w.flush().unwrap();
            w.get_ref().sync_data().unwrap();
        }
    });
    let text = fs::read_to_string(&path).unwrap();
    let to0 = format!("0x{}", alloy::hex::encode(rows[0].to));
    let last_t = rows.last().unwrap().t_ms;
    let (_, q0) = timed(|| text.lines().rev().take(100).count());
    let (_, q1) = timed(|| {
        text.lines()
            .rev()
            .take_while(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["t_ms"].as_u64().unwrap() > last_t - 3_600_000
            })
            .filter(|l| l.contains("\"0xa0000007\""))
            .count()
    });
    let (_, q2) = timed(|| {
        text.lines()
            .filter(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["to"] == to0.as_str())
            .count()
    });
    let (_, q3) = timed(|| {
        text.lines()
            .filter(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["protocol"] == "uniswap_v3" && v["kind"] == "swap"
            })
            .count()
    });
    report(
        "jsonl",
        ms / 1000.0,
        fs::metadata(&path).unwrap().len(),
        [q0, q1, q2, q3],
    );
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp".into());
    println!("{N} rows, batch {BATCH}, dir {dir}");
    let rows = synth(N);
    sqlite(&rows, &dir);
    redb_bench(&rows, &dir);
    log_bench(&rows, &dir);
}
