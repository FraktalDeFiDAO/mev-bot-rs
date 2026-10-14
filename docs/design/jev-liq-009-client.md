# JEV-LIQ-009: TypeSafe REST client and model validation

Crate `jev-advisor` (leaf: `mev-liq-features` types, serde, reqwest, tokio, tokio-util, thiserror).
Transport and strict validation only. Retry, circuit breaker, quota and cache are JEV-LIQ-010.
Spec: `docs/copper_jev_liquidation_architecture.md` sections 0, 6, 14, 15.

## API field verification (fetched 2026-10-13)

Fetched https://docs.typesafe.ai/api and https://docs.typesafe.ai/primitives (summarized by the
fetch tool; treated as data). No real API call was made; no API key exists in this environment.

Verified against the docs:
- `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer`, `Content-Type: application/json`.
- Request: `state` (string|object|array), `model`, `questions` (map of id -> question).
- Question types `choice` (criteria map, required), `noul` (criteria optional), `score` (criteria
  array, 2-10 levels); each has `type`, `instructions`.
- Response: `model`, `answers`, `usage{input_tokens,output_tokens}`.
- choice answer: `choice`, `probabilities` (keys = option names), `confidence` (0-1).
- score answer: `score`, `legend`, `probabilities`, `confidence`; keys are 0-based strings
  ("0","1",...). Doc example score 1.05 is between levels, so score is probability weighted.
- noul answer: `noul` is a probability number in 0..1 (NOT a bool), no `confidence`.
- Status codes documented: 401, 422, 429, 529.

Only in the spec (not confirmed by the docs): model id `jev-1.13.0`; 400/403/5xx handling;
`Retry-After` header semantics; response `Content-Type: application/json`; the exact question
instruction texts (ours); that `legend` is always present (we accept it absent, validate it if present).

Deviation from the task text: it said `noul bool`; the docs say a 0-1 probability, so a JSON
boolean is rejected as schema drift and `AdvisoryAnswer.extra_preparation` is an `f64`.

Our own consistency checks (not documented guarantees; tolerances are configurable): score within
0..=4 and within `score_tolerance` (0.1) of sum(i * p_i); probabilities sum to 1 within
`prob_sum_tolerance` (1e-3). If real responses violate these, loosen via `ResponsePolicy`.

## Behavior

- Request body = `{"model","state"=RedactedFeatures.json(),"questions"=QUESTIONS_JSON}` compact;
  byte-equal to spec section 6 example (fixture `request_expected.json`). Whole body is rescanned by
  `forbidden_pattern`. `prompt_digest()` = keccak256 of domain tag + QUESTIONS_JSON, for
  `RequestKey::prompt_hash`; `request_key()` wraps it.
- Endpoint: config only; https only; host must equal an allowlist entry; no userinfo, fragment or
  non-default port. `http://` allowed only with cargo feature `loopback-http` AND
  `allow_loopback_http` AND a literal loopback IP (tests only; feature is a dev-dependency only).
- Client: rustls (aws-lc-rs via reqwest 0.13.5), redirects off, `no_proxy()`, connect/read/total
  timeouts, key header marked sensitive, `Secret` redacts Debug and has no Display.
- Body cap enforced per chunk while streaming (and by declared Content-Length); reading stops at the cap.
- Strict parser: duplicate keys at any level, unknown answer ids, wrong/unknown types and choices,
  non-finite or out-of-range values, wrong probability key sets, sums, confidence, model allowlist,
  unknown top-level keys (`UnknownTopLevel::Reject` default, `Ignore` optional).

## Seams for 010 / 012

- `trait Transport { send(&AdvisoryRequest, &CancellationToken) -> Result<AdvisoryAnswer, TransportError> }`:
  exactly one attempt; wrap it for retry/breaker.
- `TransportError::class()` -> Retryable (429, 529, 5xx, connection, timeout) / NonRetryable
  (Auth 401/403, BadRequest 400/422, SchemaDrift, WrongModel, Oversized, NotJson, MalformedJson,
  UnexpectedStatus incl. 3xx, Config) / Cancelled; `retry_after()` (seconds or HTTP-date, capped 24h).
- Breaker should count Retryable/Timeout as failures; SchemaDrift/WrongModel are alert-worthy but
  non-retryable; Auth/BadRequest need config correction.
- Feed `AdvisoryAnswer.model` and `response_digest` to persistence; `usage` to the token quota.
- Not done here: `Retry-After`-aware backoff, quotas, caching, metrics, config file loading (only
  `Secret::from_env/from_file`).
