//! Test fixtures: a real local HTTP/1.1 server on a TCP socket (a minimal copy of the JEV-LIQ-009
//! fixture in crates/jev-advisor/tests/common; it is test code, never production code), temp
//! SQLite files populated through `SnapshotRepository::enqueue_with_snapshot`, and log capture.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, B256, U256, keccak256};
use copper_jev_advisor::{Config, RawArgs};
use jev_advisor::{ClientConfig, Secret};
use mev_liq_features::{
    AssetClass, Chain, DebtUsdBand, FeatureV1, HealthFactorBand, HfTrend, LiquidityToDebtBand,
    OracleVolatilityBand, Protocol, RecheckAgeBand, RedactedFeatures, RouteSuccessBand,
    SnapshotQuality,
};
use mev_liq_state::{
    AccountKey, BorrowerSnapshot, ChainRef, ChainSource, ExportItem, OutboxConfig,
    SnapshotRepository, SnapshotStatus, WAD,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;

pub const KEY: &str = "tsk-test-KEY-9f8e7d6c5b4a";
pub const MODEL: &str = "jev-1.13.0";
pub const POLICY: &str = "policy-v1";

// ---------------------------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub enum Script {
    /// Write these bytes and close.
    Raw(Vec<u8>),
    /// Read the request, never answer.
    Hang,
    /// Answer connection number `i` (0-based, by arrival) with `scripts[min(i, len-1)]`.
    Seq(Vec<Script>),
    /// Wait `delay` after reading the request, then behave as `inner`.
    Delayed(Duration, Box<Script>),
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    pub connections: Arc<AtomicUsize>,
}

impl TestServer {
    pub async fn spawn(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let (r, c) = (requests.clone(), connections.clone());
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let n = c.fetch_add(1, Ordering::SeqCst);
                let chosen = match &script {
                    Script::Seq(v) => v[n.min(v.len() - 1)].clone(),
                    other => other.clone(),
                };
                tokio::spawn(handle(sock, chosen, r.clone()));
            }
        });
        Self {
            addr,
            requests,
            connections,
        }
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    pub fn hits(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

async fn handle(
    mut sock: tokio::net::TcpStream,
    script: Script,
    requests: Arc<Mutex<Vec<Recorded>>>,
) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let headers: Vec<(String, String)> = head
        .split("\r\n")
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    requests.lock().unwrap().push(Recorded {
        headers,
        body: buf[head_end..head_end + len].to_vec(),
    });
    let mut script = script;
    while let Script::Delayed(d, inner) = script {
        tokio::time::sleep(d).await;
        script = *inner;
    }
    match script {
        Script::Seq(_) | Script::Delayed(..) => {}
        Script::Raw(bytes) => {
            let _ = sock.write_all(&bytes).await;
            let _ = sock.shutdown().await;
        }
        Script::Hang => {
            let mut sink = [0u8; 64];
            while matches!(sock.read(&mut sink).await, Ok(n) if n > 0) {}
        }
    }
}

pub fn http(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut b = out.into_bytes();
    b.extend_from_slice(body);
    b
}

pub fn valid_body() -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valid.json"))
        .unwrap()
}

pub fn ok() -> Script {
    Script::Raw(http(
        "200 OK",
        &[("Content-Type", "application/json")],
        &valid_body(),
    ))
}

pub fn too_many() -> Script {
    Script::Raw(http("429 Too Many Requests", &[], b"{}"))
}

/// A loopback port nothing listens on.
pub fn closed_port_addr() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

// ---------------------------------------------------------------------------------------------
// Databases
// ---------------------------------------------------------------------------------------------

pub fn pool() -> Address {
    Address::repeat_byte(0xaa)
}

pub fn account(n: u8) -> AccountKey {
    AccountKey {
        chain_id: 42161,
        pool: pool(),
        borrower: Address::repeat_byte(n),
    }
}

