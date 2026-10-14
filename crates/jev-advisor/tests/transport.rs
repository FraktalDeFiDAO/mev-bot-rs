mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use common::*;
use jev_advisor::*;
use mev_liq_features::forbidden_pattern;

async fn run(script: Script) -> (Result<AdvisoryAnswer, TransportError>, TestServer) {
    run_with(script, |_| {}).await
}

async fn run_with(
    script: Script,
    tweak: impl FnOnce(&mut ClientConfig),
) -> (Result<AdvisoryAnswer, TransportError>, TestServer) {
    let (r, server, _) = run_timed(script, tweak).await;
    (r, server)
}

/// Like `run_with`, but also returns how long `send` alone took. Server spawn, client
/// construction (TLS setup) and request building are excluded: under a loaded machine they
/// can take seconds and are not what the deadline bounds.
async fn run_timed(
    script: Script,
    tweak: impl FnOnce(&mut ClientConfig),
) -> (Result<AdvisoryAnswer, TransportError>, TestServer, Duration) {
    let server = TestServer::spawn(script).await;
    let mut c = cfg(server.addr);
    tweak(&mut c);
    let client = TypeSafeClient::new(c).unwrap();
    let req = build_request(&features(), MODEL).unwrap();
    let t = Instant::now();
    let r = client.send(&req, &CancellationToken::new()).await;
    (r, server, t.elapsed())
}

fn status(code: &str) -> Script {
    Script::Raw(http(code, &[("Content-Type", "application/json")], b"{}"))
}

#[tokio::test]
async fn valid_response_and_exact_request() {
    let body = fixture("valid.json");
    let (r, server) = run(json_ok(&body)).await;
    let a = r.unwrap();
    assert_eq!(a.model, MODEL);
    assert_eq!(a.watch_tier.choice, WatchTier::High);
    assert_eq!(a.watch_tier.probabilities, [0.7, 0.2, 0.1]);
    assert_eq!(a.extra_preparation, 0.35);
    assert_eq!(a.monitoring_value.score, 2.6);
    assert_eq!(
        a.usage,
        Usage {
            input_tokens: 412,
            output_tokens: 31
        }
    );
    assert_eq!(a.response_digest, response_digest(&body));

    let reqs = server.recorded();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert!(
        r.request_line.starts_with("POST /v1/systemone "),
        "{}",
        r.request_line
    );
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    // Byte-equal to the spec section 6 example (compacted), recorded in a fixture.
    assert_eq!(r.body, fixture("request_expected.json"));
}

#[tokio::test]
async fn content_type_with_charset_is_accepted() {
    let body = fixture("valid.json");
    let s = Script::Raw(http(
        "200 OK",
        &[("Content-Type", "Application/JSON; charset=utf-8")],
        &body,
    ));
    assert!(run(s).await.0.is_ok());
}

#[test]
fn request_carries_no_identifiers_or_secrets() {
    let req = build_request(&features(), MODEL).unwrap();
    let body = std::str::from_utf8(req.body()).unwrap();
    assert!(forbidden_pattern(body).is_none());
    assert!(!body.contains("0x") && !body.contains("://") && !body.contains(KEY));
    let mut run = 0;
    for b in body.bytes() {
        run = if b.is_ascii_hexdigit() { run + 1 } else { 0 };
        assert!(run < 40, "40+ hex run in request body");
    }
    // Digest and key are stable and bound to the exact instructions.
    assert_eq!(req.key().prompt_hash, prompt_digest());
    assert_eq!(req.key().model, MODEL);
    assert!(build_request(&features(), "bad model!").is_err());
}

#[tokio::test]
async fn http_status_mapping() {
    type Check = fn(&TransportError) -> bool;
    let cases: &[(&str, Check)] = &[
        ("400 Bad Request", |e| {
            matches!(e, TransportError::BadRequest(400))
        }),
        ("422 Unprocessable Entity", |e| {
            matches!(e, TransportError::BadRequest(422))
        }),
        ("401 Unauthorized", |e| {
            matches!(e, TransportError::Auth(401))
        }),
        ("403 Forbidden", |e| matches!(e, TransportError::Auth(403))),
        ("404 Not Found", |e| {
            matches!(e, TransportError::UnexpectedStatus(404))
        }),
        ("429 Too Many Requests", |e| {
            matches!(
                e,
                TransportError::Retryable {
                    kind: RetryableKind::RateLimited,
                    ..
                }
            )
        }),
        ("529 Overloaded", |e| {
            matches!(
                e,
                TransportError::Retryable {
                    kind: RetryableKind::Overloaded,
                    ..
                }
            )
        }),
        ("500 Internal Server Error", |e| {
            matches!(
                e,
                TransportError::Retryable {
                    kind: RetryableKind::Server(500),
                    ..
                }
            )
        }),
        ("503 Service Unavailable", |e| {
            matches!(
                e,
                TransportError::Retryable {
                    kind: RetryableKind::Server(503),
                    ..
                }
            )
        }),
    ];
    for (line, check) in cases {
        let (r, _) = run(status(line)).await;
        let e = r.unwrap_err();
        assert!(check(&e), "{line} -> {e:?}");
        let expect = if matches!(line.as_bytes()[0], b'4') && !line.starts_with("429") {
            ErrorClass::NonRetryable
        } else {
            ErrorClass::Retryable
        };
        assert_eq!(e.class(), expect, "{line}");
    }
}

