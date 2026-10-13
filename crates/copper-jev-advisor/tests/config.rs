//! Startup validation and the real binary's exit behaviour (config errors, readiness probe,
//! SIGTERM). The worker flow itself is covered in e2e.rs.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use copper_jev_advisor::config::RawSecret;
use copper_jev_advisor::{Config, ConfigError, Mode};

fn err(env: &Env, f: impl FnOnce(&mut copper_jev_advisor::RawArgs)) -> ConfigError {
    let mut a = env.raw_args();
    f(&mut a);
    Config::from_raw(a).unwrap_err()
}

#[test]
fn defaults_validate_and_secrets_are_redacted() {
    let env = Env::new(0);
    let mut a = env.raw_args();
    a.typesafe_api_key_file = None;
    a.typesafe_api_key = Some(RawSecret::new(KEY));
    let dbg_raw = format!("{a:?}");
    let c = Config::from_raw(a).unwrap();
    assert_eq!(c.mode, Mode::Shadow);
    assert_eq!(c.allowed_models, vec![MODEL.to_string()]);
    assert_eq!(c.resilience.max_inflight, 2);
    for dump in [
        dbg_raw,
        format!("{c:?}"),
        format!("{:?}", c.client_config()),
    ] {
        assert!(!dump.contains(KEY), "secret leaked via Debug: {dump}");
    }
}

#[test]
fn active_mode_is_rejected_with_a_pointer_to_task_019() {
    let env = Env::new(0);
    let e = err(&env, |a| a.jev_mode = "active".into());
    assert_eq!(e, ConfigError::ActiveModeNotSupported);
    assert!(e.to_string().contains("JEV-LIQ-019"));
    assert_eq!(
        err(&env, |a| a.jev_mode = "turbo".into()),
        ConfigError::UnknownMode
    );
}

#[test]
fn key_file_must_not_be_group_or_world_accessible() {
    let env = Env::new(0);
    // `raw_args()` rewrites its own default key file, so test a separate path.
    let p = env.dir.path().join("other.key");
    std::fs::write(&p, format!("{KEY}\n")).unwrap();
    for bad in [0o644, 0o640, 0o604, 0o660, 0o666] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(bad)).unwrap();
        let e = err(&env, |a| a.typesafe_api_key_file = Some(p.clone()));
        assert!(
            matches!(e, ConfigError::KeyFilePermissions { mode, .. } if mode == bad),
            "{bad:o}: {e:?}"
        );
        assert!(e.to_string().contains("0600"));
        assert!(!e.to_string().contains(KEY));
    }
    for good in [0o600, 0o400] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(good)).unwrap();
        let a = {
            let mut a = env.raw_args();
            a.typesafe_api_key_file = Some(p.clone());
            a
        };
        Config::from_raw(a).unwrap();
    }
    // Missing file and a directory are reported without leaking contents.
    let missing = env.dir.path().join("none");
    assert!(matches!(
        err(&env, |a| a.typesafe_api_key_file = Some(missing)),
        ConfigError::KeyFileUnreadable(_)
    ));
    let d = env.dir.path().to_path_buf();
    assert!(matches!(
        err(&env, |a| a.typesafe_api_key_file = Some(d)),
        ConfigError::KeyFileNotRegular(_)
    ));
}

#[test]
fn key_sources_are_exclusive_and_required() {
    let env = Env::new(0);
    assert_eq!(
        err(&env, |a| a.typesafe_api_key = Some(RawSecret::new(KEY))),
        ConfigError::AmbiguousApiKey
    );
    assert_eq!(
        err(&env, |a| a.typesafe_api_key_file = None),
        ConfigError::MissingApiKey
    );
    assert_eq!(
        err(&env, |a| {
            a.typesafe_api_key_file = None;
            a.typesafe_api_key = Some(RawSecret::new("has space"));
        }),
        ConfigError::KeyMalformed
    );
}

