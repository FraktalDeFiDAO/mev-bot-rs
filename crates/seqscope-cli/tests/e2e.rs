//! The real binary, run against the recorded corpus: every offline command, exit codes, and the files it writes.
//! (The live feed is covered by `scripts/test-seqscope.sh --live`.)

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU32, Ordering},
};

static N: AtomicU32 = AtomicU32::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir().join(format!(
            "seqscope-e2e-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../seqscope-core/tests/fixtures/corpus.jsonl")
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_seqscope"))
        .args(args)
        .output()
        .expect("run seqscope")
}

fn ok(args: &[&str]) -> String {
    let o = run(args);
    assert!(
        o.status.success(),
        "seqscope {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn fails(args: &[&str]) -> String {
    let o = run(args);
    assert!(!o.status.success(), "seqscope {args:?} should have failed");
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn every_template_checks_and_runs_over_the_corpus() {
    let d = Dir::new();
    let list = ok(&["rules", "template"]);
    let names: Vec<&str> = list
        .lines()
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(names.len() >= 6, "{list}");
    for n in names {
        let f = d.path(&format!("{n}.toml"));
        std::fs::write(&f, ok(&["rules", "template", n])).unwrap();
        let out = ok(&["rules", "check", s(&f)]);
        assert!(out.trim_end().ends_with("ok"), "{n}: {out}");
        let db = d.path(&format!("{n}.db"));
        let o = run(&["replay", s(&corpus()), "-r", s(&f), "--db", s(&db)]);
        assert!(
            o.status.success(),
            "{n}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("transactions ->"),
            "{n}"
        );
    }
}

#[test]
fn replay_into_a_database_then_query_stats_gaps_and_sql_agree() {
    let d = Dir::new();
    let rules = d.path("r.toml");
    std::fs::write(
        &rules,
        format!(
            "{}{}",
            ok(&["rules", "template", "dex-swaps"]),
            ok(&["rules", "template", "gaps"]).replacen("version = 1\n", "", 1)
        ),
    )
    .unwrap();
    let db = d.path("x.db");
    let err = run(&["replay", s(&corpus()), "-r", s(&rules), "--db", s(&db)]);
    let msg = String::from_utf8_lossy(&err.stderr).into_owned();
    assert!(err.status.success(), "{msg}");
    let stored: u64 = msg
        .split("stored ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{msg}"));
    assert!(stored > 50);

    // The same count from three directions.
    let sql = ok(&[
        "sql",
        "SELECT count(*) AS n FROM rows",
        "--db",
        s(&db),
        "--json",
    ]);
    let v: serde_json::Value = serde_json::from_str(&sql).unwrap();
    assert_eq!(v["rows"][0][0], stored);
    let q = run(&["query", "--db", s(&db), "--limit", "100000", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&q.stdout).lines().count() as u64,
        stored
    );
    let stats = ok(&["stats", "--db", s(&db)]);
    assert!(stats.contains(&format!("{stored} rows")), "{stats}");

    // Filters narrow, never widen.
    let swaps = run(&[
        "query",
        "--db",
        s(&db),
        "--kind",
        "swap",
        "--limit",
        "100000",
        "--json",
    ]);
    let n_swaps = String::from_utf8_lossy(&swaps.stdout).lines().count() as u64;
    assert!(n_swaps > 0 && n_swaps < stored);
    for line in String::from_utf8_lossy(&swaps.stdout).lines() {
        let r: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(r["kind"], "swap");
        assert_eq!(r["status"], "pending");
    }
    let gaps = ok(&["gaps", "--db", s(&db)]);
    assert!(
        gaps.contains("calls") && gaps.contains("destination"),
        "{gaps}"
    );
}

#[test]
fn rules_test_reports_counts_and_examples_without_a_database() {
    let out = ok(&[
        "rules",
        "test",
        "-f",
        s(&corpus()),
        "-w",
        "kind == swap",
        "--samples",
        "2",
    ]);
    assert!(out.contains("2049 transactions"), "{out}");
    let matched: u64 = out
        .split(" match")
        .next()
        .and_then(|h| h.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{out}"));
    assert!(matched > 50, "{out}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("  ")).count(),
        2,
        "two examples: {out}"
    );
    let none = ok(&[
        "rules",
        "test",
        "-f",
        s(&corpus()),
        "-w",
        "kind == swap && swap.amount_in > 1e60",
    ]);
    assert!(none.contains(": 0 match"), "{none}");
}

#[test]
fn mistakes_exit_nonzero_with_a_message_that_names_the_problem() {
    let d = Dir::new();
    let bad = d.path("bad.toml");
    std::fs::write(&bad, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"r\"\nwhen = \"swap.amount > 5 && selector == 0x12\"\n").unwrap();
    let e = fails(&["rules", "check", s(&bad)]);
    assert!(e.contains("did you mean swap.amount_in"), "{e}");
    assert!(
        fails(&["rules", "test", "-f", s(&corpus()), "-w", "kind =="]).contains("expected a value")
    );
    assert!(fails(&["rules", "template", "nope"]).contains("no template"));
    assert!(fails(&["decode", "0xnothex", "0x00"]).contains("bad address"));
    assert!(
        fails(&[
            "decode",
            "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45",
            "0xzz"
        ])
        .contains("bad hex")
    );
    assert!(fails(&["query", "--db", s(&d.path("missing.db"))]).contains("does not exist"));
    let typo = d.path("typo.toml");
    std::fs::write(&typo, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"r\"\nwhen = \"true\"\nacton = \"drop\"\n").unwrap();
    assert!(
        fails(&["rules", "check", s(&typo)])
            .to_lowercase()
            .contains("acton")
    );
    assert!(fails(&["replay", s(&d.path("nofile.jsonl"))]).contains("opening"));
}

#[test]
fn decode_shows_every_call_inside_a_wrapper() {
    // Multicall3.aggregate3([(0x...01, true, 0xdeadbeef), (0x...02, true, 0xcafebabe)])
    let word = |n: u64| format!("{n:064x}");
    let addr = |a: u8| format!("{:064x}", a);
    let call = |t: u8, d: &str| {
        format!(
            "{}{}{}{}{}{}",
            addr(t),
            word(1),
            word(0x60),
            word(4),
            d,
            "0".repeat(56)
        )
    };
    let c1 = call(1, "deadbeef");
    let c2 = call(2, "cafebabe");
    let one = c1.len() as u64 / 2;
    let data = format!(
        "{}{}{}{}{}{}",
        word(0x20),
        word(2),
        word(0x40),
        word(0x40 + one),
        c1,
        c2
    );
    let input = format!("0x82ad56cb{data}");
    let out = ok(&[
        "decode",
        "0xcA11bde05977b3631167028862bE2a173976CA11",
        &input,
    ]);
    assert!(out.starts_with("2 call(s)"), "{out}");
    assert!(
        out.contains("sel 0xdeadbeef")
            && out.contains("sel 0xcafebabe")
            && out.contains("multicall3"),
        "{out}"
    );
}

#[test]
fn catalogue_lists_fields_protocols_and_machine_readable_json() {
    let f = ok(&["catalogue"]);
    for needle in [
        "swap.amount_in",
        "liq.tick_lower",
        "liquidation.borrower",
        "kind",
        "via",
        "OPERATORS",
    ] {
        assert!(f.contains(needle), "{needle}");
    }
    let p = ok(&["catalogue", "protocols"]);
    assert!(p.contains("aave_v3") && p.contains("uniswap_v4"));
    let j: serde_json::Value = serde_json::from_str(&ok(&["catalogue", "--json"])).unwrap();
    assert!(j["fields"].as_array().unwrap().len() > 30);
    assert!(
        j["wrappers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "entrypoint_v6")
    );
}

#[test]
fn a_closed_pipe_is_not_an_error() {
    // `seqscope catalogue all | head -1` must not print a panic.
    let o = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} catalogue all 2>&1 | head -1",
            env!("CARGO_BIN_EXE_seqscope")
        ))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        !text.contains("panicked") && !text.contains("Broken pipe"),
        "{text}"
    );
}

