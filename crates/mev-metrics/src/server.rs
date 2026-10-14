//! Optional `/metrics` HTTP listener. Default OFF everywhere; loopback only unless the operator
//! explicitly overrides. Plain `std::net`, one accept thread, one request at a time, hard per-connection
//! deadline, `GET /metrics` only. It renders from the [`Registry`] on each scrape, so a slow or hostile
//! client can delay other scrapes by at most the deadline and can never touch a producer.

use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::registry::Registry;

/// Total time one connection may take (read request, render, write).
const CONN_DEADLINE: Duration = Duration::from_secs(2);
const MAX_REQUEST: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum ListenAddrError {
    Unparseable(String),
    NotLoopback(IpAddr),
}

impl std::fmt::Display for ListenAddrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable(s) => write!(f, "metrics address {s:?} is not host:port"),
            Self::NotLoopback(ip) => write!(
                f,
                "metrics address {ip} is not loopback; pass the explicit non-loopback override to expose it"
            ),
        }
    }
}
impl std::error::Error for ListenAddrError {}

/// Parse `ip:port`. Hostnames are refused (no DNS). A non-loopback IP needs `allow_non_loopback`.
pub fn parse_listen_addr(s: &str, allow_non_loopback: bool) -> Result<SocketAddr, ListenAddrError> {
    let a: SocketAddr = s
        .trim()
        .parse()
        .map_err(|_| ListenAddrError::Unparseable(s.to_string()))?;
    if !a.ip().is_loopback() && !allow_non_loopback {
        return Err(ListenAddrError::NotLoopback(a.ip()));
    }
    Ok(a)
}

/// Read the optional exporter address of a process from the environment: `<prefix>_ADDR` (unset or empty
/// means off) and `<prefix>_ALLOW_NON_LOOPBACK=1`.
pub fn addr_from_env(prefix: &str) -> Result<Option<SocketAddr>, ListenAddrError> {
    let Ok(v) = std::env::var(format!("{prefix}_ADDR")) else {
        return Ok(None);
    };
    if v.trim().is_empty() {
        return Ok(None);
    }
    let allow = std::env::var(format!("{prefix}_ALLOW_NON_LOOPBACK")).is_ok_and(|x| x == "1");
    parse_listen_addr(&v, allow).map(Some)
}

pub struct ServerHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Bind `addr` and serve `registry` on a background thread.
    pub fn start(addr: SocketAddr, registry: Arc<Registry>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("metrics-http".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    if let Ok(s) = conn {
                        serve(s, &registry);
                    }
                }
            })?;
        tracing::info!(%addr, "prometheus exporter listening");
        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the blocking accept.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body.as_bytes());
    let _ = s.flush();
}

fn serve(mut s: TcpStream, registry: &Registry) {
    let start = Instant::now();
    let left = |start: Instant| CONN_DEADLINE.saturating_sub(start.elapsed());
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        let remaining = left(start);
        if remaining.is_zero() || s.set_read_timeout(Some(remaining)).is_err() {
            return;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_REQUEST {
            respond(
                &mut s,
                "431 Request Header Fields Too Large",
                "text/plain",
                "too large\n",
            );
            return;
        }
    }
    let _ = s.set_write_timeout(Some(left(start).max(Duration::from_millis(50))));
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = path.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/metrics") => respond(
            &mut s,
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            &registry.render(),
        ),
        ("GET" | "HEAD", _) => respond(&mut s, "404 Not Found", "text/plain", "not found\n"),
        _ => respond(
            &mut s,
            "405 Method Not Allowed",
            "text/plain",
            "method not allowed\n",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(addr: SocketAddr, req: &str) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    #[test]
    fn loopback_only_unless_overridden() {
        assert!(parse_listen_addr("127.0.0.1:9100", false).is_ok());
        assert!(parse_listen_addr("[::1]:9100", false).is_ok());
        assert_eq!(
            parse_listen_addr("0.0.0.0:9100", false),
            Err(ListenAddrError::NotLoopback("0.0.0.0".parse().unwrap()))
        );
        assert!(parse_listen_addr("0.0.0.0:9100", true).is_ok());
        assert!(matches!(
            parse_listen_addr("localhost:9100", true),
            Err(ListenAddrError::Unparseable(_))
        ));
        assert!(parse_listen_addr("9100", false).is_err());
    }

    #[test]
    fn env_is_off_by_default() {
        assert_eq!(addr_from_env("MEV_METRICS_TEST_UNSET_PREFIX"), Ok(None));
    }

    #[test]
    fn serves_metrics_and_rejects_everything_else() {
        let reg = Arc::new(Registry::new());
        reg.add_collector(|e| {
            e.family("copper_test_total", "t", crate::Kind::Counter);
            e.sample_u64(&[], 7);
        });
        let h = ServerHandle::start("127.0.0.1:0".parse().unwrap(), reg).unwrap();
        assert!(h.local_addr().ip().is_loopback());
        let ok = get(h.local_addr(), "GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK"), "{ok}");
        assert!(ok.contains("version=0.0.4"));
        assert!(ok.contains("copper_test_total 7\n"));
        assert!(get(h.local_addr(), "GET /other HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(get(h.local_addr(), "POST /metrics HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
        let big = format!("GET /metrics HTTP/1.1\r\nX: {}", "a".repeat(9000));
        assert!(get(h.local_addr(), &big).starts_with("HTTP/1.1 431"));
    }

    #[test]
    fn a_stalled_client_does_not_hold_the_listener_past_the_deadline() {
        let reg = Arc::new(Registry::new());
        let h = ServerHandle::start("127.0.0.1:0".parse().unwrap(), reg).unwrap();
        let _stalled = TcpStream::connect(h.local_addr()).unwrap();
        let t = Instant::now();
        let ok = get(h.local_addr(), "GET /metrics HTTP/1.1\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200"));
        assert!(t.elapsed() < Duration::from_secs(CONN_DEADLINE.as_secs() + 3));
    }

    #[test]
    fn dropping_the_handle_stops_the_listener() {
        let reg = Arc::new(Registry::new());
        let h = ServerHandle::start("127.0.0.1:0".parse().unwrap(), reg).unwrap();
        let addr = h.local_addr();
        drop(h);
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
    }
}