#[tokio::test]
async fn retry_after_seconds_and_date() {
    let s = Script::Raw(http("429 Too Many Requests", &[("Retry-After", "7")], b""));
    let e = run(s).await.0.unwrap_err();
    assert_eq!(e.retry_after(), Some(Duration::from_secs(7)));

    let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
    let s = Script::Raw(http("529 Overloaded", &[("Retry-After", &future)], b""));
    let d = run(s).await.0.unwrap_err().retry_after().unwrap();
    assert!(
        d > Duration::from_secs(100) && d <= Duration::from_secs(120),
        "{d:?}"
    );

    let s = Script::Raw(http("503 Service Unavailable", &[], b""));
    assert_eq!(run(s).await.0.unwrap_err().retry_after(), None);
}

#[test]
fn retry_after_parser() {
    let now = SystemTime::now();
    assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
    assert_eq!(
        parse_retry_after(" 30 ", now),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        parse_retry_after("99999999999999999999999", now),
        Some(MAX_RETRY_AFTER)
    );
    assert_eq!(parse_retry_after("999999999", now), Some(MAX_RETRY_AFTER));
    assert_eq!(parse_retry_after("-5", now), None);
    assert_eq!(parse_retry_after("soon", now), None);
    assert_eq!(parse_retry_after("", now), None);
    let past = httpdate::fmt_http_date(now - Duration::from_secs(60));
    assert_eq!(parse_retry_after(&past, now), Some(Duration::ZERO));
    let fut = httpdate::fmt_http_date(now + Duration::from_secs(45));
    let d = parse_retry_after(&fut, now).unwrap();
    assert!(d >= Duration::from_secs(43) && d <= Duration::from_secs(45));
}

