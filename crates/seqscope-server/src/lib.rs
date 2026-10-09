//! seqscope's local server: the collector and the API the web UI (and anything else) talks to.
//!
//! One process does three things: reads transactions (the live feed, or a recorded file), judges them with the current
//! rules and stores what matches (see `seqscope-store`), and serves an HTTP + WebSocket API and the embedded web UI.
//!
//! Safety: it binds to localhost and, unless `allow_remote`, only answers requests whose `Host` is a loopback name (a
//! web page on another site cannot reach it through DNS rebinding); every request that changes anything must carry
//! `X-Seqscope: 1` (a browser cannot add that header cross-site without a preflight that is refused); a WebSocket whose
//! `Origin` is not this server is refused. The rules endpoint is the only writer, and it only writes the rules file.

mod collector;

use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use seqscope_core::{Engine, Preview, Row, RuleError, RuleSet, Tx, catalogue};
use seqscope_store::{Ingest, Query as RowQuery, Reader};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

pub use collector::Source;

/// How many recent transactions are kept for dry-run previews and probes.
pub const RECENT: usize = 5_000;

pub struct Config {
    pub rules: PathBuf,
    pub db: PathBuf,
    pub bind: SocketAddr,
    pub allow_remote: bool,
    pub source: Source,
    pub writer: seqscope_store::WriterConfig,
}

struct RulesState {
    text: String,
    errors: Vec<RuleError>,
    mtime: Option<SystemTime>,
}

pub struct AppState {
    rules_path: PathBuf,
    db_path: PathBuf,
    engine: RwLock<Arc<Engine>>,
    rules: RwLock<RulesState>,
    ingest: Ingest,
    rows_tx: broadcast::Sender<Arc<str>>,
    txs_tx: broadcast::Sender<Arc<Tx>>,
    recent: Mutex<VecDeque<Tx>>,
    txs: AtomicU64,
    rows: AtomicU64,
    started: Instant,
    allow_remote: bool,
}

fn counters_rules() -> RuleSet {
    use seqscope_core::{ActionKind, Mode, Profile, Rule};
    let rule = |k: &str| Rule {
        name: k.into(),
        enabled: true,
        when: format!("kind == {k}"),
        action: ActionKind::Count,
        tag: None,
        sample: 1,
    };
    RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "counters".into(),
            enabled: true,
            mode: Mode::All,
            rule: [
                "swap",
                "liquidity",
                "liquidation",
                "undecoded_known",
                "unknown",
            ]
            .map(rule)
            .to_vec(),
        }],
    }
}

fn toml_error(message: String) -> RuleError {
    RuleError {
        profile: String::new(),
        rule: String::new(),
        message,
        column: None,
    }
}

/// Parse and compile rules text; every problem is returned, none is hidden.
pub fn validate(text: &str) -> std::result::Result<(RuleSet, Engine), Vec<RuleError>> {
    let set = RuleSet::from_toml(text).map_err(|e| vec![toml_error(e)])?;
    let engine = Engine::compile(&set)?;
    Ok((set, engine))
}

impl AppState {
    pub fn new(
        rules_path: PathBuf,
        db_path: PathBuf,
        ingest: Ingest,
        allow_remote: bool,
    ) -> Arc<AppState> {
        let (rows_tx, _) = broadcast::channel(4096);
        let (txs_tx, _) = broadcast::channel(4096);
        let st = Arc::new(AppState {
            rules_path,
            db_path,
            engine: RwLock::new(Arc::new(
                Engine::compile(&counters_rules()).expect("built-in rules compile"),
            )),
            rules: RwLock::new(RulesState {
                text: String::new(),
                errors: Vec::new(),
                mtime: None,
            }),
            ingest,
            rows_tx,
            txs_tx,
            recent: Mutex::new(VecDeque::with_capacity(RECENT)),
            txs: AtomicU64::new(0),
            rows: AtomicU64::new(0),
            started: Instant::now(),
            allow_remote,
        });
        st.reload_rules_file();
        st
    }

