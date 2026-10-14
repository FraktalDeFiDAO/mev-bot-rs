# JEV-LIQ-010: retry, breaker, quota, cache around the Jev transport

Module `jev_advisor::resilience` (no new crate, no new dependency). `ResilientClient<T: Transport, C: Clock, J: JitterSource>`
wraps the one-attempt `Transport` of 009 and never changes its semantics. Spec: sections 0 (invariants 1, 6), 5, 6 (HTTP
reliability), 11, 14, 15 of `docs/copper_jev_liquidation_architecture.md`. Advisory only: every failure becomes a typed
`Unavailable{reason}` the caller maps to the deterministic baseline (spec invariant 1). No variant blocks past the deadline.

## API

```
ResilientClient::new(transport, clock, rng, ResilienceConfig) -> Result<_, ConfigError>
client.call(&AdvisoryRequest, deadline: Duration /*work item's remaining TTL*/, &CancellationToken) -> Outcome
Outcome { result: Result<Arc<AdvisoryAnswer>, Unavailable>, source: Network|Cache|Coalesced|Local, attempts, elapsed }
Unavailable { reason: FallbackReason, last_error: Option<TransportError> }
client.snapshot() -> MetricsSnapshot ; client.config_changed()
```

Business validity (account revision, digest checks) stays with the caller (008/011): a cached or coalesced answer is only
"the same prompt+features+model answered recently", never "still valid for this account state".

## Defaults (all in `ResilienceConfig::default()`)

| Knob | Default | Spec |
|---|---|---|
| `attempt_timeout` | 1500 ms (clamped to time left before the deadline) | `jev_timeout_ms` |
| `max_inflight` / `inflight_policy` | 4 / `Wait` (bounded by the deadline; `FailFast` available) | `jev_max_inflight` |
| `retry.max_attempts` | 3 (first attempt included) | |
| `retry.base_delay` / `max_delay` / `jitter_permille` | 100 ms / 2 s / 500 | |
| `breaker.consecutive_failures` / `cooldown` / `ratio` | 5 / 30 s / off (`RatioTrip{permille, window, min_samples}`) | |
| `quota.requests_per_window` / `window` | 200 / 3600 s | `jev_budget_per_hour` |
| `quota.tokens_per_window` | `None` (not enforced) | no number in spec |
| `quota.usd_micro_per_window` + `quota.prices` | `None` / `None` (disabled) | |
| `cache.ttl` / `max_entries` / `negative_ttl` | 60 s / 1024 / 5 s | `advisor_ttl_seconds` |

`validate()` rejects zero attempts/inflight/timeouts, jitter > 1000, and a USD budget without prices. TypeSafe prices are not
published in the spec and are never invented: set `quota.prices` (integer micro-USD per million input/output tokens) and
`usd_micro_per_window` together, or neither. Without prices, `estimated_cost_micro_usd_total` stays 0.

## Retry

- Only `TransportError::class() == Retryable` (429, 529, 5xx, connection, timeout). `NonRetryable` (400/401/403/422, schema
  drift, wrong model, not-JSON, oversized, unexpected status, config) and `Cancelled` end the call immediately.
- Delay = `max(backoff, Retry-After)`; `backoff = cap - cap*jitter*r/2^64`, `cap = min(max_delay, base*2^(n-1))`, integer
  math. `r` comes from the injected `JitterSource`; `SeededRng` (SplitMix64) is provided. One draw per scheduled retry.
- Bounds, each producing a typed reason: `max_attempts` (`RetriesExhausted`); the deadline: if `delay >= time left` the call
  gives up at once rather than sleeping (`DeadlineExceeded`, so a huge `Retry-After` costs no waiting); the request budget
  (`RequestBudget`, every attempt spends one); the `CancellationToken` (`Cancelled`, also during backoff).
- An attempt cut short by the caller's deadline (timeout clamped below `attempt_timeout`) returns `DeadlineExceeded` and is
  not counted against the breaker.

## Circuit breaker

`Closed -> Open` after `consecutive_failures` retryable failures (or the configured failure ratio over a sliding window with
`min_samples`) `-> HalfOpen` once `cooldown` elapsed and a call asks for admission; exactly one probe is admitted (others get
`CircuitOpen`); probe success `-> Closed`, probe failure `-> Open` with a fresh cooldown. Open performs zero network calls.
Non-retryable replies other than auth (400/422/schema drift) prove reachability and count as breaker success; local `Config`
errors and cancellations release the probe slot without judgment.