fn snapshot(n: u8, block: u64) -> BorrowerSnapshot {
    BorrowerSnapshot {
        key: account(n),
        revision: 0,
        chain: ChainRef {
            block_number: block,
            block_hash: keccak256(block.to_be_bytes()),
            source: ChainSource::Provisional,
            observed_at_ns: block * 1_000,
        },
        health_factor_wad: WAD + U256::from(5u8),
        total_debt_base: U256::from(100u8),
        total_collateral_base: U256::from(200u8),
        available_borrows_base: U256::from(7u8),
        liquidation_threshold: U256::from(8000u16),
        ltv: U256::from(7500u16),
        reserves: vec![],
        risk_metadata_version: 1,
        dirty_reasons: vec![],
        last_successful_check_ns: Some(block * 1_000),
        status: SnapshotStatus::Valid,
        provenance: "test".into(),
        feature_digest: B256::ZERO,
    }
    .seal()
}

/// Distinct features per `i` (0..64) so requests do not coalesce or hit the cache.
pub fn features_n(i: usize) -> RedactedFeatures {
    use DebtUsdBand as D;
    use HealthFactorBand as H;
    let hf = [
        H::NoDebt,
        H::Below1,
        H::B100To102,
        H::B102To105,
        H::B105To110,
        H::B110To125,
        H::B125To200,
        H::Above200,
    ];
    let debt = [
        D::Zero,
        D::Under1k,
        D::K1To10,
        D::K10To100,
        D::K100To500,
        D::K500ToM1,
        D::Above1m,
        D::Unknown,
    ];
    RedactedFeatures::new(FeatureV1 {
        chain: Chain::ArbitrumOne,
        protocol: Protocol::AaveV3,
        health_factor_band: hf[i % 8],
        debt_usd_band: debt[(i / 8) % 8],
        collateral_class: AssetClass::VolatileEthCorrelated,
        debt_class: AssetClass::Stablecoin,
        oracle_volatility_band: OracleVolatilityBand::High,
        hf_trend: HfTrend::Falling,
        liquidity_to_debt_band: LiquidityToDebtBand::Medium,
        historical_route_success_band: RouteSuccessBand::Low,
        last_account_recheck_age_band: RecheckAgeBand::Under30s,
        snapshot_quality: SnapshotQuality::VerifiedCanonical,
    })
    .unwrap()
}

pub struct Env {
    pub dir: tempfile::TempDir,
    pub repo: SnapshotRepository,
}