    /// Read the rules file; a broken file keeps the previous engine and records what is wrong.
    pub fn reload_rules_file(&self) {
        let Ok(text) = std::fs::read_to_string(&self.rules_path) else {
            return; // no file yet: counters only
        };
        let mtime = std::fs::metadata(&self.rules_path)
            .and_then(|m| m.modified())
            .ok();
        let mut r = self.rules.write().unwrap();
        r.text = text.clone();
        r.mtime = mtime;
        match validate(&text) {
            Ok((_, engine)) => {
                r.errors.clear();
                *self.engine.write().unwrap() = Arc::new(engine);
            }
            Err(e) => r.errors = e,
        }
    }

    fn file_changed(&self) -> bool {
        let m = std::fs::metadata(&self.rules_path)
            .and_then(|m| m.modified())
            .ok();
        m.is_some() && m != self.rules.read().unwrap().mtime
    }

    /// Validate, write atomically, and switch to the new rules. Nothing is written or switched if anything is wrong.
    pub fn save_rules(&self, text: &str) -> std::result::Result<(), Vec<RuleError>> {
        let (_, engine) = validate(text)?;
        let tmp = self.rules_path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &self.rules_path))
            .map_err(|e| {
                vec![toml_error(format!(
                    "could not write {}: {e}",
                    self.rules_path.display()
                ))]
            })?;
        let mut r = self.rules.write().unwrap();
        r.text = text.to_string();
        r.errors.clear();
        r.mtime = std::fs::metadata(&self.rules_path)
            .and_then(|m| m.modified())
            .ok();
        *self.engine.write().unwrap() = Arc::new(engine);
        Ok(())
    }

    /// One transaction through the rules: remember it for previews, store and broadcast what matches.
    pub fn feed(&self, tx: Tx) {
        self.txs.fetch_add(1, Ordering::Relaxed);
        let engine = self.engine.read().unwrap().clone();
        for row in engine.process(&tx) {
            self.rows.fetch_add(1, Ordering::Relaxed);
            let _ = self.rows_tx.send(row_message(&row).into());
            self.ingest.send(row);
        }
        let tx = Arc::new(tx);
        let _ = self.txs_tx.send(tx.clone());
        let mut recent = self.recent.lock().unwrap();
        if recent.len() == RECENT {
            recent.pop_front();
        }
        recent.push_back((*tx).clone());
    }

    fn status(&self, window: Duration, last_txs: u64) -> Value {
        let s = self.ingest.stats().snapshot();
        let engine = self.engine.read().unwrap().clone();
        let txs = self.txs.load(Ordering::Relaxed);
        json!({
            "type": "status",
            "uptime_s": self.started.elapsed().as_secs(),
            "txs": txs,
            "tx_per_s": ((txs - last_txs) as f64 / window.as_secs_f64().max(0.001)).round(),
            "calls": engine.calls_seen(),
            "rows": self.rows.load(Ordering::Relaxed),
            "written": s.written, "dropped": s.dropped, "failed": s.failed,
            "last_error": s.last_error,
            "rules": engine.stats(),
        })
    }
}

fn row_message(row: &Row) -> String {
    json!({ "type": "row", "row": row }).to_string()
}

// ---------------------------------------------------------------------------------------------------------------
// Errors and guards

struct ApiError(StatusCode, Value);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

fn bad_request(message: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        json!({ "ok": false, "error": message.to_string() }),
    )
}

fn host_only(h: &str) -> &str {
    // "localhost:7878" -> "localhost"; "[::1]:7878" -> "[::1]"
    if let Some(rest) = h.strip_prefix('[') {
        return rest
            .split(']')
            .next()
            .map(|_| &h[..h.find(']').map_or(h.len(), |i| i + 1)])
            .unwrap_or(h);
    }
    h.split(':').next().unwrap_or(h)
}

fn is_loopback_host(h: &str) -> bool {
    matches!(host_only(h), "127.0.0.1" | "localhost" | "[::1]")
}

