//! Real local HTTP/1.1 server on a TCP socket. It records the exact bytes the client sent and
//! replies with scripted raw bytes (fixtures), so the production client code path is unmodified.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jev_advisor::{ClientConfig, Secret};
use mev_liq_features::{
    AssetClass, Chain, DebtUsdBand, FeatureV1, HealthFactorBand, HfTrend, LiquidityToDebtBand,
    OracleVolatilityBand, Protocol, RecheckAgeBand, RedactedFeatures, RouteSuccessBand,
    SnapshotQuality,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const KEY: &str = "tsk-test-KEY-9f8e7d6c5b4a";
pub const MODEL: &str = "jev-1.13.0";

#[derive(Clone)]
pub enum Script {
    /// Write these bytes and close.
    Raw(Vec<u8>),
    /// Read the request, never answer.
    Hang,
    /// Write `head`, then `body` one byte at a time with `delay` between bytes.
    Drip {
        head: Vec<u8>,
        delay: Duration,
        body: Vec<u8>,
    },
    /// 200 + chunked JSON, then keep streaming `total` bytes in `chunk`-sized pieces.
    Flood { chunk: usize, total: usize },
    /// Answer request number `i` (0-based, by arrival) with `scripts[min(i, len-1)]`.
    Seq(Vec<Script>),
    /// Wait `delay` (real time) after reading the request, then behave as `inner`.
    Delayed(Duration, Box<Script>),
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub request_line: String,
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
    pub flood_sent: Arc<AtomicUsize>,
}

impl TestServer {
    pub async fn spawn(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let flood_sent = Arc::new(AtomicUsize::new(0));
        let (r, c, f) = (requests.clone(), connections.clone(), flood_sent.clone());
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
                tokio::spawn(handle(sock, chosen, r.clone(), f.clone()));
            }
        });
        Self {
            addr,
            requests,
            connections,
            flood_sent,
        }
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

async fn handle(
    mut sock: tokio::net::TcpStream,
    script: Script,
    requests: Arc<Mutex<Vec<Recorded>>>,
    flood_sent: Arc<AtomicUsize>,
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
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
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
        request_line,
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
        Script::Drip { head, delay, body } => {
            let _ = sock.write_all(&head).await;
            for b in body {
                if sock.write_all(&[b]).await.is_err() || sock.flush().await.is_err() {
                    return;
                }
                tokio::time::sleep(delay).await;
            }
        }
        Script::Flood { chunk, total } => {
            let hd = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if sock.write_all(hd).await.is_err() {
                return;
            }
            let data = vec![b' '; chunk];
            let mut sent = 0;
            while sent < total {
                let frame = [format!("{chunk:x}\r\n").as_bytes(), &data, b"\r\n"].concat();
                if sock.write_all(&frame).await.is_err() {
                    break;
                }
                sent += chunk;
                flood_sent.store(sent, Ordering::SeqCst);
            }
        }
    }
}

/// A complete HTTP response with Content-Length and `Connection: close`.
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

pub fn json_ok(body: &[u8]) -> Script {
    Script::Raw(http(
        "200 OK",
        &[("Content-Type", "application/json")],
        body,
    ))
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

pub fn cfg(addr: SocketAddr) -> ClientConfig {
    let mut c = ClientConfig::new(Secret::new(KEY).unwrap(), MODEL, vec![MODEL.to_string()]);
    c.endpoint = format!("http://{addr}/v1/systemone");
    c.allowed_hosts = vec!["127.0.0.1".to_string()];
    c.allow_loopback_http = true;
    c.connect_timeout = Duration::from_millis(500);
    c.read_timeout = Duration::from_millis(1000);
    c.total_timeout = Duration::from_millis(2000);
    c.max_response_bytes = 16 * 1024;
    c
}

/// The synthetic features of the spec section 6 example.
pub fn features() -> RedactedFeatures {
    RedactedFeatures::new(FeatureV1 {
        chain: Chain::ArbitrumOne,
        protocol: Protocol::AaveV3,
        health_factor_band: HealthFactorBand::B100To102,
        debt_usd_band: DebtUsdBand::K100To500,
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

/// Same shape as [`features`] but distinct per `i` (0..64), so requests have distinct keys.
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

pub mod manual;
