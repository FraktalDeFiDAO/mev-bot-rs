//! The server's API, in-process (requests go straight to the router) plus real WebSocket connections on a local port.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use futures::StreamExt;
use seqscope_core::Tx;
use seqscope_server::{AppState, router};
use seqscope_store::{Reader, Writer, WriterConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

static N: AtomicU32 = AtomicU32::new(0);

struct H {
    dir: PathBuf,
    st: Arc<AppState>,
    writer: Option<Writer>,
}

impl H {
    fn new(rules: Option<&str>, allow_remote: bool) -> H {
        let dir = std::env::temp_dir().join(format!(
            "seqscope-api-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(r) = rules {
            std::fs::write(dir.join("r.toml"), r).unwrap();
        }
        let writer = Writer::spawn(
            dir.join("x.db"),
            WriterConfig {
                flush: Duration::from_millis(20),
                ..Default::default()
            },
        )
        .unwrap();
        let st = AppState::new(
            dir.join("r.toml"),
            dir.join("x.db"),
            writer.ingest(),
            allow_remote,
        );
        H {
            dir,
            st,
            writer: Some(writer),
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:7878");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = if let Some(j) = body {
            b.header("content-type", "application/json")
                .body(Body::from(j.to_string()))
                .unwrap()
        } else {
            b.body(Body::empty()).unwrap()
        };
        let resp = router(self.st.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 10_000_000)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned())),
        )
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Method::GET, path, None, &[]).await
    }
    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, path, Some(body), &[("x-seqscope", "1")])
            .await
    }
    async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, path, Some(body), &[("x-seqscope", "1")])
            .await
    }

    fn feed_corpus(&self, n: usize) {
        for t in corpus().iter().take(n) {
            self.st.feed(t.clone());
        }
    }

    /// Wait until the database holds at least `n` rows.
    fn wait_rows(&self, n: u64) -> u64 {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let c = Reader::open(self.dir.join("x.db"))
                .and_then(|r| r.count())
                .unwrap_or(0);
            if c >= n || Instant::now() > end {
                return c;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for H {
    fn drop(&mut self) {
        if let Some(w) = self.writer.take() {
            let _ = w.shutdown();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn corpus() -> &'static Vec<Tx> {
    static C: std::sync::OnceLock<Vec<Tx>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        include_str!("../../seqscope-core/tests/fixtures/corpus.jsonl")
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    })
}

const STORE_ALL: &str =
    "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"all\"\nwhen = \"true\"\n";
const STORE_SWAPS: &str = "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"swaps\"\nwhen = \"kind == swap\"\n";

#[tokio::test]
async fn the_ui_and_health_are_served() {
    let h = H::new(None, false);
    let (s, v) = h.get("/api/health").await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(true)));
    for (path, needle) in [("/", "seqscope"), ("/app.js", ""), ("/app.css", "")] {
        let resp = router(h.st.clone())
            .oneshot(
                Request::get(path)
                    .header("host", "localhost:7878")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), 10_000_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains(needle) && !body.is_empty(), "{path}");
    }
}

