# Copper liquidation alerts runbook (JEV-LIQ-013)

Rules: `infra/monitoring/copper-liq.rules.yml` (validate: `scripts/promtool.sh check rules infra/monitoring/copper-liq.rules.yml`).
Dashboard: `infra/monitoring/copper-liq-dashboard.json`. Metric catalogue and what is not produced yet:
`docs/design/jev-liq-013-observability.md`.

## Ground rules

* **Two classes.** `page="true"` (severity `critical`) alerts are about the mandatory liquidation path or a broken invariant.
  `page="false"` (severity `warning`, class `advisory-only`) alerts are about the Jev advisor; they must never page, because
  the deterministic baseline keeps monitoring and trading regardless of Jev (spec section 0, invariants 1, 2, 6).
* **The advisor can always be stopped without affecting trading.** It has no signer, no RPC and no executor channel; the bot
  never waits for it. Stop it with `kill -TERM <pid>` (or `podman stop -t 20 <advisor container>` once task 014 ships the
  container; the worker needs `ADVISOR_SHUTDOWN_GRACE_SECONDS + 5` = 15 s by default). Leases lapse after
  `ADVISOR_TTL_SECONDS`, rows are retried on restart, nothing is double-applied.
* **Advisory mode stays shadow.** `JEV_MODE=active` is rejected by the advisor (task JEV-LIQ-019 gates it); the bot-side policy
  `AdvisoryMode` defaults to `Off`, and in `Off`/`Shadow` dispatch equals the baseline exactly. To disable advisory evaluation
  entirely: stop the advisor and leave (or set) the policy mode to `off` (`AdvisoryMode::parse_config("off")`); no restart of
  the trading path is needed for the advisor part.
* **Exporters are off by default.** Hot path: set `MEV_METRICS_ADDR=127.0.0.1:9108` in the bot environment (loopback only;
  `MEV_METRICS_ALLOW_NON_LOOPBACK=1` is the explicit override). Advisor: `--metrics-addr 127.0.0.1:9109` /
  `ADVISOR_METRICS_ADDR` (`--metrics-allow-non-loopback` is the explicit override). Never publish either port publicly.
* Quick look at the bot: `scripts/run.sh status`, `scripts/run.sh logs`. Scheduler records are also in the JSONL event log:
  `grep '"kind":"liq_sched_counters"' data/arbitrum/events-$(date -u +%F).jsonl | tail -n 3`.
* Advisor readiness (never contacts Jev): `copper-jev-advisor check` with the worker's environment (prints
  `{"ready":true,...}`, exit 1 = not ready, exit 2 = configuration error). Advisor metrics log line:
  `podman logs <advisor container> 2>&1 | grep 'copper_jev_advisor::metrics' | tail -n 1` (or the process' stdout).
* Raw scrape: `curl -s http://127.0.0.1:9108/metrics | grep '^copper_'` (bot), `... :9109 ...` (advisor).

Alerts for the spec's other critical conditions (stale state data, indexer gap or reorg mismatch, snapshot-DB backlog or lease
errors, simulator state mismatch, REVM provider inconsistency) are not defined because no series exist for them yet; see the
design doc for the producing task. Until then use the logs and `scripts/run.sh status`.

# Critical (page)

## CopperLiqMandatoryDeadlineMiss

* **Meaning.** A shadow scheduler pass found P0 or P1 recheck jobs past their deadline (`reason="backlog"`: still queued;
  `reason="late_dispatch"`: dispatched but late). The counter adds one per overdue job per pass.
* **First checks.** `scripts/run.sh status`; `scripts/run.sh logs | grep -E 'ALERT liquidation scheduler|liq_sched_shadow'`;
  the latest `liq_sched_counters` event (fields `backlog_p0`, `backlog_p1`, `deadline_misses_*`, `free_slots`); host CPU and
  RPC latency (is the recheck budget starved?).
* **Safe actions.** None needed on the advisor side (it cannot cause this; P0/P1 never depend on Jev). If the bot is behind,
  reduce load (stop other processes on the host) and let the backlog drain. Do not raise limits blindly. `scripts/run.sh halt`
  stops new trades if state may be stale.