async fn guard(State(st): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let h = req.headers();
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !st.allow_remote && !is_loopback_host(host) {
        return (StatusCode::FORBIDDEN, "seqscope answers only to localhost").into_response();
    }
    if let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let o = origin.split("://").nth(1).unwrap_or(origin);
        if o != host {
            return (StatusCode::FORBIDDEN, "cross-origin requests are refused").into_response();
        }
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) && h.get("x-seqscope").is_none() {
        return (StatusCode::FORBIDDEN, "changes need the X-Seqscope header").into_response();
    }
    next.run(req).await
}

// ---------------------------------------------------------------------------------------------------------------
// Handlers

const INDEX: &str = include_str!("../ui/index.html");
const APP_JS: &str = include_str!("../ui/app.js");
const APP_CSS: &str = include_str!("../ui/app.css");

async fn index() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        INDEX,
    )
}
async fn app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        APP_JS,
    )
}
async fn app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        APP_CSS,
    )
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "name": "seqscope", "version": env!("CARGO_PKG_VERSION") }))
}

async fn get_catalogue() -> Json<catalogue::Catalogue> {
    Json(catalogue::catalogue())
}

async fn get_rules(State(st): State<Arc<AppState>>) -> Json<Value> {
    let r = st.rules.read().unwrap();
    Json(
        json!({ "text": r.text, "errors": r.errors, "path": st.rules_path.display().to_string(), "exists": st.rules_path.exists() }),
    )
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

fn summary(set: &RuleSet) -> Value {
    json!(set.profile.iter().map(|p| json!({
        "name": p.name, "enabled": p.enabled, "mode": p.mode,
        "rules": p.rule.iter().map(|r| json!({"name": r.name, "enabled": r.enabled, "when": r.when, "action": r.action, "tag": r.tag, "sample": r.sample})).collect::<Vec<_>>(),
    })).collect::<Vec<_>>())
}

async fn validate_rules(Json(b): Json<TextBody>) -> Json<Value> {
    match validate(&b.text) {
        Ok((set, _)) => Json(json!({ "ok": true, "errors": [], "profiles": summary(&set) })),
        Err(errors) => Json(json!({ "ok": false, "errors": errors })),
    }
}

async fn put_rules(State(st): State<Arc<AppState>>, Json(b): Json<TextBody>) -> Response {
    match st.save_rules(&b.text) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(errors) => ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "ok": false, "errors": errors }),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
struct PreviewBody {
    when: String,
    #[serde(default = "five")]
    samples: usize,
}
fn five() -> usize {
    5
}

async fn preview(State(st): State<Arc<AppState>>, Json(b): Json<PreviewBody>) -> Response {
    let txs: Vec<Tx> = st.recent.lock().unwrap().iter().cloned().collect();
    let res = tokio::task::spawn_blocking(move || {
        seqscope_core::preview(&b.when, &txs, b.samples.min(50))
    })
    .await;
    match res {
        Ok(Ok(p)) => Json::<Preview>(p).into_response(),
        Ok(Err(e)) => ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "ok": false, "errors": [e] }),
        )
        .into_response(),
        Err(e) => bad_request(e).into_response(),
    }
}