#[tokio::test]
async fn redirect_is_refused_and_target_never_contacted() {
    let target = TestServer::spawn(json_ok(&fixture("valid.json"))).await;
    let loc = format!("http://{}/steal", target.addr);
    let s = Script::Raw(http("302 Found", &[("Location", &loc)], b""));
    let (r, origin) = run(s).await;
    assert_eq!(r.unwrap_err(), TransportError::UnexpectedStatus(302));
    assert_eq!(origin.recorded().len(), 1);
    assert_eq!(target.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn non_json_content_type_rejected() {
    let body = fixture("valid.json");
    let s = Script::Raw(http("200 OK", &[("Content-Type", "text/html")], &body));
    assert_eq!(run(s).await.0.unwrap_err(), TransportError::NotJson);
    let s = Script::Raw(http("200 OK", &[], &body));
    assert_eq!(run(s).await.0.unwrap_err(), TransportError::NotJson);
}

#[tokio::test]
async fn contract_violations_over_the_wire() {
    for (name, ok) in [
        (
            "malformed.json",
            (|e: &TransportError| *e == TransportError::MalformedJson)
                as fn(&TransportError) -> bool,
        ),
        ("wrong_model.json", |e| {
            matches!(e, TransportError::WrongModel { .. })
        }),
        ("schema_drift.json", |e| {
            matches!(e, TransportError::SchemaDrift(_))
        }),
        ("nan_literal.json", |e| *e == TransportError::MalformedJson),
        ("probs_not_sum.json", |e| {
            matches!(
                e,
                TransportError::SchemaDrift(ValidationError::DistributionSum(_))
            )
        }),
        ("dup_answer.json", |e| {
            matches!(
                e,
                TransportError::SchemaDrift(ValidationError::DuplicateKey(_))
            )
        }),
        ("missing_answer.json", |e| {
            matches!(
                e,
                TransportError::SchemaDrift(ValidationError::MissingAnswer(_))
            )
        }),
        ("extra_answer.json", |e| {
            matches!(
                e,
                TransportError::SchemaDrift(ValidationError::UnexpectedAnswer(_))
            )
        }),
    ] {
        let e = run(json_ok(&fixture(name))).await.0.unwrap_err();
        assert!(ok(&e), "{name} -> {e:?}");
        assert_eq!(e.class(), ErrorClass::NonRetryable, "{name}");
    }
}

#[tokio::test]
async fn oversized_declared_length_rejected() {
    let s = Script::Raw(http(
        "200 OK",
        &[("Content-Type", "application/json")],
        &vec![b' '; 40_000],
    ));
    assert_eq!(
        run(s).await.0.unwrap_err(),
        TransportError::Oversized { limit: 16 * 1024 }
    );
}

#[tokio::test]
async fn oversized_stream_stops_at_cap() {
    let total = 50 * 1024 * 1024;
    let (r, server) = run(Script::Flood { chunk: 8192, total }).await;
    assert_eq!(
        r.unwrap_err(),
        TransportError::Oversized { limit: 16 * 1024 }
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sent = server.flood_sent.load(Ordering::SeqCst);
    assert!(
        sent < total,
        "client kept reading the whole flood ({sent} bytes)"
    );
}

#[tokio::test]
async fn silent_server_hits_the_deadline() {
    let (r, _, el) = run_timed(Script::Hang, |c| {
        c.total_timeout = Duration::from_millis(300);
        c.read_timeout = Duration::from_millis(250);
    })
    .await;
    let e = r.unwrap_err();
    assert!(matches!(e, TransportError::Timeout(_)), "{e:?}");
    assert_eq!(e.class(), ErrorClass::Retryable);
    assert!(
        el >= Duration::from_millis(200) && el < Duration::from_secs(5),
        "{el:?}"
    );
}

#[tokio::test]
async fn slowloris_body_hits_total_deadline() {
    let body = fixture("valid.json");
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    // One byte every 100ms is well inside the 1s read timeout; only the total deadline stops it.
    let (r, _, el) = run_timed(
        Script::Drip {
            head,
            delay: Duration::from_millis(100),
            body,
        },
        |c| {
            c.total_timeout = Duration::from_millis(500);
            c.read_timeout = Duration::from_secs(1);
        },
    )
    .await;
    assert!(matches!(r.unwrap_err(), TransportError::Timeout(_)));
    assert!(el < Duration::from_secs(5), "{el:?}");
}

#[tokio::test]
async fn cancellation_aborts_promptly() {
    let server = TestServer::spawn(Script::Hang).await;
    let mut c = cfg(server.addr);
    c.total_timeout = Duration::from_secs(30);
    c.read_timeout = Duration::from_secs(30);
    let client = TypeSafeClient::new(c).unwrap();
    let req = build_request(&features(), MODEL).unwrap();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        c2.cancel();
    });
    let t = Instant::now();
    let e = client.send(&req, &cancel).await.unwrap_err();
    assert_eq!(e, TransportError::Cancelled);
    assert_eq!(e.class(), ErrorClass::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[tokio::test]
async fn pre_cancelled_never_connects() {
    let server = TestServer::spawn(json_ok(&fixture("valid.json"))).await;
    let client = TypeSafeClient::new(cfg(server.addr)).unwrap();
    let req = build_request(&features(), MODEL).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client.send(&req, &cancel).await.unwrap_err(),
        TransportError::Cancelled
    );
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn refused_connection_is_retryable_transport_error() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let client = TypeSafeClient::new(cfg(addr)).unwrap();
    let req = build_request(&features(), MODEL).unwrap();
    let e = client
        .send(&req, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            TransportError::Retryable {
                kind: RetryableKind::Connection,
                retry_after: None
            }
        ),
        "{e:?}"
    );
}

#[tokio::test]
async fn endpoint_policy_refuses_before_any_connection() {
    let server = TestServer::spawn(json_ok(&fixture("valid.json"))).await;
    let addr = server.addr;
    let hosts = vec!["api.typesafe.ai".to_string()];
    let bad = [
        "http://api.typesafe.ai/v1/systemone".to_string(),
        "https://api.typesafe.ai.evil.example/v1/systemone".to_string(),
        "https://api.typesafe.ai@evil.example/v1/systemone".to_string(),
        "https://user:pw@api.typesafe.ai/v1/systemone".to_string(),
        "https://api.typesafe.ai:8443/v1/systemone".to_string(),
        "https://api.typesafe.ai/v1/systemone#frag".to_string(),
        "https://other.example/v1/systemone".to_string(),
        "ftp://api.typesafe.ai/".to_string(),
        "not a url".to_string(),
        format!("http://{addr}/v1/systemone"),
    ];
    for url in &bad {
        let mut c = cfg(addr);
        c.endpoint = url.clone();
        c.allowed_hosts = hosts.clone();
        c.allow_loopback_http = false;
        let e = TypeSafeClient::new(c).unwrap_err();
        assert!(matches!(e, TransportError::Config(_)), "{url}");
        assert!(
            !format!("{e} {e:?}").contains("pw"),
            "credentials leaked for {url}"
        );
    }
    // Loopback http is only allowed with the explicit flag AND an allowlisted loopback IP.
    let mut c = cfg(addr);
    c.allowed_hosts = hosts;
    assert!(TypeSafeClient::new(c).is_err());
    assert!(
        validate_endpoint(
            "https://API.TypeSafe.ai/v1/systemone",
            &["api.typesafe.ai".into()],
            false
        )
        .is_ok()
    );
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
}

#[test]
fn config_rejects_unlisted_model_and_bad_limits() {
    let addr = "127.0.0.1:1".parse().unwrap();
    let mut c = cfg(addr);
    c.model = "jev-latest".into();
    assert!(matches!(
        TypeSafeClient::new(c),
        Err(TransportError::Config(_))
    ));
    let mut c = cfg(addr);
    c.max_response_bytes = 0;
    assert!(TypeSafeClient::new(c).is_err());
    let mut c = cfg(addr);
    c.total_timeout = Duration::ZERO;
    assert!(TypeSafeClient::new(c).is_err());
}

#[tokio::test]
async fn request_model_must_match_client_model() {
    let server = TestServer::spawn(json_ok(&fixture("valid.json"))).await;
    let mut c = cfg(server.addr);
    c.response = ResponsePolicy::new(vec![MODEL.into(), "jev-1.14.0".into()]);
    let client = TypeSafeClient::new(c).unwrap();
    let req = build_request(&features(), "jev-1.14.0").unwrap();
    let e = client
        .send(&req, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, TransportError::Config(_)));
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn api_key_never_appears_in_debug_display_or_errors() {
    let (_, server) = run(json_ok(&fixture("valid.json"))).await;
    let c = cfg(server.addr);
    let mut seen = vec![format!("{c:?}"), format!("{:?}", c.api_key)];
    let client = TypeSafeClient::new(c).unwrap();
    seen.push(format!("{client:?}"));
    seen.push(format!("{:?}", build_request(&features(), MODEL).unwrap()));
    for script in [
        status("401 Unauthorized"),
        status("500 Internal Server Error"),
        json_ok(&fixture("wrong_model.json")),
        json_ok(&fixture("valid.json")),
        Script::Raw(b"garbage".to_vec()),
    ] {
        match run(script).await.0 {
            Ok(a) => seen.push(format!("{a:?}")),
            Err(e) => seen.push(format!("{e} | {e:?}")),
        }
    }
    for s in seen {
        assert!(
            !s.contains(KEY) && !s.contains("9f8e7d6c5b4a"),
            "key leaked in: {s}"
        );
    }
}

#[test]
fn secret_loading_and_redaction() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("key");
    std::fs::write(&p, format!("{KEY}\n")).unwrap();
    let s = Secret::from_file(&p).unwrap();
    assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
    assert!(Secret::from_file(&dir.path().join("missing")).is_err());
    let e = Secret::from_file(&dir.path().join("missing"))
        .unwrap_err()
        .to_string();
    assert!(!e.contains(dir.path().to_str().unwrap()));
    std::fs::write(&p, "two words\n").unwrap();
    assert!(Secret::from_file(&p).is_err());
    assert!(Secret::new("   ").is_err());
    assert!(Secret::from_env("JEV_ADVISOR_TEST_SURELY_UNSET_VAR").is_err());
}

/// Real API smoke test. Skipped by default; never run in CI. Needs a funded key.
#[tokio::test]
#[ignore = "needs TYPESAFE_API_KEY"]
async fn real_api_smoke() {
    let Ok(key) = Secret::from_env("TYPESAFE_API_KEY") else {
        eprintln!("TYPESAFE_API_KEY not set; skipping");
        return;
    };
    let model = "jev-1.13.0";
    let client =
        TypeSafeClient::new(ClientConfig::new(key, model, vec![model.to_string()])).unwrap();
    let req = build_request(&features(), model).unwrap();
    let a = client.send(&req, &CancellationToken::new()).await.unwrap();
    assert_eq!(a.model, model);
}