#[test]
fn replay_to_the_screen_is_deterministic() {
    let a = ok(&["replay", s(&corpus()), "-w", "kind != unknown", "--json"]);
    let b = ok(&["replay", s(&corpus()), "-w", "kind != unknown", "--json"]);
    assert_eq!(a, b);
    assert!(a.lines().count() > 50);
}

// ---------------------------------------------------------------------------------------------------------------
// The collector against a local fake sequencer feed: parsing, rules, storage and a graceful stop, all offline.

mod fake_feed {
    use alloy::{
        consensus::{SignableTransaction, TxEip1559, TxEnvelope},
        eips::eip2718::Encodable2718,
        network::TxSignerSync,
        primitives::{Address, TxKind, U256},
        signers::local::PrivateKeySigner,
        sol,
        sol_types::SolCall,
    };
    use base64::Engine as _;
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    sol! {
        struct P { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
        function exactInputSingle(P params) external payable returns (uint256 amountOut);
    }

    pub fn swap_input(amount: u64) -> Vec<u8> {
        exactInputSingleCall {
            params: P {
                tokenIn: Address::with_last_byte(0xa1),
                tokenOut: Address::with_last_byte(0xb2),
                fee: alloy::primitives::Uint::<24, 1>::from(500u32),
                recipient: Address::with_last_byte(1),
                amountIn: U256::from(amount),
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
            },
        }
        .abi_encode()
    }

    /// One broadcast frame carrying one signed transaction.
    pub fn frame(seq: u64, to: Address, input: Vec<u8>) -> String {
        let signer = PrivateKeySigner::random();
        let mut t = TxEip1559 {
            chain_id: 42161,
            nonce: seq,
            to: TxKind::Call(to),
            input: input.into(),
            gas_limit: 300_000,
            max_fee_per_gas: 10_000_000,
            ..Default::default()
        };
        let sig = signer.sign_transaction_sync(&mut t).unwrap();
        let env = TxEnvelope::Eip1559(t.into_signed(sig));
        let mut l2 = vec![4u8]; // a signed transaction
        l2.extend(env.encoded_2718());
        serde_json::json!({
            "version": 1,
            "messages": [{ "sequenceNumber": seq, "message": { "message": {
                "header": { "kind": 3 },
                "l2Msg": base64::engine::general_purpose::STANDARD.encode(l2)
            } } }]
        })
        .to_string()
    }

    /// Serve `frames` (one every 15 ms) to whoever connects, then hold the connection open. Returns the port.
    pub fn serve(frames: Vec<String>) -> u16 {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            rt.block_on(async move {
                loop {
                    let Ok((sock, _)) = listener.accept().await else {
                        return;
                    };
                    let frames = frames.clone();
                    tokio::spawn(async move {
                        let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                            return;
                        };
                        for f in frames {
                            if ws.send(Message::text(f)).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                        }
                        std::future::pending::<()>().await;
                    });
                }
            });
        });
        port
    }
}