#[derive(Deserialize, Default)]
struct RowsParams {
    kind: Option<String>,
    protocol: Option<String>,
    selector: Option<String>,
    to: Option<String>,
    profile: Option<String>,
    rule: Option<String>,
    tag: Option<String>,
    #[serde(default)]
    alerts: bool,
    minutes: Option<u64>,
    limit: Option<usize>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

async fn rows(
    State(st): State<Arc<AppState>>,
    Query(p): Query<RowsParams>,
) -> Result<Json<Vec<Row>>, ApiError> {
    let db = st.db_path.clone();
    let q = RowQuery {
        since_ms: p.minutes.map(|m| now_ms().saturating_sub(m * 60_000)),
        until_ms: None,
        protocol: p.protocol,
        kind: p.kind,
        selector: p.selector,
        to: p.to,
        profile: p.profile,
        rule: p.rule,
        tag: p.tag,
        alert_only: p.alerts,
        limit: Some(p.limit.unwrap_or(100).min(5_000)),
    };
    let r = tokio::task::spawn_blocking(move || Reader::open(db).and_then(|r| r.query(&q)))
        .await
        .map_err(bad_request)?;
    r.map(Json).map_err(bad_request)
}

#[derive(Deserialize)]
struct SqlBody {
    sql: String,
    #[serde(default = "thousand")]
    max_rows: usize,
}
fn thousand() -> usize {
    1000
}

async fn sql(
    State(st): State<Arc<AppState>>,
    Json(b): Json<SqlBody>,
) -> Result<Json<seqscope_store::SqlResult>, ApiError> {
    let db = st.db_path.clone();
    let r = tokio::task::spawn_blocking(move || {
        Reader::open(db).and_then(|r| r.sql(&b.sql, b.max_rows.min(10_000)))
    })
    .await
    .map_err(bad_request)?;
    r.map(Json).map_err(bad_request)
}

async fn stats(State(st): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let db = st.db_path.clone();
    let by = tokio::task::spawn_blocking(move || {
        let r = Reader::open(&db)?;
        let span = r.sql("SELECT count(*), min(t_ms), max(t_ms) FROM rows", 1)?;
        let kinds = r.sql("SELECT kind, coalesce(protocol,'-') AS protocol, count(*) AS rows FROM rows GROUP BY 1,2 ORDER BY 3 DESC LIMIT 40", 40)?;
        let rules = r.sql("SELECT profile, rule, count(*) AS rows FROM rows GROUP BY 1,2 ORDER BY 3 DESC LIMIT 40", 40)?;
        Ok::<_, seqscope_store::StoreError>((span, kinds, rules, seqscope_store::disk_bytes(&db)))
    })
    .await
    .map_err(bad_request)?
    .map_err(bad_request)?;
    let status = st.status(Duration::from_secs(1), st.txs.load(Ordering::Relaxed));
    Ok(Json(json!({
        "db": { "rows": by.0.rows[0][0], "first_ms": by.0.rows[0][1], "last_ms": by.0.rows[0][2], "bytes": by.3 },
        "by_kind": by.1, "by_rule": by.2, "collector": status,
    })))
}

#[derive(Deserialize)]
struct GapParams {
    limit: Option<usize>,
}

async fn gaps(
    State(st): State<Arc<AppState>>,
    Query(p): Query<GapParams>,
) -> Result<Json<seqscope_store::SqlResult>, ApiError> {
    let db = st.db_path.clone();
    let limit = p.limit.unwrap_or(30).min(500);
    let r = tokio::task::spawn_blocking(move || {
        Reader::open(db).and_then(|r| {
            r.sql(
                &format!(
                    "SELECT kind, coalesce(known,'(unidentified)') AS contract, to_addr AS destination, selector, count(*) AS calls
                     FROM v_rows WHERE kind IN ('undecoded_known','unknown') GROUP BY 1,2,3,4 ORDER BY calls DESC LIMIT {limit}"
                ),
                limit,
            )
        })
    })
    .await
    .map_err(bad_request)?;
    r.map(Json).map_err(bad_request)
}

#[derive(Deserialize)]
struct LiveParams {
    /// `rows` (what the stored rules matched) or `probe` (the rule in `when`, tried on every transaction as it arrives).
    mode: Option<String>,
    when: Option<String>,
}

async fn live(
    ws: WebSocketUpgrade,
    State(st): State<Arc<AppState>>,
    Query(p): Query<LiveParams>,
) -> Response {
    ws.on_upgrade(move |socket| live_socket(st, socket, p))
}

async fn send(socket: &mut WebSocket, v: &Value) -> bool {
    socket
        .send(Message::Text(v.to_string().into()))
        .await
        .is_ok()
}

async fn live_socket(st: Arc<AppState>, mut socket: WebSocket, p: LiveParams) {
    if p.mode.as_deref() == Some("probe") {
        let when = p.when.unwrap_or_else(|| "true".into());
        let engine = match Engine::compile(&RuleSet::single(&when)) {
            Ok(e) => e,
            Err(errors) => {
                let _ = send(&mut socket, &json!({ "type": "error", "errors": errors })).await;
                return;
            }
        };
        // Show what the last minutes would have matched, then follow the stream.
        let history: Vec<Tx> = st.recent.lock().unwrap().iter().cloned().collect();
        let mut earlier: Vec<Row> = history.iter().flat_map(|t| engine.process(t)).collect();
        let skip = earlier.len().saturating_sub(200);
        for r in earlier.drain(skip..) {
            if !send(
                &mut socket,
                &json!({ "type": "row", "row": r, "history": true }),
            )
            .await
            {
                return;
            }
        }
        let mut rx = st.txs_tx.subscribe();
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Ok(tx) => for r in engine.process(&tx) { if !send(&mut socket, &json!({ "type": "row", "row": r })).await { return; } },
                    Err(broadcast::error::RecvError::Lagged(n)) => { if !send(&mut socket, &json!({ "type": "lagged", "skipped": n })).await { return; } }
                    Err(_) => return,
                },
                c = socket.recv() => if !matches!(c, Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_)))) { return; },
            }
        }
    }
    let mut rx = st.rows_tx.subscribe();
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(s) => if socket.send(Message::Text(s.to_string().into())).await.is_err() { return; },
                Err(broadcast::error::RecvError::Lagged(n)) => { if !send(&mut socket, &json!({ "type": "lagged", "skipped": n })).await { return; } }
                Err(_) => return,
            },
            c = socket.recv() => if !matches!(c, Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_)))) { return; },
        }
    }
}

