//! `seqscope`: watch the Arbitrum sequencer feed through rules, keep what matches, and look at it.
//!
//!   seqscope run      collect: feed -> rules -> database        (the usual way to run it)
//!   seqscope serve    collect and serve the web interface (http://127.0.0.1:7878)
//!   seqscope tail     watch matches live, nothing saved
//!   seqscope record   save raw transactions from the feed to a file (to replay and tune rules offline)
//!   seqscope replay   run a recorded file through rules
//!   seqscope rules    check a rules file, try a rule on recorded data, print a starter template
//!   seqscope decode   decode one call (all wrapped calls inside it)
//!   seqscope query    filtered rows from the database;  seqscope sql  free-form read-only SQL
//!   seqscope stats    what the database holds;  seqscope gaps  what is still undecoded
//!   seqscope catalogue  the fields, protocols and contracts rules can use
//!
//! Rules are in a TOML file (see `seqscope rules template`). Edit it while `run` or `tail` is going: it reloads, and
//! a mistake keeps the previous rules and says what is wrong.

#[macro_use]
mod output;
mod templates;

use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use seqscope_core::{ActionKind, Engine, Mode, Profile, Rule, RuleSet, Tx, catalogue, preview};
use seqscope_store::{Query, Reader, Writer, WriterConfig};

const DEFAULT_FEED: &str = "wss://arb1.arbitrum.io/feed";

#[derive(Parser)]
#[command(
    name = "seqscope",
    version,
    about = "Filter and decode the Arbitrum sequencer feed with rules you write"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Source {
    /// Sequencer feed URL.
    #[arg(long, default_value = DEFAULT_FEED)]
    url: String,
}

#[derive(Args, Clone)]
struct RulesArg {
    /// Rules file (TOML). Without one, only counters run.
    #[arg(long, short = 'r', default_value = "seqscope.toml")]
    rules: PathBuf,
    /// A single rule typed on the command line instead of a file: stores matches (see `seqscope catalogue`).
    #[arg(long, short = 'w')]
    when: Option<String>,
}

#[derive(Args, Clone)]
struct DbArg {
    /// Database file.
    #[arg(long, default_value = "seqscope.db")]
    db: PathBuf,
}