#[test]
fn the_collector_runs_against_a_local_feed_and_flushes_everything_on_sigterm() {
    use alloy::primitives::Address;
    let router = mev_protocol_uniswap_router::SWAP_ROUTER_02;
    let frames: Vec<String> = (0..40u64)
        .map(|i| {
            if i % 4 == 0 {
                fake_feed::frame(1000 + i, router, fake_feed::swap_input(1_000 + i))
            } else {
                fake_feed::frame(
                    1000 + i,
                    Address::with_last_byte(0x44),
                    vec![0xa9, 0x05, 0x9c, 0xbb, i as u8],
                )
            }
        })
        .collect();
    let port = fake_feed::serve(frames);

    let d = Dir::new();
    let rules = d.path("r.toml");
    std::fs::write(&rules, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"all\"\nwhen = \"true\"\n").unwrap();
    let db = d.path("c.db");
    let mut child = Command::new(env!("CARGO_BIN_EXE_seqscope"))
        .args([
            "run",
            "--url",
            &format!("ws://127.0.0.1:{port}"),
            "-r",
            s(&rules),
            "--db",
            s(&db),
            "--every",
            "1",
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3_500)); // 40 frames take ~0.6 s; room for slow machines
    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the collector did not stop on SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(status.success(), "exit {status:?}\n{err}");
    assert!(err.contains("stopping"), "{err}");
    assert!(
        err.contains("stored 40 rows (0 dropped, 0 failed)"),
        "{err}"
    );

    // Everything the feed sent is in the database, decoded: ten swaps among forty calls.
    let r = ok(&[
        "sql",
        "SELECT kind, count(*) FROM rows GROUP BY kind ORDER BY kind",
        "--db",
        s(&db),
        "--json",
    ]);
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    let get = |kind: &str| {
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row[0] == kind)
            .map(|row| row[1].as_u64().unwrap())
            .unwrap_or(0)
    };
    assert_eq!(get("swap"), 10, "{r}");
    assert_eq!(
        get("swap") + get("unknown") + get("undecoded_known"),
        40,
        "{r}"
    );
}