/// The routes, with the guards. Tests build requests against this directly.
pub fn router(st: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/app.css", get(app_css))
        .route("/api/health", get(health))
        .route("/api/catalogue", get(get_catalogue))
        .route("/api/rules", get(get_rules).put(put_rules))
        .route("/api/rules/validate", post(validate_rules))
        .route("/api/preview", post(preview))
        .route("/api/rows", get(rows))
        .route("/api/sql", post(sql))
        .route("/api/stats", get(stats))
        .route("/api/gaps", get(gaps))
        .route("/api/live", get(live))
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        .with_state(st)
}

/// Run the collector and the API until the process is told to stop; then flush the database.
pub async fn serve(cfg: Config) -> Result<()> {
    if !cfg.allow_remote && !cfg.bind.ip().is_loopback() {
        anyhow::bail!(
            "refusing to listen on {}: not a loopback address (pass allow_remote to override; the API can edit the rules file)",
            cfg.bind
        );
    }
    let writer = seqscope_store::Writer::spawn(&cfg.db, cfg.writer.clone())
        .with_context(|| format!("opening {}", cfg.db.display()))?;
    let st = AppState::new(
        cfg.rules.clone(),
        cfg.db.clone(),
        writer.ingest(),
        cfg.allow_remote,
    );
    let tasks = collector::spawn(st.clone(), cfg.source);
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("binding {}", cfg.bind))?;
    eprintln!(
        "seqscope: http://{}   rules: {}   database: {}",
        listener.local_addr()?,
        cfg.rules.display(),
        cfg.db.display()
    );
    let result = axum::serve(listener, router(st.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    for t in tasks {
        t.abort();
    }
    drop(st);
    let s = writer.shutdown();
    eprintln!(
        "stored {} rows ({} dropped, {} failed)",
        s.written, s.dropped, s.failed
    );
    result.map_err(Into::into)
}

/// Bind and serve on an existing listener until `stop` resolves (tests, embedding).
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    st: Arc<AppState>,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(st))
        .with_graceful_shutdown(stop)
        .await
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        },
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