* **Escalation.** Persisting more than 15 minutes, or accompanied by `CopperLiqCoverageViolation`: owner decision on halting
  and on the scheduler budget (`PolicyConfig` concurrency and shares, docs/design/jev-liq-005-scheduler.md).

## CopperLiqCoverageViolation

* **Meaning.** The scheduler cannot guarantee every tracked account is rechecked within the max age (capacity too small for the
  tracked set, or overdue mandatory backlog).
* **First checks.** `liq_sched_shadow` records (`coverage_violations` array in the event) via the logs or JSONL; tracked
  account count vs `sweep_capacity_per_window`.
* **Safe actions.** Reduce the tracked set or raise recheck concurrency (config change, restart of the bot is the owner's call).
  No advisor action helps.
* **Escalation.** Owner: coverage claims are void until resolved; do not enable advisory ordering.

## CopperLiqSchedulerPassStale

* **Meaning.** No shadow scheduler pass was observed for 3 minutes: the pass is disabled, stuck, or the bot stalled.
  (The scheduler shadow pass is off unless enabled; if it was never enabled the series does not exist and this cannot fire.)
* **First checks.** `scripts/run.sh status` (is the container up and healthy); `scripts/run.sh logs | tail`; heartbeat file
  `data/arbitrum/heartbeat` (unix seconds, written by the metrics writer).
* **Safe actions.** If the bot is wedged, `scripts/run.sh halt` first, then restart per `docs/runbooks/live-cutover.md`.
* **Escalation.** Owner, if the heartbeat is also stale.

## CopperLiqAdvisoryChangedMandatory

* **Meaning.** An advisory shadow pass reported `mandatory_unchanged=false`: the would-be ordering altered P0/P1 output. This
  must be impossible (spec invariant 2). The bot logs `ALERT advisory ordering changed P0/P1 output (invariant breach)`.
* **First checks.** The `liq_advisory_shadow` event (JSONL) and the log line; which commit is deployed.
* **Safe actions.** Stop the advisor and set the policy mode to `off` (see ground rules). Dispatch is unaffected in `Shadow`,
  but do not proceed to any active-mode work.
* **Escalation.** Treat as a code defect: open an incident, attach the event record, block JEV-LIQ-019.

# Advisory-only (no page)

For all of these: trading and the baseline schedule are unaffected. Worst case is that the advisor is stopped.

## CopperJevHttpErrors

* **Meaning.** More than 5 attempts in 10 minutes ended 429 (`rate_limited`), 529 (`overloaded`) or 5xx (`server_error`).
* **First checks.** `curl -s :9109/metrics | grep copper_jev_requests_total`; the advisor metrics log line (`requests_total`,
  `circuit_state`); TypeSafe status page.
* **Safe actions.** Wait (retry with backoff and the breaker are automatic). Lower `JEV_MAX_INFLIGHT` or
  `JEV_BUDGET_PER_HOUR` if 429s persist; or stop the advisor.
* **Escalation.** None unless it persists for days; then contact TypeSafe about limits.

## CopperJevCircuitOpen

* **Meaning.** The breaker has been open (state 2) for 5 minutes; no rankings are produced. It half-opens by itself after the
  cooldown (30 s default).
* **First checks.** `copper_jev_fallback_total{reason="circuit_open"}`; preceding errors in `CopperJevHttpErrors`; logs
  `fallback=circuit_open`.
* **Safe actions.** None needed. If it is `misconfigured`, see the next entry.
* **Escalation.** None.

## CopperJevMisconfigured

* **Meaning.** Jev answered 401/403: wrong or revoked key, or wrong endpoint. The breaker is sticky; the worker does not
  hot reload.
* **First checks.** `copper-jev-advisor check` (validates config and file permissions, not the key); key file mode must be
  `0600`; `TYPESAFE_API_KEY_FILE` points at the right file.
* **Safe actions.** Fix the key or endpoint, restart the advisor. Never paste the key into logs or tickets.
* **Escalation.** Owner provisions a new key.

## CopperJevBudgetExhausted

* **Meaning.** Calls fell back because a request, token or USD budget was used up (`request_budget`, `token_budget`,
  `cost_budget`). The worker pauses claiming for `ADVISOR_FALLBACK_PAUSE_MS`.
* **First checks.** `copper_jev_window_requests_used`, `copper_jev_window_tokens_used`, `copper_jev_window_cost_usd_used`;
  `JEV_BUDGET_*` settings.
* **Safe actions.** Wait for the window; raise the budget only deliberately (cost). Or stop the advisor.
* **Escalation.** Owner, if the spend is higher than planned (see also `CopperJevCostSpike`).

## CopperJevSchemaOrModelMismatch

* **Meaning.** Responses were rejected: schema drift, wrong model, or policy/feature-schema/chain mismatch
  (`result="schema_error"` or rows dead-lettered as `dead_invalid_response`).
* **First checks.** Advisor logs for `dead-lettered`; `advisor_outbox.last_error` in the advisory DB; `JEV_MODEL` and
  `ALLOWED_MODELS`; whether TypeSafe changed the model.
* **Safe actions.** Stop the advisor; fix `ALLOWED_MODELS` or pin the model; do not widen the allowlist without review.
* **Escalation.** Owner decides on model change (re-validation needed before any use).

## CopperJevDecisionExpiry

* **Meaning.** More than 50 answers in 30 minutes were stale or expired when they arrived (`copper_jev_stale_response_total`):
  the advisor is slower than `ADVISOR_TTL_SECONDS`.
* **First checks.** Request latency panel; `copper_jev_advisor_queue_rows{state="pending"}`; `JEV_TIMEOUT_MS`.
* **Safe actions.** Raise `JEV_MAX_INFLIGHT` or `ADVISOR_TTL_SECONDS` modestly, or stop the advisor. Expired decisions are
  ignored by the policy anyway.
* **Escalation.** None.

## CopperJevCostSpike

* **Meaning.** Estimated spend above 2 USD in an hour (threshold in the rule file; tune to the budget). The estimate is zero
  unless `JEV_PRICE_*` are configured.
* **First checks.** `copper_jev_input_tokens_total` rate, request rate, cache hit ratio.
* **Safe actions.** Lower `JEV_BUDGET_USD_MICRO_PER_HOUR` / `JEV_BUDGET_PER_HOUR`, or stop the advisor.
* **Escalation.** Owner, if unexplained.

## CopperJevAdvisorQueueBacklog

* **Meaning.** More than 800 pending rows for 10 minutes (the cap `ADVISOR_MAX_PENDING` is 1000; beyond it the producer sheds
  export rows).
* **First checks.** `copper-jev-advisor check`; pending vs in-flight panel; `CopperJevCircuitOpen`/budget alerts as a cause.
* **Safe actions.** Raise `JEV_MAX_INFLIGHT`, or accept the shedding. Stopping the advisor lets rows age out.
* **Escalation.** None.

## CopperJevAdvisorLeaseOrDeadRows

* **Meaning.** The advisor queue is losing work: `lease_lost`, dead-lettered rows (`dead_invalid_features`,
  `dead_non_retryable`) or export rows shed before copy (`copper_jev_advisor_queue_missed_total`). This is the advisory DB only;
  the snapshot DB and baseline are separate processes.
* **First checks.** Advisor logs `dead-lettered`; `ADVISOR_DB` disk space; `lease_lost` is normal under load, rows retry.
* **Safe actions.** Free disk; raise the TTL or lower concurrency; restart the advisor.
* **Escalation.** Owner if rows are dead-lettered for `invalid_features` (producer/advisor schema mismatch).

## CopperJevAdvisorStatsFailing

* **Meaning.** The advisor cannot read its own queue statistics (`copper_jev_advisor_queue_stats_ok == 0`).
* **First checks.** `copper-jev-advisor check`; permissions and free space of the `ADVISOR_DB` directory.
* **Safe actions.** Fix permissions or disk; restart the advisor.
* **Escalation.** None.

## CopperJevAdvisorDown

* **Meaning.** Prometheus cannot scrape the advisor (stopped, crashed, exporter off, or wrong target).
* **First checks.** Is it supposed to run? `ps`/`podman ps`; `copper-jev-advisor check`; was it started with `--metrics-addr`.
* **Safe actions.** Start it, or silence the alert if the advisor is intentionally off.
* **Escalation.** None.