Auth (401/403) moves to `Misconfigured`: no cooldown, no probe, every call returns `Misconfigured` until the owner calls
`config_changed()` after fixing the key/endpoint (a bad credential must not be probed in a loop). Gauge `copper_jev_circuit_state`
is 0 closed, 1 half-open, 2 open; `Misconfigured` reports 2 plus `circuit_misconfigured = true`.

## Quota

Fixed windows aligned to the injected clock's origin (`index = now / 3600s`), not wall-clock hours; the counters reset when the
index changes. Request budget is reserved per HTTP attempt (retries included). Token and USD budgets can only be known after an
answer, so they gate new requests against spend already recorded; overshoot is bounded by `max_inflight` answers. Usage is
recorded only from successful answers (`AdvisoryAnswer.usage`) into the window current at completion. Cache hits and coalesced
followers spend nothing. In-flight limit is a semaphore taken before breaker admission and budget reservation, so waiting for a
slot never burns budget or a probe.

## Cache and single-flight

Key = `RequestKey::id()` (feature_hash, model, prompt_hash). Concurrent identical calls elect one leader that runs the attempt
loop in its own task; followers wait on a `watch` channel bounded by their own deadline and cancel token. If the leader ends in
`Cancelled`/`DeadlineExceeded` (its own context), followers with time left re-contend instead of inheriting it; a dropped leader
frees the slot via a guard. Successful answers are cached for `cache.ttl` in a size-bounded LRU. Failures are never cached except
the explicit negative cache: schema-class errors (`SchemaDrift`, `WrongModel`, `NotJson`, `MalformedJson`, `Oversized`) are
remembered per key for `negative_ttl` (5 s) and return `NegativeCached` without network, so a drifting API cannot be hammered
by one hot borrower state.

## Time

All timing uses the `Clock` trait (`now()`, `sleep(d, SleepPurpose)`); `TokioClock` is production (honors `tokio::time::pause`).
No wall-clock read exists in decision logic. `SleepPurpose` (Backoff / AttemptTimeout / DeadlineWait) lets test clocks advance
only backoffs. The integration tests use a `ManualClock` (tests/common/manual.rs) so no real timer races real socket IO
(`tokio::time::pause` with live TCP auto-advances while IO is pending and would fire timeouts spuriously).

## Metrics (`MetricsSnapshot`)

`requests_total{result}` (per HTTP attempt: ok, rate_limited, overloaded, server_error, connection_error, timeout, auth_error,
bad_request, schema_error, cancelled, other_error), `request_duration{status}` (same labels; count, sum, 8 buckets + overflow),
`circuit_state`, `fallback_total{reason}` (per call returning `Unavailable`, labels = `FallbackReason::label()`),
`input_tokens_total` (+ output), `estimated_cost_micro_usd_total`, plus `cache_hits_total`, `coalesced_total`, `inflight`
and current-window usage gauges. All labels are fixed enums; no borrower, key or address ever appears. Task 013 maps this to
the exporter (`copper_jev_*`; divide micro-USD by 1e6).

## Tests (`crates/jev-advisor/tests/resilience.rs`, plus unit tests in each module)

Real `TypeSafeClient` against the 009 local TCP server (extended with `Script::Seq` and `Script::Delayed`): 429 + Retry-After,
Retry-After beyond deadline, no retry on 400/401/403/422/schema/wrong-model/malformed, 529 then success, exact backoff
schedule (jitter off: 100/200/400 ms; seeded: replayed through the public formula and reproducible), cancellation mid-backoff,
breaker closed/open/half-open/closed, failed probe, single probe, open = zero server hits, auth -> misconfigured -> manual reset,
request/token/USD budgets and hour-boundary reset, in-flight cap (fail-fast, wait, wait bounded by deadline), 10 concurrent
identical requests = 1 server hit, TTL hit/miss, LRU bound, failures not cached, negative cache, outage (refused and hanging
server) within the deadline, and a proptest over arbitrary response sequences asserting attempts <= max_attempts, total hits <=
request budget, hits == reported attempts, and `elapsed <= deadline` on the manual clock.

## Gaps

- Token/USD budgets are checked before, not after, spend (bounded overshoot, above).
- `Misconfigured` has no automatic recovery by design; the composition root must call `config_changed()` on reload.
- No Prometheus exporter or config-file loading (013/012). The `JitterSource` seed is supplied by the composition root.
- Wall-time tests (in-flight, outage) use real short durations with a 50 ms slack; they pass reliably but are the only
  time-sensitive ones.