#[test]
fn other_settings_are_validated_at_startup() {
    let env = Env::new(0);
    assert!(matches!(
        err(&env, |a| a.typesafe_endpoint =
            "http://api.typesafe.ai/v1/systemone".into()),
        ConfigError::Endpoint(_)
    ));
    assert!(matches!(
        err(&env, |a| a.typesafe_endpoint =
            "https://evil.example/v1/systemone".into()),
        ConfigError::Endpoint(_)
    ));
    assert_eq!(
        err(&env, |a| a.allowed_models = vec!["other-model".into()]),
        ConfigError::ModelNotAllowed(MODEL.into())
    );
    assert_eq!(
        err(&env, |a| a.jev_policy_version = "bad version!".into()),
        ConfigError::BadPolicyVersion
    );
    for f in [
        (|a: &mut copper_jev_advisor::RawArgs| a.jev_max_inflight = 0) as fn(&mut _),
        |a| a.jev_timeout_ms = 0,
        |a| a.advisor_ttl_seconds = 0,
        |a| a.advisor_max_attempts = 0,
        |a| a.advisor_retry_base_ms = 1_000_000,
        |a| a.snapshot_db = a.advisor_db.clone(),
        // A USD budget with no prices is refused (prices are never invented).
        |a| a.jev_budget_usd_micro_per_hour = Some(5_000_000),
        |a| a.jev_price_input_micro_usd_per_mtok = Some(1),
    ] {
        assert!(matches!(err(&env, f), ConfigError::Invalid(_)));
    }
    let mut ok = env.raw_args();
    ok.jev_budget_usd_micro_per_hour = Some(5_000_000);
    ok.jev_price_input_micro_usd_per_mtok = Some(3_000_000);
    ok.jev_price_output_micro_usd_per_mtok = Some(15_000_000);
    ok.jev_budget_tokens_per_hour = Some(100_000);
    let c = Config::from_raw(ok).unwrap();
    assert_eq!(c.resilience.quota.usd_micro_per_window, Some(5_000_000));
}

// ---- the real binary ------------------------------------------------------------------------

fn bin(env: &Env, key_file: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_copper-jev-advisor"));
    c.env_clear()
        .env("TYPESAFE_API_KEY_FILE", key_file)
        .env("JEV_POLICY_VERSION", POLICY)
        .env("ADVISOR_DB", env.advisor_path())
        .env("SNAPSHOT_DB", env.snapshot_path())
        .env("ADVISOR_POLL_MS", "20")
        .env("ADVISOR_METRICS_INTERVAL_SECONDS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

#[test]
fn binary_check_reports_ready_without_touching_the_api() {
    let env = Env::new(1);
    let kf = env.key_file();
    let out = bin(&env, &kf).arg("check").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ready"], true);
    assert_eq!(v["snapshot_schema"], mev_liq_state::SCHEMA_VERSION);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(KEY));

    // Not ready when the snapshot DB is absent.
    let out = bin(&env, &kf)
        .env("SNAPSHOT_DB", env.dir.path().join("absent.db"))
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not ready"));
}

#[test]
fn binary_refuses_active_mode_and_loose_key_permissions() {
    let env = Env::new(0);
    let kf = env.key_file();
    let out = bin(&env, &kf).env("JEV_MODE", "active").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("JEV-LIQ-019"), "{stderr}");

    std::fs::set_permissions(&kf, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = bin(&env, &kf).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("0600") && !stderr.contains(KEY), "{stderr}");
}

#[test]
fn binary_exits_zero_on_sigterm() {
    let env = Env::new(0);
    let kf = env.key_file();
    let mut child = bin(&env, &kf).spawn().unwrap();
    // Wait for startup (the advisory DB is created by the worker).
    let start = Instant::now();
    while !env.advisor_path().exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "worker did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no exit after SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0), "status: {status:?}");
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
    assert!(out.contains("advisor worker stopped"), "{out}");
    assert!(!out.contains(KEY));
}
