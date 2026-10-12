//! Isolated audit reproductions. No network, keys, signing, or production state.
use mev_risk::{RiskGuard, RiskLimits};
use std::path::PathBuf;

#[path = "../../../../../../crates/mev-exec/src/nonce.rs"]
#[allow(dead_code)]
mod nonce;

fn limits() -> RiskLimits {
    RiskLimits {
        min_profit_usd: 0.05,
        max_gas_usd_per_day: 3.0,
        max_arb_gas_usd_per_day: 3.0,
        max_consecutive_failures: 10,
    }
}

fn guard(dir: &std::path::Path) -> RiskGuard {
    RiskGuard::new(limits(), dir.join("STOP")).with_max_in_flight(3)
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("expression-depth") {
        // Run separately: stack overflow aborts the process. The text is only 40 KB.
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let text = "not ".repeat(10_000) + "true";
                let _ = seqscope_core::expr::parse(&text);
            })
            .unwrap()
            .join()
            .unwrap();
        return;
    }
    let dir = PathBuf::from(format!("/tmp/mev-audit-repro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let now = 20_000 * 86_400 + 100;

    let mut g = guard(&dir);
    g.record(false, 1.8, now);
    for _ in 0..3 {
        assert!(g.check_arb(1.0, 0.7, now).is_ok());
        g.mark_sent();
    }
    for _ in 0..3 {
        g.record_lost_race(0.7, now);
    }
    assert!(g.spent_today_usd() > 3.0);
    println!(
        "pending budget: 3 sends accepted; spend {:.2} exceeds 3.00 cap",
        g.spent_today_usd()
    );

    let mut g = guard(&dir);
    for _ in 0..100 {
        assert!(g.check_concurrent(1.0, 0.1, now).is_ok());
        g.mark_sent();
    }
    println!("backruns: 100 accepted without receipts, despite max_in_flight=3");

    let path = dir.join("risk.json");
    let mut g = guard(&dir).with_persistence(path.clone(), now);
    g.record(false, 1.8, now);
    g.mark_sent(); // A broadcast survives the process that submitted it.
    drop(g);
    let mut resumed = guard(&dir).with_persistence(path.clone(), now);
    assert!(resumed.check(1.0, 1.0, now).is_ok());
    println!("restart: outstanding send not restored or reserved; another send admitted");

    std::fs::write(&path, b"invalid JSON").unwrap();
    let mut resumed = guard(&dir).with_persistence(path, now);
    assert_eq!(resumed.spent_today_usd(), 0.0);
    assert!(resumed.check(1.0, 3.0, now).is_ok());
    println!("corrupt persistence: spend reset to 0; entire daily budget admitted");

    let mut g = guard(&dir);
    assert!(g.check(f64::NAN, f64::NAN, now).is_ok());
    println!("non-finite inputs: NaN profit and gas admitted by risk guard");

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../../");
    let config = std::fs::read_to_string(root.join("config/arbitrum.toml")).unwrap();
    let config = config.replace("min_profit_usd = 0.05", "min_profit_usd = nan");
    let cfg_path = dir.join("invalid-config.toml");
    std::fs::write(&cfg_path, config).unwrap();
    assert!(
        mev_config::ChainConfig::load(&cfg_path)
            .unwrap()
            .risk
            .min_profit_usd
            .is_nan()
    );
    println!("config: NaN min_profit_usd accepted by ChainConfig::load");

    let m = nonce::NonceManager::new(7);
    let held = m.take();
    m.reset(7); // An RPC read races a transaction not yet reflected at the endpoint.
    assert_eq!(held, m.take());
    println!("nonce reset: nonce 7 allocated twice while first reservation outstanding");

    let sample = "https://example-user:example-password@rpc.example/v2/example_api_key_123456";
    let redacted = mev_types::redact_secrets(sample);
    assert!(redacted.contains("example-password"));
    assert!(!redacted.contains("example_api_key_123456"));
    println!("redaction: synthetic URL path key masked, synthetic userinfo password retained");
}