#[tokio::test]
async fn guards_refuse_foreign_hosts_cross_origin_and_unmarked_changes() {
    let h = H::new(None, false);
    // A foreign Host (DNS rebinding) is refused, a loopback one is not.
    let r = router(h.st.clone())
        .oneshot(
            Request::get("/api/health")
                .header("host", "evil.example:7878")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    for ok in ["127.0.0.1:7878", "localhost:1", "[::1]:9"] {
        let r = router(h.st.clone())
            .oneshot(
                Request::get("/api/health")
                    .header("host", ok)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{ok}");
    }
    // A change without the marker header is refused; with it, accepted.
    let (s, _) = h
        .call(
            Method::PUT,
            "/api/rules",
            Some(json!({"text": STORE_ALL})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h.put("/api/rules", json!({"text": STORE_ALL})).await;
    assert_eq!(s, StatusCode::OK);
    // Another site's page cannot read or write, even with the header.
    let (s, _) = h
        .call(
            Method::GET,
            "/api/rules",
            None,
            &[("origin", "https://evil.example")],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h
        .call(
            Method::GET,
            "/api/rules",
            None,
            &[("origin", "http://127.0.0.1:7878")],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "same origin is fine");
    // Remote access only when asked for.
    let remote = H::new(None, true);
    let r = router(remote.st.clone())
        .oneshot(
            Request::get("/api/health")
                .header("host", "10.0.0.5:7878")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn serving_on_a_public_address_is_refused_unless_asked_for() {
    let cfg = |allow| seqscope_server::Config {
        rules: "x.toml".into(),
        db: std::env::temp_dir().join(format!("seqscope-bind-{}.db", std::process::id())),
        bind: "0.0.0.0:0".parse().unwrap(),
        allow_remote: allow,
        source: seqscope_server::Source::Idle,
        writer: WriterConfig::default(),
    };
    let e = seqscope_server::serve(cfg(false))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("not a loopback address"), "{e}");
    let _ = std::fs::remove_file(
        std::env::temp_dir().join(format!("seqscope-bind-{}.db", std::process::id())),
    );
}

#[tokio::test]
async fn the_catalogue_describes_fields_and_protocols() {
    let h = H::new(None, false);
    let (s, v) = h.get("/api/catalogue").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["fields"].as_array().unwrap().len() > 30);
    assert!(
        v["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "aave_v3")
    );
    assert!(
        v["wrappers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "entrypoint_v6")
    );
}

#[tokio::test]
async fn rules_can_be_validated_saved_and_are_never_half_applied() {
    let h = H::new(None, false);
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(
        (v["exists"].as_bool(), v["text"].as_str()),
        (Some(false), Some(""))
    );

    // Validation reports every problem and writes nothing.
    let bad = "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"a\"\nwhen = \"swap.amount > 1\"\n[[profile.rule]]\nname = \"b\"\nwhen = \"kind ==\"\n";
    let (s, v) = h.post("/api/rules/validate", json!({"text": bad})).await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(false)));
    assert_eq!(v["errors"].as_array().unwrap().len(), 2);
    assert!(
        v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("did you mean swap.amount_in")
    );
    assert!(!h.dir.join("r.toml").exists());

    // A TOML mistake is reported too, with the key that is wrong.
    let (_, v) = h
        .post(
            "/api/rules/validate",
            json!({"text": "version = 1\n[[profile]]\nname = \"p\"\nbogus = 1\n"}),
        )
        .await;
    assert_eq!(v["ok"], false);
    assert!(
        v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("bogus"),
        "{v}"
    );

    // A good file is summarised, saved, and takes effect on the next transaction.
    let (_, v) = h
        .post("/api/rules/validate", json!({"text": STORE_SWAPS}))
        .await;
    assert_eq!(v["profiles"][0]["rules"][0]["when"], "kind == swap");
    let (s, _) = h.put("/api/rules", json!({"text": STORE_SWAPS})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(h.dir.join("r.toml")).unwrap(),
        STORE_SWAPS
    );
    h.feed_corpus(600);
    let n = h.wait_rows(1);
    assert!(n > 0);
    let (_, rows) = h.get("/api/rows?limit=5000").await;
    assert!(
        rows.as_array().unwrap().iter().all(|r| r["kind"] == "swap"),
        "only swaps were stored"
    );

    // A bad save is refused, the file and the running rules are untouched.
    let (s, v) = h.put("/api/rules", json!({"text": bad})).await;
    assert_eq!(
        (s, v["ok"].as_bool()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some(false))
    );
    assert_eq!(
        std::fs::read_to_string(h.dir.join("r.toml")).unwrap(),
        STORE_SWAPS
    );
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(v["text"], STORE_SWAPS);
}

#[tokio::test]
async fn edits_made_to_the_file_are_picked_up_and_a_broken_edit_is_reported_not_applied() {
    let h = H::new(Some(STORE_ALL), false);
    h.feed_corpus(100);
    let first = h.wait_rows(100);
    assert!(first >= 100, "{first}");
    // Break the file on disk: the running rules stay, the errors are visible.
    std::fs::write(h.dir.join("r.toml"), "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"a\"\nwhen = \"nonsense ==\"\n").unwrap();
    h.st.reload_rules_file();
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(v["errors"].as_array().unwrap().len(), 1);
    h.feed_corpus(50);
    assert!(
        h.wait_rows(first + 50) >= first + 50,
        "the previous rules kept storing"
    );
    // Fix it: errors clear.
    std::fs::write(h.dir.join("r.toml"), STORE_SWAPS).unwrap();
    h.st.reload_rules_file();
    let (_, v) = h.get("/api/rules").await;
    assert!(v["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_dry_run_counts_what_a_rule_would_match_over_recent_traffic() {
    let h = H::new(None, false);
    let (s, v) = h
        .post("/api/preview", json!({"when": "kind == swap"}))
        .await;
    assert_eq!(
        (s, v["matched"].as_u64()),
        (StatusCode::OK, Some(0)),
        "nothing seen yet"
    );
    h.feed_corpus(800);
    let (_, v) = h
        .post(
            "/api/preview",
            json!({"when": "kind == swap", "samples": 3}),
        )
        .await;
    assert_eq!(v["transactions"], 800);
    assert!(v["matched"].as_u64().unwrap() > 10);
    assert_eq!(v["samples"].as_array().unwrap().len(), 3);
    let (_, none) = h
        .post(
            "/api/preview",
            json!({"when": "kind == swap && swap.amount_in > 1e70"}),
        )
        .await;
    assert_eq!(none["matched"], 0);
    let (s, v) = h
        .post("/api/preview", json!({"when": "swap.nope == 1"}))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
    // Previewing saves nothing.
    assert_eq!(
        Reader::open(h.dir.join("x.db")).unwrap().count().unwrap(),
        0
    );
}

#[tokio::test]
async fn rows_sql_stats_and_gaps_read_what_was_stored() {
    let h = H::new(
        Some(
            "version = 1\n[[profile]]\nname = \"all\"\n[[profile.rule]]\nname = \"every call\"\nwhen = \"true\"\n",
        ),
        false,
    );
    h.feed_corpus(700);
    let stored = h.wait_rows(700);
    assert!(stored >= 700);

    let (_, rows) = h.get("/api/rows?kind=swap&limit=2000").await;
    let swaps = rows.as_array().unwrap().len();
    assert!(swaps > 10 && swaps < stored as usize);
    let (_, one) = h.get("/api/rows?kind=swap&limit=1").await;
    assert_eq!(one.as_array().unwrap().len(), 1);
    let (_, r) = h.get("/api/rows?alerts=true").await;
    assert!(r.as_array().unwrap().is_empty());

    let (s, v) = h
        .post(
            "/api/sql",
            json!({"sql": "SELECT kind, count(*) AS n FROM rows GROUP BY kind ORDER BY n DESC"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["columns"], json!(["kind", "n"]));
    let (s, v) = h.post("/api/sql", json!({"sql": "DELETE FROM rows"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(
        Reader::open(h.dir.join("x.db")).unwrap().count().unwrap(),
        stored,
        "nothing was deleted"
    );
    let (s, _) = h.post("/api/sql", json!({"sql": "SELEC nonsense"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, st) = h.get("/api/stats").await;
    assert_eq!(st["db"]["rows"].as_u64(), Some(stored));
    assert!(st["by_kind"]["rows"].as_array().unwrap().len() > 2);
    assert_eq!(st["collector"]["txs"], 700);
    assert_eq!(st["collector"]["dropped"], 0);

    let (_, g) = h.get("/api/gaps?limit=5").await;
    assert!(g["rows"].as_array().unwrap().len() <= 5 && !g["rows"].as_array().unwrap().is_empty());
    assert_eq!(g["columns"][0], "kind");
}

async fn serve_on_local_port(h: &H) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let st = h.st.clone();
    tokio::spawn(async move {
        let _ = seqscope_server::serve_on(listener, st, async {
            let _ = rx.await;
        })
        .await;
    });
    (port, stop)
}

async fn next_json(
    ws: &mut (
             impl StreamExt<
        Item = Result<
            tokio_tungstenite::tungstenite::Message,
            tokio_tungstenite::tungstenite::Error,
        >,
    > + Unpin
         ),
    within: Duration,
) -> Option<Value> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(Some(Ok(m))) if m.is_text() => {
                return serde_json::from_str(m.to_text().unwrap()).ok();
            }
            Ok(Some(Ok(_))) => {}
            Ok(_) => return None,
            Err(_) => {}
        }
    }
    None
}

#[tokio::test]
async fn the_live_socket_streams_stored_rows_and_status() {
    let h = H::new(Some(STORE_SWAPS), false);
    let (port, stop) = serve_on_local_port(&h).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/api/live"))
        .await
        .unwrap();
    let st = h.st.clone();
    tokio::task::spawn_blocking(move || {
        for t in corpus().iter().take(700) {
            st.feed(t.clone());
        }
    });
    let mut rows = 0;
    let end = Instant::now() + Duration::from_secs(10);
    while rows < 5 && Instant::now() < end {
        if let Some(m) = next_json(&mut ws, Duration::from_secs(2)).await
            && m["type"] == "row"
        {
            assert_eq!(m["row"]["kind"], "swap");
            assert_eq!(m["row"]["status"], "pending");
            rows += 1;
        }
    }
    assert!(rows >= 5, "got {rows} rows");
    let _ = stop.send(());
}

#[tokio::test]
async fn a_probe_tries_a_rule_on_history_and_then_on_the_live_stream_without_saving() {
    let h = H::new(None, false); // no stored rules at all
    h.feed_corpus(500);
    let (port, stop) = serve_on_local_port(&h).await;
    let url = format!("ws://127.0.0.1:{port}/api/live?mode=probe&when=kind%20%3D%3D%20swap");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let first = next_json(&mut ws, Duration::from_secs(3))
        .await
        .expect("history rows arrive at once");
    assert_eq!(
        (first["type"].as_str(), first["history"].as_bool()),
        (Some("row"), Some(true))
    );
    // New traffic after connecting is matched live.
    let st = h.st.clone();
    tokio::task::spawn_blocking(move || {
        for t in corpus().iter().skip(500).take(600) {
            st.feed(t.clone());
        }
    });
    let mut live = 0;
    let end = Instant::now() + Duration::from_secs(10);
    while live < 3 && Instant::now() < end {
        if let Some(m) = next_json(&mut ws, Duration::from_secs(2)).await
            && m["type"] == "row"
            && m["history"].is_null()
        {
            assert_eq!(m["row"]["kind"], "swap");
            live += 1;
        }
    }
    assert!(live >= 3, "{live}");
    assert_eq!(
        Reader::open(h.dir.join("x.db")).unwrap().count().unwrap(),
        0,
        "a probe stores nothing"
    );

    // A broken rule is explained over the socket, then the connection closes.
    let (mut bad, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/api/live?mode=probe&when=swap.nope%20%3D%3D%201"
    ))
    .await
    .unwrap();
    let m = next_json(&mut bad, Duration::from_secs(3)).await.unwrap();
    assert_eq!(m["type"], "error");
    assert!(
        m["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn a_socket_from_another_site_is_refused() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let h = H::new(None, false);
    let (port, stop) = serve_on_local_port(&h).await;
    let mut req = format!("ws://127.0.0.1:{port}/api/live")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("403"), "{err}");
    let _ = stop.send(());
}

#[tokio::test]
async fn a_replayed_recording_drives_the_whole_pipeline_with_no_network() {
    let h = H::new(Some(STORE_ALL), false);
    let file = h.dir.join("rec.jsonl");
    std::fs::write(
        &file,
        include_str!("../../seqscope-core/tests/fixtures/corpus.jsonl"),
    )
    .unwrap();
    let tasks = {
        // The collector module is private; the public way in is `serve` with a replay source.
        let cfg = seqscope_server::Config {
            rules: h.dir.join("r.toml"),
            db: h.dir.join("served.db"),
            bind: "127.0.0.1:0".parse().unwrap(),
            allow_remote: false,
            source: seqscope_server::Source::Replay { file, speed: 0.0 },
            writer: WriterConfig {
                flush: Duration::from_millis(20),
                ..Default::default()
            },
        };
        tokio::spawn(seqscope_server::serve(cfg))
    };
    let end = Instant::now() + Duration::from_secs(20);
    let db = h.dir.join("served.db");
    let mut n = 0;
    while Instant::now() < end {
        n = Reader::open(&db).and_then(|r| r.count()).unwrap_or(0);
        if n > 2_000 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tasks.abort();
    assert!(n > 2_000, "the replay stored only {n} rows");
}

// ---------------------------------------------------------------------------------------------------------------
// The guided interface: plain English, a readable rule list, edits without the text

#[tokio::test]
async fn any_rule_can_be_explained_in_plain_english_and_a_bad_one_says_why() {
    let h = H::new(None, false);
    let (s, v) = h.post("/api/explain", json!({"when": "kind == swap && swap.token_in == 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 && swap.amount_in >= 10000e6"})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["text"],
        "Catches a call when it is a swap and it sells at least 10,000 USDC."
    );
    let (s, v) = h.post("/api/explain", json!({"when": "kind =="})).await;
    assert_eq!(
        (s, v["ok"].as_bool()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some(false))
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("expected a value")
    );
    assert_eq!(v["error"]["column"], 8);
    let (s, v) = h
        .post("/api/explain", json!({"when": "swap.amount > 1"}))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("did you mean swap.amount_in")
    );
}

#[tokio::test]
async fn the_catalogue_carries_names_for_pickers() {
    let h = H::new(None, false);
    let (_, v) = h.get("/api/catalogue").await;
    assert!(
        v["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["symbol"] == "USDC" && t["decimals"] == 6)
    );
    assert!(
        v["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "aave_v3" && p["label"] == "Aave V3")
    );
    assert!(
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| !f["label"].as_str().unwrap().is_empty())
    );
    assert!(
        v["functions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f[0] == "a415bcad" && f[1] == "Aave: borrow")
    );
    assert!(v["wrapper_labels"].as_array().unwrap().len() >= 5);
}

async fn op(h: &H, body: Value) -> (StatusCode, Value) {
    h.post("/api/rules/op", body).await
}

fn new_rule(name: &str, when: &str) -> Value {
    json!({"name": name, "when": when})
}

#[tokio::test]
async fn rules_are_added_switched_changed_and_removed_without_touching_text() {
    let h = H::new(None, false);
    // Add: the file is created, the running rules switch, and the list says what the rule means.
    let (s, v) = op(&h, json!({"op": "add_rule", "profile": "My rules", "rule": new_rule("big swaps", "kind == swap && swap.amount_in > 1e15")})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        std::fs::read_to_string(h.dir.join("r.toml"))
            .unwrap()
            .contains("big swaps")
    );
    h.feed_corpus(600);
    assert!(h.wait_rows(1) > 0);
    let (_, v) = h.get("/api/rules").await;
    let rule = &v["rules"][0];
    assert_eq!(
        (
            rule["profile"].as_str(),
            rule["name"].as_str(),
            rule["enabled"].as_bool()
        ),
        (Some("My rules"), Some("big swaps"), Some(true))
    );
    assert!(
        rule["explain"]
            .as_str()
            .unwrap()
            .starts_with("Catches a call when it is a swap")
    );
    assert!(rule["matches"].as_u64().unwrap() > 0, "{rule}");
    assert!(rule["stored"].as_u64().unwrap() > 0);

    // Refusals: a duplicate name, an unknown field, an unknown rule: each says why and changes nothing.
    let before = std::fs::read_to_string(h.dir.join("r.toml")).unwrap();
    for (body, needle) in [
        (
            json!({"op": "add_rule", "profile": "My rules", "rule": new_rule("big swaps", "true")}),
            "already a rule named",
        ),
        (
            json!({"op": "add_rule", "profile": "My rules", "rule": new_rule("x", "swap.amount > 1")}),
            "did you mean swap.amount_in",
        ),
        (
            json!({"op": "add_rule", "profile": "My rules", "rule": new_rule("  ", "true")}),
            "needs a name",
        ),
        (
            json!({"op": "set_enabled", "profile": "My rules", "rule": "ghost", "enabled": false}),
            "no rule",
        ),
        (
            json!({"op": "remove_rule", "profile": "nope", "rule": "big swaps"}),
            "no profile",
        ),
        (
            json!({"op": "add_template", "name": "nope"}),
            "no starter set",
        ),
    ] {
        let (s, v) = op(&h, body).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert!(v["error"].as_str().unwrap().contains(needle), "{v}");
    }
    assert_eq!(
        std::fs::read_to_string(h.dir.join("r.toml")).unwrap(),
        before
    );

    // Switch off: it stops catching, and the list still shows it, off.
    let (s, _) = op(
        &h,
        json!({"op": "set_enabled", "profile": "My rules", "rule": "big swaps", "enabled": false}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let count = Reader::open(h.dir.join("x.db")).unwrap().count().unwrap();
    h.feed_corpus(600);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        Reader::open(h.dir.join("x.db")).unwrap().count().unwrap(),
        count,
        "a switched-off rule stores nothing"
    );
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(v["rules"][0]["enabled"], false);

    // Change it, then remove it.
    let (s, _) = op(&h, json!({"op": "update_rule", "profile": "My rules", "rule": "big swaps", "patch": {"name": "huge swaps", "action": "alert", "tag": "whale", "enabled": true}})).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = h.get("/api/rules").await;
    let r = &v["rules"][0];
    assert_eq!(
        (
            r["name"].as_str(),
            r["action"].as_str(),
            r["tag"].as_str(),
            r["enabled"].as_bool()
        ),
        (Some("huge swaps"), Some("alert"), Some("whale"), Some(true))
    );
    let (s, _) = op(
        &h,
        json!({"op": "remove_rule", "profile": "My rules", "rule": "huge swaps"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = h.get("/api/rules").await;
    assert!(v["rules"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn starter_sets_are_listed_in_words_and_added_once() {
    let h = H::new(None, false);
    let (_, v) = h.get("/api/templates").await;
    let list = v.as_array().unwrap();
    assert!(list.len() >= 6);
    let aave = list.iter().find(|t| t["name"] == "aave-lending").unwrap();
    assert_eq!(aave["new"], 5);
    assert!(
        aave["rules"][1]["explain"]
            .as_str()
            .unwrap()
            .contains("Aave: borrow"),
        "{aave}"
    );
    let (s, v) = op(&h, json!({"op": "add_template", "name": "aave-lending"})).await;
    assert_eq!(
        (s, v["added"].as_u64(), v["skipped"].as_u64()),
        (StatusCode::OK, Some(5), Some(0))
    );
    let (_, v) = op(&h, json!({"op": "add_template", "name": "aave-lending"})).await;
    assert_eq!(
        (v["added"].as_u64(), v["skipped"].as_u64()),
        (Some(0), Some(5)),
        "adding it again changes nothing"
    );
    let (_, v) = h.get("/api/templates").await;
    assert_eq!(
        v.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "aave-lending")
            .unwrap()["new"],
        0
    );
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(v["rules"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn a_guided_edit_keeps_the_notes_at_the_top_of_the_file_and_refuses_a_broken_file() {
    let h = H::new(
        Some(
            "# my notes: keep these\n# second line\n\nversion = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"a\"\nwhen = \"kind == swap\"\n",
        ),
        false,
    );
    let (s, v) = op(
        &h,
        json!({"op": "add_rule", "profile": "p", "rule": new_rule("b", "kind == liquidity")}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let text = std::fs::read_to_string(h.dir.join("r.toml")).unwrap();
    assert!(
        text.starts_with("# my notes: keep these\n# second line\n"),
        "{text}"
    );
    assert!(text.contains("name = \"b\"") && text.contains("name = \"a\""));
    // A file that does not even parse cannot be edited this way, and is not overwritten.
    std::fs::write(h.dir.join("r.toml"), "version = 1\n[[profile]\nbroken").unwrap();
    h.st.reload_rules_file();
    let (s, v) = op(
        &h,
        json!({"op": "add_rule", "profile": "p", "rule": new_rule("c", "true")}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("fixed in the text view"),
        "{v}"
    );
    assert_eq!(
        std::fs::read_to_string(h.dir.join("r.toml")).unwrap(),
        "version = 1\n[[profile]\nbroken"
    );
}

#[tokio::test]
async fn simultaneous_edits_all_land() {
    let h = Arc::new(H::new(None, false));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let h = h.clone();
        tasks.push(tokio::spawn(async move { op(&h, json!({"op": "add_rule", "profile": "p", "rule": new_rule(&format!("rule {i}"), "kind == swap")})).await.0 }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }
    let (_, v) = h.get("/api/rules").await;
    assert_eq!(
        v["rules"].as_array().unwrap().len(),
        8,
        "no edit overwrote another"
    );
}
