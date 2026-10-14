//! Process isolation (spec invariant 7): Process B has no signing keys, no wallet or RPC
//! privileges and no executor channel. This test inspects the real resolved dependency graph of
//! the production build (`cargo tree -p`, normal + build edges; dev-dependencies are excluded) and fails if any
//! execution, risk, engine, RPC or signer crate (or an alloy signer feature) is reachable.

use std::collections::BTreeSet;
use std::process::Command;

use clap::CommandFactory;

const ROOT: &str = "copper-jev-advisor";

/// Crates that must never be reachable from the advisor binary.
const FORBIDDEN_CRATES: &[&str] = &[
    "mev-exec",
    "mev-risk",
    "mev-engine",
    "mev-bot",
    "mev-rpc",
    "mev-sequencer",
    "mev-liquidate",
    "mev-liq-sched",
    "mev-search",
    "mev-sim",
    "mev-state",
    "mev-oracle",
    "mev-lending",
    "mev-config",
    "alloy-signer",
    "alloy-signer-local",
    "alloy-signer-aws",
    "alloy-signer-gcp",
    "alloy-signer-ledger",
    "alloy-signer-trezor",
    "alloy-signer-keystore",
    "alloy-provider",
    "alloy-rpc-client",
    "alloy-transport-ws",
    "alloy-transport-http",
    "k256",
    "coins-bip32",
    "coins-bip39",
];

/// `(crate name, enabled features)` for every crate reachable through normal/build edges of
/// `package`, resolved for that package alone exactly as `cargo build -p` would (so a feature
/// enabled only by some other workspace member does not count, and dev-dependencies are excluded).
fn tree(package: &str) -> Vec<(String, BTreeSet<String>)> {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            package,
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--locked",
            "--offline",
            "-f",
            "{p}|{f}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let (p, f) = l.split_once('|')?;
            let name = p.split_whitespace().next()?.to_string();
            let feats = f
                .trim_end_matches(" (*)")
                .split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect();
            Some((name, feats))
        })
        .collect()
}

fn names(t: &[(String, BTreeSet<String>)]) -> BTreeSet<&str> {
    t.iter().map(|(n, _)| n.as_str()).collect()
}

#[test]
fn no_execution_risk_engine_rpc_or_signer_crate_is_reachable() {
    let t = tree(ROOT);
    let reachable = names(&t);
    assert!(reachable.contains("jev-advisor"), "graph walk is broken");
    assert!(reachable.contains("mev-liq-state"), "graph walk is broken");
    let bad: Vec<_> = FORBIDDEN_CRATES
        .iter()
        .filter(|c| reachable.contains(**c))
        .collect();
    assert!(
        bad.is_empty(),
        "copper-jev-advisor must not depend on {bad:?} (spec invariant 7)"
    );
}

#[test]
fn no_alloy_signer_feature_is_enabled_in_the_production_build() {
    let t = tree(ROOT);
    let mut alloy_seen = false;
    for (name, feats) in &t {
        if name.starts_with("alloy") {
            alloy_seen = true;
            for f in feats {
                assert!(
                    !f.contains("signer") && !f.contains("wallet") && !f.contains("keystore"),
                    "{name} has feature `{f}` enabled in the advisor build"
                );
            }
        }
    }
    assert!(alloy_seen, "alloy primitives should be present");
}

/// Control: the detector must actually see forbidden crates where they exist (the live bot).
#[test]
fn control_the_detector_flags_the_live_bot() {
    let t = tree("mev-bot");
    let reachable = names(&t);
    assert!(
        reachable.contains("mev-exec"),
        "control failed: mev-bot should reach mev-exec"
    );
    assert!(
        t.iter()
            .any(|(n, f)| n == "alloy" && f.contains("signer-local")),
        "control failed: the workspace alloy enables signer-local"
    );
}

#[test]
fn configuration_surface_has_no_rpc_key_wallet_executor_or_database_url() {
    let cmd = copper_jev_advisor_cmd();
    let mut envs: Vec<String> = Vec::new();
    for a in cmd.get_arguments() {
        if let Some(e) = a.get_env() {
            envs.push(e.to_string_lossy().to_string());
        }
        envs.push(a.get_id().to_string());
    }
    assert!(envs.iter().any(|e| e == "TYPESAFE_API_KEY_FILE"));
    for e in &envs {
        let u = e.to_ascii_uppercase();
        for banned in [
            "RPC",
            "PRIVATE",
            "WALLET",
            "SIGNER",
            "EXECUTOR",
            "DATABASE_URL",
            "MNEMONIC",
            "NONCE",
        ] {
            assert!(
                !u.contains(banned),
                "config setting `{e}` is not allowed on Process B"
            );
        }
        if u.contains("KEY") {
            assert!(
                u.starts_with("TYPESAFE_API_KEY") || u.contains("TYPESAFE_API_KEY"),
                "unexpected key-like setting `{e}`"
            );
        }
    }
}

fn copper_jev_advisor_cmd() -> clap::Command {
    #[derive(clap::Parser)]
    struct Probe {
        #[command(flatten)]
        args: copper_jev_advisor::RawArgs,
    }
    Probe::command()
}
