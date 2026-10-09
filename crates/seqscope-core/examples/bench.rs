//! Rule-engine throughput: `cargo run --release -p seqscope-core --example bench`.
//! Measures per-transaction cost with 100 enabled rules (decoding included), and the rules alone.

use alloy::primitives::{Address, B256, Bytes, U256};
use seqscope_core::{ActionKind, Engine, Mode, Profile, Rule, RuleSet, Tx};
use std::time::Instant;

fn tx(h: u8, to: Address, input: Vec<u8>) -> Tx {
    Tx {
        hash: B256::repeat_byte(h),
        seq: h as u64,
        t_ms: 0,
        from: Some(Address::with_last_byte(1)),
        to: Some(to),
        tx_type: 2,
        value: U256::ZERO,
        gas_limit: 300_000,
        max_fee: 1,
        tip: Some(1),
        nonce: 1,
        input: Bytes::from(input),
    }
}

fn main() {
    // Realistic rules that mostly do not match: different protocols, selectors, thresholds.
    let protos = [
        "aave_v3",
        "morpho_blue",
        "compound_v3",
        "uniswap_v2",
        "uniswap_v3",
        "pancake_v3",
        "balancer",
        "fluid_vaults",
    ];
    let rules: Vec<Rule> = (0..100)
        .map(|i| Rule {
            name: format!("r{i}"),
            enabled: true,
            when: format!(
                "protocol == {} && selector in [0x{:08x}, 0x{:08x}] && swap.amount_in >= {}e6",
                protos[i % protos.len()],
                0xa415_bcad_u32.wrapping_add(i as u32),
                0x573a_de81_u32.wrapping_add(i as u32),
                i + 1
            ),
            action: ActionKind::Store,
            tag: None,
            sample: 1,
        })
        .collect();
    let set = RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "bench".into(),
            enabled: true,
            mode: Mode::All,
            rule: rules,
        }],
    };
    let engine = Engine::compile(&set).unwrap();

    // Mixed traffic: an unknown contract (the common case on the feed), a plain transfer, a swap-router call.
    let unknown = tx(
        1,
        Address::with_last_byte(0x44),
        vec![0xa9, 0x05, 0x9c, 0xbb, 0, 0, 0, 0],
    );
    let transfer = tx(2, Address::with_last_byte(0x45), vec![]);
    let ur_rows = include_str!("../../mev-protocol-uniswap-router/tests/fixtures/ur_v4_rows.csv");
    let f: Vec<&str> = ur_rows.lines().nth(1).unwrap().split(',').collect();
    let ur = tx(
        3,
        f[1].parse().unwrap(),
        alloy::hex::decode(f[2].trim_start_matches("0x")).unwrap(),
    );
    for (name, t) in [
        ("unknown contract", &unknown),
        ("plain transfer", &transfer),
        ("universal router v4", &ur),
    ] {
        let n = 200_000u32;
        let start = Instant::now();
        let mut rows = 0;
        for _ in 0..n {
            rows += std::hint::black_box(engine.process(std::hint::black_box(t))).len();
        }
        let per = start.elapsed().as_nanos() as f64 / f64::from(n);
        println!(
            "{name:22} {:8.2} us/tx with 100 rules (rows {rows})",
            per / 1000.0
        );
    }
    // The same traffic with one trivial rule: the cost without the rules.
    let one = RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "p".into(),
            enabled: true,
            mode: Mode::All,
            rule: vec![Rule {
                name: "x".into(),
                enabled: true,
                when: "kind == swap && swap.amount_in > 1e30".into(),
                action: ActionKind::Store,
                tag: None,
                sample: 1,
            }],
        }],
    };
    let e1 = Engine::compile(&one).unwrap();
    for (name, t) in [("unknown contract", &unknown), ("universal router v4", &ur)] {
        let n = 200_000u32;
        let start = Instant::now();
        for _ in 0..n {
            std::hint::black_box(e1.process(std::hint::black_box(t)));
        }
        println!(
            "{name:22} {:8.2} us/tx with 1 rule (decode + overhead)",
            start.elapsed().as_nanos() as f64 / f64::from(n) / 1000.0
        );
    }
}