impl Env {
    /// Snapshot DB with `n` accounts (1..=n) exported through the real producer path.
    pub fn new(n: u8) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = SnapshotRepository::open(&dir.path().join("snapshot.db")).unwrap();
        let env = Self { dir, repo };
        for i in 1..=n {
            env.export(i, 10, 0, &features_n(usize::from(i)));
        }
        env
    }

    pub fn snapshot_path(&self) -> PathBuf {
        self.dir.path().join("snapshot.db")
    }

    pub fn advisor_path(&self) -> PathBuf {
        self.dir.path().join("advisor.db")
    }

    pub fn export(&self, n: u8, block: u64, expected: u64, f: &RedactedFeatures) {
        self.export_raw(n, block, expected, f.json(), f.digest());
    }

    pub fn export_raw(&self, n: u8, block: u64, expected: u64, json: &str, digest: B256) {
        self.repo
            .enqueue_with_snapshot(
                &snapshot(n, block),
                expected,
                &ExportItem {
                    policy_version: POLICY,
                    feature_digest: digest,
                    feature_json: json,
                    enqueued_at_ns: 1,
                },
                &OutboxConfig {
                    max_export_rows: 10_000,
                },
            )
            .unwrap();
    }

    pub fn key_file(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = self.dir.path().join("typesafe.key");
        std::fs::write(&p, format!("{KEY}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        p
    }

    pub fn raw_args(&self) -> RawArgs {
        RawArgs {
            typesafe_endpoint: "https://api.typesafe.ai/v1/systemone".into(),
            typesafe_allowed_hosts: vec!["api.typesafe.ai".into()],
            typesafe_api_key_file: Some(self.key_file()),
            typesafe_api_key: None,
            jev_model: MODEL.into(),
            allowed_models: vec![],
            jev_mode: "shadow".into(),
            jev_policy_version: POLICY.into(),
            chain_id: 42161,
            jev_max_inflight: 2,
            jev_timeout_ms: 1500,
            jev_budget_per_hour: 200,
            jev_budget_tokens_per_hour: None,
            jev_budget_usd_micro_per_hour: None,
            jev_price_input_micro_usd_per_mtok: None,
            jev_price_output_micro_usd_per_mtok: None,
            advisor_db: self.advisor_path(),
            snapshot_db: self.snapshot_path(),
            advisor_ttl_seconds: 60,
            advisor_poll_ms: 20,
            advisor_max_pending: 1000,
            advisor_max_attempts: 5,
            advisor_retry_base_ms: 20,
            advisor_retry_max_ms: 100,
            advisor_fallback_pause_ms: 50,
            advisor_shutdown_grace_seconds: 5,
            advisor_metrics_interval_seconds: 1,
        }
    }

    /// Production-validated config, then test-tuned timings (fields are public).
    pub fn config(&self) -> Config {
        let mut c = Config::from_raw(self.raw_args()).unwrap();
        c.metrics_interval = Duration::from_millis(100);
        c.resilience.retry.base_delay = Duration::from_millis(5);
        c.resilience.retry.max_delay = Duration::from_millis(10);
        c
    }
}

/// Transport config pointing at the local server (loopback http is a dev-only client feature).
pub fn client_cfg(addr: SocketAddr) -> ClientConfig {
    let mut c = ClientConfig::new(Secret::new(KEY).unwrap(), MODEL, vec![MODEL.to_string()]);
    c.endpoint = format!("http://{addr}/v1/systemone");
    c.allowed_hosts = vec!["127.0.0.1".to_string()];
    c.allow_loopback_http = true;
    c.connect_timeout = Duration::from_millis(500);
    c.read_timeout = Duration::from_millis(1500);
    c.total_timeout = Duration::from_millis(3000);
    c.max_response_bytes = 16 * 1024;
    c
}

pub async fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < timeout, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// Log capture (global: every test in this process writes to the same buffer)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> CaptureWriter {
        CaptureWriter(self.0.clone())
    }
}

/// Records every `jev_request` span and whether it is a trace root.
struct RootRecorder(Arc<Mutex<Vec<(String, bool)>>>);

impl<S> tracing_subscriber::Layer<S> for RootRecorder
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).unwrap();
        self.0
            .lock()
            .unwrap()
            .push((attrs.metadata().name().to_string(), span.parent().is_none()));
    }
}

static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
type SpanLog = Arc<Mutex<Vec<(String, bool)>>>;
static SPANS: OnceLock<SpanLog> = OnceLock::new();
static INIT: Once = Once::new();

pub fn init_logs() {
    INIT.call_once(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let spans = Arc::new(Mutex::new(Vec::new()));
        LOGS.set(buf.clone()).ok();
        SPANS.set(spans.clone()).ok();
        let fmt = tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .with_ansi(false)
            .with_writer(Capture(buf));
        let sub = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(fmt)
            .with(RootRecorder(spans));
        tracing::subscriber::set_global_default(sub).unwrap();
    });
}

pub fn logs() -> String {
    String::from_utf8_lossy(&LOGS.get().unwrap().lock().unwrap()).into_owned()
}

/// `(span name, is trace root)` for every span created so far.
pub fn spans() -> Vec<(String, bool)> {
    SPANS.get().unwrap().lock().unwrap().clone()
}

/// Longest run of hex digits in `s`.
pub fn longest_hex_run(s: &str) -> usize {
    let (mut best, mut run) = (0, 0);
    for b in s.bytes() {
        if b.is_ascii_hexdigit() {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

pub fn assert_logs_clean() {
    let l = logs();
    assert!(!l.is_empty(), "no logs were captured");
    assert!(!l.contains(KEY), "API key leaked into logs");
    let lower = l.to_ascii_lowercase();
    assert!(
        !lower.contains("authorization"),
        "auth header leaked into logs"
    );
    assert!(!lower.contains("bearer"), "bearer token leaked into logs");
    assert!(
        longest_hex_run(&l) < 40,
        "an address/hash-sized hex string leaked into logs"
    );
}

pub fn dir_of(p: &Path) -> &Path {
    p.parent().unwrap()
}