#[derive(Subcommand)]
enum Cmd {
    /// Collect: feed -> rules -> database. Prints a status line every few seconds.
    Run {
        #[command(flatten)]
        source: Source,
        #[command(flatten)]
        rules: RulesArg,
        #[command(flatten)]
        db: DbArg,
        /// Seconds between status lines.
        #[arg(long, default_value_t = 10)]
        every: u64,
        /// Also print each stored row.
        #[arg(long)]
        print: bool,
    },
    /// Collect, and serve the web interface and API on this machine (http://127.0.0.1:7878 by default).
    Serve {
        #[command(flatten)]
        source: Source,
        #[command(flatten)]
        rules: RulesArg,
        #[command(flatten)]
        db: DbArg,
        /// Address to listen on. Only loopback addresses are accepted unless --allow-remote.
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: std::net::SocketAddr,
        /// Replay a recording (from `seqscope record`) instead of reading the feed; no network needed.
        #[arg(long)]
        replay: Option<PathBuf>,
        /// Replay pace: 1 = recorded, 10 = ten times faster, 0 = as fast as possible.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Do not read any transactions: serve an existing database only.
        #[arg(long)]
        idle: bool,
        /// Accept requests that are not addressed to localhost. The interface can edit the rules file: only on a network you trust.
        #[arg(long)]
        allow_remote: bool,
    },
    /// Watch matches live without saving.
    Tail {
        #[command(flatten)]
        source: Source,
        #[command(flatten)]
        rules: RulesArg,
        /// JSON lines instead of text.
        #[arg(long)]
        json: bool,
        /// Stop after this many rows.
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Save raw transactions from the feed to a file (JSON lines) for `replay` and `rules test`.
    Record {
        #[command(flatten)]
        source: Source,
        #[arg(long, short = 'o')]
        out: PathBuf,
        /// Stop after this many seconds.
        #[arg(long, default_value_t = 60)]
        secs: u64,
    },
    /// Run a recorded file through rules.
    Replay {
        file: PathBuf,
        #[command(flatten)]
        rules: RulesArg,
        /// Store matches here instead of printing them.
        #[arg(long)]
        db: Option<PathBuf>,
        /// 1 = original pace, 10 = ten times faster, 0 = as fast as possible.
        #[arg(long, default_value_t = 0.0)]
        speed: f64,
        #[arg(long)]
        json: bool,
    },
    /// Check, try and start rules.
    Rules {
        #[command(subcommand)]
        cmd: RulesCmd,
    },
    /// Decode one call: every action inside it, through multicall / Multicall3 / smart accounts / ERC-4337.
    Decode {
        /// Destination address.
        to: String,
        /// Calldata as 0x hex.
        input: String,
    },
    /// Filtered rows from the database, newest first.
    Query {
        #[command(flatten)]
        db: DbArg,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        protocol: Option<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        alerts: bool,
        /// Only the last N minutes.
        #[arg(long)]
        minutes: Option<u64>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Free-form read-only SQL against the database (table `rows`, text-address view `v_rows`).
    Sql {
        sql: String,
        #[command(flatten)]
        db: DbArg,
        #[arg(long, default_value_t = 1000)]
        max_rows: usize,
        #[arg(long)]
        json: bool,
    },
    /// What the database holds.
    Stats {
        #[command(flatten)]
        db: DbArg,
    },
    /// Destinations and functions the decoder cannot read yet, busiest first.
    Gaps {
        #[command(flatten)]
        db: DbArg,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// The fields, protocols and contracts rules can use.
    Catalogue {
        /// fields | protocols | all
        #[arg(default_value = "fields")]
        what: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RulesCmd {
    /// Compile a rules file and list what is in it; exit 1 with located errors if it is wrong.
    Check { file: PathBuf },
    /// Count what a rule would match over a recorded file, with examples.
    Test {
        #[arg(long, short = 'w')]
        when: String,
        /// Recorded transactions (`seqscope record`).
        #[arg(long, short = 'f')]
        file: PathBuf,
        #[arg(long, default_value_t = 5)]
        samples: usize,
    },
    /// Print a starter rules file, or list the templates.
    Template { name: Option<String> },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run {
            source,
            rules,
            db,
            every,
            print,
        } => runtime()?.block_on(run(source, rules, db, every, print)),
        Cmd::Serve {
            source,
            rules,
            db,
            bind,
            replay,
            speed,
            idle,
            allow_remote,
        } => {
            let src = if idle {
                seqscope_server::Source::Idle
            } else if let Some(file) = replay {
                seqscope_server::Source::Replay { file, speed }
            } else {
                seqscope_server::Source::Feed(source.url)
            };
            runtime()?.block_on(seqscope_server::serve(seqscope_server::Config {
                rules: rules.rules,
                db: db.db,
                bind,
                allow_remote,
                source: src,
                writer: WriterConfig::default(),
            }))
        }
        Cmd::Tail {
            source,
            rules,
            json,
            limit,
        } => runtime()?.block_on(tail(source, rules, json, limit)),
        Cmd::Record { source, out, secs } => runtime()?.block_on(record(source, out, secs)),
        Cmd::Replay {
            file,
            rules,
            db,
            speed,
            json,
        } => replay(file, rules, db, speed, json),
        Cmd::Rules { cmd } => rules_cmd(cmd),
        Cmd::Decode { to, input } => decode(&to, &input),
        Cmd::Query {
            db,
            kind,
            protocol,
            selector,
            to,
            profile,
            rule,
            tag,
            alerts,
            minutes,
            limit,
            json,
        } => {
            let since_ms = minutes.map(|m| now_ms().saturating_sub(m * 60_000));
            query(
                db,
                Query {
                    since_ms,
                    until_ms: None,
                    protocol,
                    kind,
                    selector,
                    to,
                    profile,
                    rule,
                    tag,
                    alert_only: alerts,
                    limit: Some(limit),
                },
                json,
            )
        }
        Cmd::Sql {
            sql,
            db,
            max_rows,
            json,
        } => sql_cmd(&db.db, &sql, max_rows, json),
        Cmd::Stats { db } => stats(&db.db),
        Cmd::Gaps { db, limit } => gaps(&db.db, limit),
        Cmd::Catalogue { what, json } => catalogue_cmd(&what, json),
    }
}

/// Ctrl-C or SIGTERM (what `kill`, systemd and `podman stop` send): either way the collector must flush its queue.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---------------------------------------------------------------------------------------------------------------
// Rules, loaded and reloaded

/// The built-in rules when none are given: counters by kind, nothing stored.
fn counters() -> RuleSet {
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

fn single(when: &str) -> RuleSet {
    RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "cli".into(),
            enabled: true,
            mode: Mode::All,
            rule: vec![Rule {
                name: "when".into(),
                enabled: true,
                when: when.into(),
                action: ActionKind::Store,
                tag: None,
                sample: 1,
            }],
        }],
    }
}

fn compile(set: &RuleSet) -> Result<Engine> {
    Engine::compile(set).map_err(|errs| {
        anyhow::anyhow!(
            errs.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

/// Rules that reload when the file changes. A broken edit keeps the previous rules running.
struct Live {
    path: Option<PathBuf>,
    engine: Arc<Engine>,
    seen: Option<SystemTime>,
    last_check: Instant,
}

impl Live {
    fn open(a: &RulesArg) -> Result<Live> {
        if let Some(w) = &a.when {
            return Ok(Live {
                path: None,
                engine: Arc::new(compile(&single(w))?),
                seen: None,
                last_check: Instant::now(),
            });
        }
        if a.rules.exists() {
            let text = std::fs::read_to_string(&a.rules)
                .with_context(|| format!("reading {}", a.rules.display()))?;
            let set = RuleSet::from_toml(&text)
                .map_err(|e| anyhow::anyhow!("{}: {e}", a.rules.display()))?;
            let engine = compile(&set).with_context(|| format!("in {}", a.rules.display()))?;
            let seen = std::fs::metadata(&a.rules).and_then(|m| m.modified()).ok();
            eprintln!(
                "rules: {} ({} profiles)",
                a.rules.display(),
                set.profile.len()
            );
            return Ok(Live {
                path: Some(a.rules.clone()),
                engine: Arc::new(engine),
                seen,
                last_check: Instant::now(),
            });
        }
        eprintln!(
            "rules: {} not found; running counters only. Start one with `seqscope rules template`.",
            a.rules.display()
        );
        Ok(Live {
            path: Some(a.rules.clone()),
            engine: Arc::new(compile(&counters())?),
            seen: None,
            last_check: Instant::now(),
        })
    }

    /// Cheap enough to call per message; looks at the file at most once a second.
    fn engine(&mut self) -> Arc<Engine> {
        if self.last_check.elapsed() >= Duration::from_secs(1) {
            self.last_check = Instant::now();
            if let Some(p) = self.path.clone() {
                let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                if m.is_some() && m != self.seen {
                    self.seen = m;
                    match std::fs::read_to_string(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|t| RuleSet::from_toml(&t))
                        .and_then(|s| {
                            Engine::compile(&s).map_err(|e| {
                                e.iter()
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                        }) {
                        Ok(e) => {
                            eprintln!("rules: reloaded {}", p.display());
                            self.engine = Arc::new(e);
                        }
                        Err(e) => {
                            eprintln!("rules: NOT reloaded, keeping the previous rules:\n{e}")
                        }
                    }
                }
            }
        }
        self.engine.clone()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Live commands

fn feed(source: &Source) -> tokio::sync::mpsc::UnboundedReceiver<mev_sequencer::SequencedTx> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(mev_sequencer::run(source.url.clone(), tx));
    rx
}

async fn run(source: Source, rules: RulesArg, db: DbArg, every: u64, print: bool) -> Result<()> {
    let mut live = Live::open(&rules)?;
    let writer = Writer::spawn(&db.db, WriterConfig::default())
        .with_context(|| format!("opening {}", db.db.display()))?;
    let ingest = writer.ingest();
    eprintln!(
        "database: {}   feed: {}   Ctrl-C to stop",
        db.db.display(),
        source.url
    );
    let mut rx = feed(&source);
    let stop = shutdown_signal();
    tokio::pin!(stop);
    let mut tick = tokio::time::interval(Duration::from_secs(every.max(1)));
    tick.tick().await;
    let (mut txs, mut stored, mut last_txs) = (0u64, 0u64, 0u64);
    let started = Instant::now();
    loop {
        tokio::select! {
            m = rx.recv() => {
                let Some(st) = m else { bail!("the feed task ended") };
                txs += 1;
                let tx = Tx::from_sequenced(&st);
                for row in live.engine().process(&tx) {
                    if print { say!("{}", output::row_line(&row)); }
                    ingest.send(row);
                    stored += 1;
                }
            }
            _ = tick.tick() => {
                let s = ingest.stats().snapshot();
                let e = live.engine();
                let top: Vec<String> = { let mut v = e.stats(); v.sort_by_key(|r| std::cmp::Reverse(r.matches)); v.iter().take(4).filter(|r| r.matches > 0).map(|r| format!("{}={}", r.rule, r.matches)).collect() };
                eprintln!("{:>5}s  {} tx/s  calls {}  rows {}  written {}  dropped {}  failed {}  | {}", started.elapsed().as_secs(), (txs - last_txs) / every.max(1), e.calls_seen(), stored, s.written, s.dropped, s.failed, top.join(" "));
                last_txs = txs;
                if let Some(err) = s.last_error { eprintln!("storage error: {err}"); }
            }
            _ = &mut stop => {
                eprintln!("stopping; writing what is queued…");
                break;
            }
        }
    }
    drop(ingest);
    let s = writer.shutdown();
    eprintln!(
        "stored {} rows ({} dropped, {} failed)",
        s.written, s.dropped, s.failed
    );
    Ok(())
}

async fn tail(source: Source, rules: RulesArg, json: bool, limit: Option<u64>) -> Result<()> {
    let mut rules = rules;
    if rules.when.is_none() && !rules.rules.exists() {
        // Nothing asked for: show what the decoder reads.
        rules.when = Some("kind == swap || kind == liquidity || kind == liquidation".into());
    }
    let mut live = Live::open(&rules)?;
    let mut rx = feed(&source);
    let stop = shutdown_signal();
    tokio::pin!(stop);
    let mut shown = 0u64;
    loop {
        tokio::select! {
            m = rx.recv() => {
                let Some(st) = m else { bail!("the feed task ended") };
                for row in live.engine().process(&Tx::from_sequenced(&st)) {
                    if json { say!("{}", serde_json::to_string(&row)?); } else { say!("{}", output::row_line(&row)); }
                    shown += 1;
                    if limit.is_some_and(|l| shown >= l) { return Ok(()); }
                }
            }
            _ = &mut stop => return Ok(()),
        }
    }
}

async fn record(source: Source, out: PathBuf, secs: u64) -> Result<()> {
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(&out).with_context(|| format!("creating {}", out.display()))?,
    );
    let mut rx = feed(&source);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut n = 0u64;
    eprintln!("recording {secs}s from {} to {}", source.url, out.display());
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(st)) => {
                writeln!(f, "{}", serde_json::to_string(&Tx::from_sequenced(&st))?)?;
                n += 1;
            }
            Ok(None) => bail!("the feed task ended"),
            Err(_) => {}
        }
    }
    f.flush()?;
    eprintln!("saved {n} transactions");
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Offline commands

fn read_txs(file: &Path) -> Result<Vec<Tx>> {
    let f = std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    std::io::BufReader::new(f)
        .lines()
        .enumerate()
        .filter(|(_, l)| l.as_ref().map(|l| !l.trim().is_empty()).unwrap_or(true))
        .map(|(i, l)| {
            serde_json::from_str::<Tx>(&l?).with_context(|| {
                format!("{}:{}: not a recorded transaction", file.display(), i + 1)
            })
        })
        .collect()
}

fn replay(
    file: PathBuf,
    rules: RulesArg,
    db: Option<PathBuf>,
    speed: f64,
    json: bool,
) -> Result<()> {
    let txs = read_txs(&file)?;
    let mut live = Live::open(&rules)?;
    let writer = db
        .as_ref()
        .map(|p| Writer::spawn(p, WriterConfig::default()))
        .transpose()?;
    let ingest = writer.as_ref().map(|w| w.ingest());
    let (start, t0) = (Instant::now(), txs.first().map_or(0, |t| t.t_ms));
    let mut rows = 0u64;
    for tx in &txs {
        if speed > 0.0 {
            let due = Duration::from_millis(((tx.t_ms.saturating_sub(t0)) as f64 / speed) as u64);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        for row in live.engine().process(tx) {
            rows += 1;
            match &ingest {
                Some(i) => {
                    // Replay is offline: wait for room instead of dropping.
                    while !i.send(row.clone()) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                None if json => say!("{}", serde_json::to_string(&row)?),
                None => say!("{}", output::row_line(&row)),
            }
        }
    }
    if let Some(w) = writer {
        drop(ingest);
        let s = w.shutdown();
        eprintln!(
            "{} transactions -> {} rows, stored {}",
            txs.len(),
            rows,
            s.written
        );
    } else {
        eprintln!("{} transactions -> {} rows", txs.len(), rows);
    }
    Ok(())
}

fn rules_cmd(cmd: RulesCmd) -> Result<()> {
    match cmd {
        RulesCmd::Check { file } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let set = RuleSet::from_toml(&text)
                .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
            match Engine::compile(&set) {
                Ok(_) => {
                    for p in &set.profile {
                        say!(
                            "profile {} ({}{})",
                            p.name,
                            if p.mode == Mode::First {
                                "first"
                            } else {
                                "all"
                            },
                            if p.enabled { "" } else { ", disabled" }
                        );
                        for r in &p.rule {
                            say!(
                                "  {}{:<24} {:?}  {}",
                                if r.enabled { " " } else { "x" },
                                r.name,
                                r.action,
                                r.when
                            );
                        }
                    }
                    say!("ok");
                    Ok(())
                }
                Err(errs) => {
                    for e in &errs {
                        eprintln!("{e}");
                    }
                    bail!("{} rule(s) have problems", errs.len())
                }
            }
        }
        RulesCmd::Test {
            when,
            file,
            samples,
        } => {
            let txs = read_txs(&file)?;
            let p = preview(&when, &txs, samples).map_err(|e| anyhow::anyhow!("{e}"))?;
            say!(
                "{} transactions, {} calls: {} match",
                p.transactions,
                p.calls,
                p.matched
            );
            for r in &p.samples {
                say!("  {}", output::row_line(r));
            }
            Ok(())
        }
        RulesCmd::Template { name: None } => {
            for (n, d) in templates::NAMES {
                say!("{n:<14} {d}");
            }
            say!("\nseqscope rules template NAME > seqscope.toml");
            Ok(())
        }
        RulesCmd::Template { name: Some(n) } => match templates::get(&n) {
            Some(t) => {
                say!("{}", t.trim_end_matches('\n'));
                Ok(())
            }
            None => bail!("no template '{n}'; run `seqscope rules template` to list them"),
        },
    }
}

fn decode(to: &str, input: &str) -> Result<()> {
    let to: alloy::primitives::Address = to.parse().context("bad address")?;
    let data = alloy::hex::decode(input.trim_start_matches("0x")).context("bad hex")?;
    let actions = mev_decode::calls::actions_in(to, &data);
    say!("{} call(s)", actions.len());
    for a in &actions {
        say!(
            "  to {:#x}  sel {}  via [{}]  {}",
            a.leaf.to,
            a.leaf
                .selector()
                .map_or("-".into(), |s| format!("0x{}", alloy::hex::encode(s))),
            a.leaf.via.join(" > "),
            mev_decode::known_contract(a.leaf.to)
                .map_or("(unidentified)".to_string(), |c| c.name.to_string())
        );
        match &a.decoded {
            Some(d) => say!("      {d:?}"),
            None => say!("      not decoded"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Database commands

fn reader(db: &Path) -> Result<Reader> {
    if !db.exists() {
        bail!(
            "{} does not exist; run `seqscope run` (or `replay --db`) first",
            db.display()
        );
    }
    Ok(Reader::open(db)?)
}

fn query(db: DbArg, q: Query, json: bool) -> Result<()> {
    let rows = reader(&db.db)?.query(&q)?;
    for r in &rows {
        if json {
            say!("{}", serde_json::to_string(r)?)
        } else {
            say!("{}", output::row_line(r))
        }
    }
    if !json {
        eprintln!("{} row(s)", rows.len());
    }
    Ok(())
}

fn sql_cmd(db: &Path, sql: &str, max_rows: usize, json: bool) -> Result<()> {
    let r = reader(db)?.sql(sql, max_rows)?;
    if json {
        say!("{}", serde_json::to_string(&r)?);
    } else {
        output::table(&r.columns, &r.rows);
        if r.truncated {
            eprintln!("(cut at {max_rows} rows)");
        }
    }
    Ok(())
}

fn stats(db: &Path) -> Result<()> {
    let r = reader(db)?;
    let one = |sql: &str| r.sql(sql, 100);
    let span =
        one("SELECT count(*) AS rows, min(t_ms) AS first_ms, max(t_ms) AS last_ms FROM rows")?;
    say!(
        "{}: {} rows, {:.1} MB",
        db.display(),
        span.rows[0][0],
        seqscope_store::disk_bytes(db) as f64 / 1e6
    );
    if let (Some(a), Some(b)) = (span.rows[0][1].as_u64(), span.rows[0][2].as_u64()) {
        say!(
            "from {} to {} UTC ({} min)",
            output::clock(a),
            output::clock(b),
            (b - a) / 60_000
        );
    }
    say!("\nby kind and protocol");
    let t = one(
        "SELECT kind, coalesce(protocol,'-') AS protocol, count(*) AS rows FROM rows GROUP BY 1,2 ORDER BY 3 DESC LIMIT 30",
    )?;
    output::table(&t.columns, &t.rows);
    say!("\nby rule");
    let t = one(
        "SELECT profile, rule, count(*) AS rows FROM rows GROUP BY 1,2 ORDER BY 3 DESC LIMIT 30",
    )?;
    output::table(&t.columns, &t.rows);
    Ok(())
}

fn gaps(db: &Path, limit: usize) -> Result<()> {
    let r = reader(db)?;
    let t = r.sql(
        &format!(
            "SELECT kind, coalesce(known,'(unidentified)') AS contract, to_addr AS destination, selector, count(*) AS calls
             FROM v_rows WHERE kind IN ('undecoded_known','unknown') GROUP BY 1,2,3,4 ORDER BY calls DESC LIMIT {limit}"
        ),
        limit,
    )?;
    if t.rows.is_empty() {
        say!(
            "no undecoded calls stored; store them with the `gaps` template (seqscope rules template gaps)"
        );
    } else {
        output::table(&t.columns, &t.rows);
    }
    Ok(())
}

fn catalogue_cmd(what: &str, json: bool) -> Result<()> {
    let c = catalogue::catalogue();
    if json {
        say!("{}", serde_json::to_string_pretty(&c)?);
        return Ok(());
    }
    if matches!(what, "fields" | "all") {
        say!("FIELDS (use in `when`)");
        for f in &c.fields {
            say!(
                "  {:<24} {:<9} {}{}",
                f.name,
                format!("{:?}", f.ty).to_lowercase(),
                f.doc,
                f.values
                    .as_ref()
                    .map_or(String::new(), |v| format!("  [{}]", v.join(", ")))
            );
        }
    }
    if matches!(what, "protocols" | "all") {
        say!("PROTOCOLS");
        for p in &c.protocols {
            say!(
                "  {}  ({} contracts, {} events)",
                p.id,
                p.contracts.len(),
                p.events.len()
            );
            for k in &p.contracts {
                say!(
                    "      {} {}{}",
                    k.address,
                    k.name,
                    if k.decoded {
                        ""
                    } else {
                        "  (calls not decoded)"
                    }
                );
            }
        }
    }
    if !matches!(what, "fields" | "protocols" | "all") {
        bail!("choose fields, protocols or all");
    }
    say!("\nACTIONS  {}", c.actions.join(", "));
    say!("OPERATORS  == != < <= > >=  in [..]  contains  set(field)   joined by && || ! and ( )");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_a_valid_rules_file_that_compiles() {
        for (name, _) in templates::NAMES {
            let text = templates::get(name)
                .unwrap_or_else(|| panic!("template {name} is listed but missing"));
            let set = RuleSet::from_toml(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            compile(&set).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!set.profile.is_empty(), "{name}");
        }
        assert!(templates::get("nonexistent").is_none());
    }

    #[test]
    fn the_builtin_counters_and_single_rule_compile() {
        compile(&counters()).unwrap();
        compile(&single("kind == swap && swap.amount_in > 5e6")).unwrap();
        assert!(compile(&single("kind == ")).is_err());
    }

    #[test]
    fn a_broken_edit_keeps_the_previous_rules_and_a_fixed_one_reloads() {
        let dir = std::env::temp_dir().join(format!("seqscope-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.toml");
        std::fs::write(&path, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"a\"\nwhen = \"true\"\n").unwrap();
        let args = RulesArg {
            rules: path.clone(),
            when: None,
        };
        let mut live = Live::open(&args).unwrap();
        assert_eq!(live.engine().stats().len(), 1);
        let later = |live: &mut Live| {
            live.last_check = Instant::now() - Duration::from_secs(5);
            live.seen = None; // as if the file's timestamp moved
        };
        std::fs::write(&path, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"a\"\nwhen = \"nonsense ==\"\n").unwrap();
        later(&mut live);
        assert_eq!(
            live.engine().stats()[0].rule,
            "a",
            "the previous engine is still in use"
        );
        std::fs::write(&path, "version = 1\n[[profile]]\nname = \"p\"\n[[profile.rule]]\nname = \"b\"\nwhen = \"true\"\n[[profile.rule]]\nname = \"c\"\nwhen = \"true\"\n").unwrap();
        later(&mut live);
        assert_eq!(live.engine().stats().len(), 2, "the fixed file is loaded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clock_and_short_read_well() {
        assert_eq!(output::clock(1_700_000_000_123), "22:13:20.123");
        assert_eq!(
            output::short("0x1234567890abcdef1234567890abcdef12345678"),
            "0x123456…5678"
        );
        assert_eq!(output::short("0xabc"), "0xabc");
    }
}
