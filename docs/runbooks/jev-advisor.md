# Jev liquidation advisor: container runbook (JEV-LIQ-014)

Process B (`copper-jev-advisor`, shadow mode only) runs as its own container, `mev-jev-advisor`, built from the `advisor`
target of the repo `Containerfile` (same pinned Arch base, only the advisor binary, no signer/RPC path). It is a compose
profile (`jev`): `scripts/run.sh shadow|live|stop` never start or touch it.

It reads the bot's `data/liq-state.sqlite` and writes `data/advisor.sqlite` (same host `./data` dir, mounted
`/var/lib/mev-bot`), and calls the TypeSafe API over HTTPS. No ports are published.

## Key file

```bash
mkdir -p secrets && install -m 600 /dev/null secrets/typesafe_api_key
${EDITOR:-vi} secrets/typesafe_api_key        # paste the key, nothing else
```

`secrets/` is gitignored. The file is bind-mounted read-only at `/run/secrets/typesafe_api_key` and passed via
`TYPESAFE_API_KEY_FILE`; the worker refuses any file with group/other permission bits.

## Start / stop / status

```bash
export JEV_POLICY_VERSION=<the bot's liquidation export policy version>   # required, must match the producer
scripts/run.sh advisor-start     # refuses if the key is missing, empty or not 0600; builds, then starts only `advisor`
scripts/run.sh advisor-status    # container state + last metrics lines
scripts/run.sh advisor-logs
scripts/run.sh advisor-stop      # SIGTERM, 20 s stop timeout (grace 10 s + 5 s abort + margin)
```

Optional overrides (environment of `advisor-start`): `JEV_MODEL`, `JEV_MAX_INFLIGHT`, `JEV_TIMEOUT_MS`,
`JEV_BUDGET_PER_HOUR`, `RUST_LOG`. The full env table is in `docs/design/jev-liq-012-worker.md`. Mode is fixed to `shadow`.

Healthcheck is `copper-jev-advisor check` (config + both DBs + schema versions; never contacts the Jev API).

## Failure behaviour

* No key / bad key permissions / `JEV_MODE=active`: the advisor exits 2 with a configuration error and (`restart:
  unless-stopped`) restart-loops; the bot is unaffected. `advisor-start` catches the missing/0644 key before starting.
* Bot has not yet created `liq-state.sqlite` or runs an older schema: `check` reports `not ready`, container shows unhealthy.
* Jev unreachable or 401/403: the worker degrades (fallback/breaker) and the baseline continues; 401/403 needs a key fix and
  `advisor-stop` + `advisor-start`.

## Disable

`scripts/run.sh advisor-stop`. To remove permanently, delete nothing else: the bot and its data are independent;
`data/advisor.sqlite` may be deleted afterwards (it only holds shadow advisory state).

## Build / validate by hand

```bash
podman build --target advisor -t localhost/copper-jev-advisor:dev .
podman-compose -f compose.yaml --profile jev config
```
