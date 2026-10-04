//! Missed-liquidation audit: every liquidation on our venues in the last N hours, and for each
//! whether the bot was up, knew the borrower, acted, and how long the position was open.
//!   cargo run --release -p mev-analytics --bin mev-liq-audit -- [hours=48] [data_dir ...]
//! `data_dir`s hold `events-*.jsonl` and `*_borrowers.json` (default `data/arbitrum`); pass the
//! server's copy too when the bot ran in more than one place.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use alloy::{
    eips::BlockNumberOrTag,
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider},
    rpc::types::{Filter, Log},
    sol,
    sol_types::SolEvent,
};
use anyhow::{Context, Result};
use mev_analytics::audit::{BotLog, Liquidation, RACE_SECS, Verdict, classify, first_liquidatable};
use mev_config::{ChainConfig, Secrets};
use mev_rpc::{Backoff, http_provider};

sol! {
    event LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken);
    event AbsorbDebt(address indexed absorber, address indexed borrower, uint256 basePaidOut, uint256 usdValue);
    event Liquidate(bytes32 indexed id, address indexed caller, address indexed borrower, uint256 repaidAssets, uint256 repaidShares, uint256 seizedAssets, uint256 badDebtAssets, uint256 badDebtShares);
    event LogLiquidate(address liquidator_, uint256 colAmt_, uint256 debtAmt_, address to_);

    #[sol(rpc)]
    interface IAudit {
        function getUserAccountData(address user) external view returns (uint256, uint256, uint256, uint256, uint256, uint256 healthFactor);
        function isLiquidatable(address account) external view returns (bool);
        function getAssetPrice(address asset) external view returns (uint256);
        function decimals() external view returns (uint8);
        function idToMarketParams(bytes32 id) external view returns (address loanToken, address collateralToken, address oracle, address irm, uint256 lltv);
    }
}

/// How far back to look for when a position became liquidatable (~30 min at 4 blocks/s).
const OPEN_SEARCH_BLOCKS: u64 = 7_200;
const LOG_CHUNK: u64 = 50_000;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let hours: i64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(48);
    let dirs: Vec<PathBuf> = match args.get(1..) {
        Some(d) if !d.is_empty() => d.iter().map(PathBuf::from).collect(),
        _ => vec!["data/arbitrum".into()],
    };
    let cfg = ChainConfig::load("config/arbitrum.toml".as_ref())?;
    let secrets = Secrets::from_env(&cfg.name.to_uppercase())?;
    let fast = http_provider(&secrets.http_urls()[0])?;
    let scan = http_provider(&cfg.liquidation.as_ref().context("[liquidation]")?.scan_rpc)?;

    let log = load_bot_log(&dirs)?;
    let indexed = load_indexed(&dirs)?;
    let ours: HashSet<Address> = secrets.executor.into_iter().collect();

    let head = fast.get_block_number().await?;
    let now = block_ts(&fast, head).await?;
    let from_ts = now - hours * 3600;
    let from = block_at_ts(&fast, head, from_ts).await?;
    eprintln!("scanning blocks {from}..={head} ({hours} h) for liquidations");

    let aave = cfg.aave.as_ref().map(|a| (a.pool, a.oracle));
    let morpho = cfg.morpho.as_ref().map(|m| m.morpho);
    let comets: Vec<Address> = cfg
        .compound_v3
        .as_ref()
        .map(|c| c.comets.iter().map(|x| x.address).collect())
        .unwrap_or_default();
    let vaults: Vec<Address> = cfg
        .fluid
        .as_ref()
        .map(|f| f.vaults.clone())
        .unwrap_or_default();
    let mut addresses: Vec<Address> = comets.iter().chain(&vaults).copied().collect();
    addresses.extend(aave.map(|a| a.0));
    addresses.extend(morpho);
    let topics = vec![
        LiquidationCall::SIGNATURE_HASH,
        AbsorbDebt::SIGNATURE_HASH,
        Liquidate::SIGNATURE_HASH,
        LogLiquidate::SIGNATURE_HASH,
    ];
    let logs = get_logs(&scan, &addresses, topics, from, head).await?;
    eprintln!("{} liquidation events; enriching", logs.len());

    let oracle = aave.map(|a| a.1);
    let mut rows = Vec::new();
    for l in &logs {
        let Some(mut liq) = decode(l, &fast, oracle, &comets, &vaults).await? else {
            continue;
        };
        let r = fast
            .get_transaction_receipt(liq.tx)
            .await?
            .context("receipt")?;
        liq.sender = r.from;
        liq.to = r.to;
        liq.tx_index = r.transaction_index.unwrap_or_default();
        liq.ts = block_ts(&fast, liq.block).await?;
        liq.open_secs = open_secs(&fast, &liq, aave.map(|a| a.0), l.address()).await;
        let known = match liq.protocol {
            "fluid" => true,
            p => indexed.get(p).is_some_and(|s| s.contains(&liq.borrower)),
        };
        let v = classify(&liq, &log, known, &ours);
        rows.push((liq, v));
    }
    report(&rows, &log, from_ts, now);
    Ok(())
}

async fn decode(
    l: &Log,
    p: &DynProvider,
    oracle: Option<Address>,
    comets: &[Address],
    vaults: &[Address],
) -> Result<Option<Liquidation>> {
    let base = |protocol, borrower| Liquidation {
        protocol,
        block: l.block_number.unwrap_or_default(),
        ts: 0,
        tx: l.transaction_hash.unwrap_or_default(),
        tx_index: 0,
        borrower,
        sender: Address::ZERO,
        to: None,
        debt_usd: None,
        open_secs: None,
    };
    let topic0 = l.topic0().copied().unwrap_or_default();
    Ok(Some(if topic0 == LiquidationCall::SIGNATURE_HASH {
        let e = l.log_decode::<LiquidationCall>()?.inner.data;
        let mut liq = base("aave-v3", e.user);
        if let Some(o) = oracle {
            liq.debt_usd = usd(p, o, e.debtAsset, e.debtToCover).await;
        }
        liq
    } else if topic0 == AbsorbDebt::SIGNATURE_HASH && comets.contains(&l.address()) {
        let e = l.log_decode::<AbsorbDebt>()?.inner.data;
        let mut liq = base("compound-v3", e.borrower);
        liq.debt_usd = Some(f64::from(e.usdValue) / 1e8);
        liq
    } else if topic0 == Liquidate::SIGNATURE_HASH {
        let e = l.log_decode::<Liquidate>()?.inner.data;
        let mut liq = base("morpho-blue", e.borrower);
        if let Some(o) = oracle {
            let params = IAudit::new(l.address(), p)
                .idToMarketParams(e.id)
                .call()
                .await;
            if let Ok(m) = params {
                liq.debt_usd = usd(p, o, m.loanToken, e.repaidAssets).await;
            }
        }
        liq
    } else if topic0 == LogLiquidate::SIGNATURE_HASH && vaults.contains(&l.address()) {
        // Fluid liquidates by price range: the vault is the unit.
        base("fluid", l.address())
    } else {
        return Ok(None);
    }))
}

/// Raw `amount` of `token` in USD at the Aave oracle's current price (None if unpriced).
async fn usd(p: &DynProvider, oracle: Address, token: Address, amount: U256) -> Option<f64> {
    let px = IAudit::new(oracle, p)
        .getAssetPrice(token)
        .call()
        .await
        .ok()?;
    let dec = IAudit::new(token, p).decimals().call().await.ok()?;
    Some(f64::from(amount) / 10f64.powi(dec as i32) * f64::from(px) / 1e8)
}

/// Seconds the position was liquidatable before the liquidation block (Aave and Compound only):
/// 0 = became liquidatable in the same block (a race against the price update), `i64::MAX` =
/// longer than the search window.
async fn open_secs(
    p: &DynProvider,
    l: &Liquidation,
    aave_pool: Option<Address>,
    emitter: Address,
) -> Option<i64> {
    let user = l.borrower;
    let check = |b: u64| {
        let p = p.clone();
        async move {
            let at = BlockNumberOrTag::Number(b).into();
            match l.protocol {
                "aave-v3" => IAudit::new(aave_pool?, &p)
                    .getUserAccountData(user)
                    .block(at)
                    .call()
                    .await
                    .ok()
                    .map(|d| d.healthFactor < U256::from(10u64.pow(18))),
                "compound-v3" => IAudit::new(emitter, &p)
                    .isLiquidatable(user)
                    .block(at)
                    .call()
                    .await
                    .ok(),
                _ => None,
            }
        }
    };
    let before = l.block.checked_sub(1)?;
    if !check(before).await? {
        return Some(0);
    }
    let lo = before.saturating_sub(OPEN_SEARCH_BLOCKS);
    // An RPC failure mid-search reads as "not liquidatable"; good enough for an estimate.
    match first_liquidatable(lo, before, |b| {
        let c = check(b);
        async move { c.await.unwrap_or(false) }
    })
    .await
    {
        Some(first) => Some(l.ts - block_ts(p, first).await.ok()?),
        None => Some(i64::MAX),
    }
}

async fn block_ts(p: &DynProvider, n: u64) -> Result<i64> {
    let b = p
        .get_block_by_number(BlockNumberOrTag::Number(n))
        .await?
        .with_context(|| format!("block {n}"))?;
    Ok(b.header.timestamp as i64)
}

/// First block at or after `ts` (binary search on timestamps).
async fn block_at_ts(p: &DynProvider, head: u64, ts: i64) -> Result<u64> {
    let (mut lo, mut hi) = (0u64, head);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if block_ts(p, mid).await? < ts {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(hi)
}

/// Wide getLogs in chunks with backoff (the scan RPC rate-limits).
async fn get_logs(
    p: &DynProvider,
    addresses: &[Address],
    topics: Vec<B256>,
    from: u64,
    to: u64,
) -> Result<Vec<Log>> {
    let mut out = Vec::new();
    let mut start = from;
    let mut backoff = Backoff::default();
    while start <= to {
        let end = (start + LOG_CHUNK - 1).min(to);
        let f = Filter::new()
            .address(addresses.to_vec())
            .event_signature(topics.clone())
            .from_block(start)
            .to_block(end);
        match p.get_logs(&f).await {
            Ok(mut v) => {
                out.append(&mut v);
                start = end + 1;
                backoff.reset();
            }
            Err(e) => {
                let d = backoff.next_delay();
                if d >= Backoff::MAX {
                    return Err(e).context("getLogs keeps failing");
                }
                eprintln!("getLogs {start}..{end}: {e}; retry in {d:?}");
                tokio::time::sleep(d).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(out)
}

fn load_bot_log(dirs: &[PathBuf]) -> Result<BotLog> {
    let mut text = String::new();
    for d in dirs {
        for e in std::fs::read_dir(d).with_context(|| format!("{}", d.display()))? {
            let path = e?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with("events-") && name.ends_with(".jsonl") {
                text.push_str(&std::fs::read_to_string(&path)?);
                text.push('\n');
            }
        }
    }
    Ok(BotLog::parse(text.lines()))
}

/// Borrowers per protocol id from the account index caches (`{"accounts": [[key, address], ...]}`).
fn load_indexed(dirs: &[PathBuf]) -> Result<HashMap<&'static str, HashSet<Address>>> {
    let mut out: HashMap<&'static str, HashSet<Address>> = HashMap::new();
    for d in dirs {
        for (file, id) in [
            ("aave_v3_borrowers.json", "aave-v3"),
            ("compound_v3_borrowers.json", "compound-v3"),
            ("morpho_borrowers.json", "morpho-blue"),
        ] {
            let Ok(text) = std::fs::read_to_string(d.join(file)) else {
                continue;
            };
            let v: serde_json::Value = serde_json::from_str(&text)?;
            let set = out.entry(id).or_default();
            for pair in v["accounts"].as_array().into_iter().flatten() {
                if let Some(a) = pair[1].as_str().and_then(|s| s.parse().ok()) {
                    set.insert(a);
                }
            }
        }
    }
    Ok(out)
}

fn report(rows: &[(Liquidation, Verdict)], log: &BotLog, from_ts: i64, now: i64) {
    let fmt_ts = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0)
            .map(|d| d.format("%m-%d %H:%M:%S").to_string())
            .unwrap_or_default()
    };
    let fmt_open = |o: Option<i64>| match o {
        None => "-".to_string(),
        Some(i64::MAX) => ">30m".to_string(),
        Some(s) => format!("{s}s"),
    };
    println!(
        "{:<14} {:<12} {:>10} {:>6} {:>4}  {:<12} verdict",
        "time (UTC)", "protocol", "debt_usd", "open", "idx", "liquidator"
    );
    let mut rows: Vec<_> = rows.iter().collect();
    rows.sort_by_key(|(l, _)| l.ts);
    for (l, v) in &rows {
        println!(
            "{:<14} {:<12} {:>10} {:>6} {:>4}  {:<12} {:?}",
            fmt_ts(l.ts),
            l.protocol,
            l.debt_usd.map(|d| format!("{d:.0}")).unwrap_or("?".into()),
            fmt_open(l.open_secs),
            l.tx_index,
            format!("{:.10}", l.to.unwrap_or(l.sender).to_string()),
            v
        );
    }
    let mut by: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for (l, v) in &rows {
        let key = match v {
            Verdict::Silent => match l.open_secs {
                Some(s) if s < RACE_SECS => "Silent (same-block race)".into(),
                Some(_) => "Silent (was open: should have acted)".into(),
                None => "Silent (open time unknown)".into(),
            },
            other => format!("{other:?}"),
        };
        let e = by.entry(key).or_default();
        e.0 += 1;
        e.1 += l.debt_usd.unwrap_or(0.0);
    }
    let window_min = ((now - from_ts) / 60).max(1);
    let up = log.up_minutes_between(from_ts, now);
    println!(
        "\nbot uptime in window: {up}/{window_min} min ({:.0}%)",
        100.0 * up as f64 / window_min as f64
    );
    println!("{:<40} {:>5} {:>12}", "verdict", "count", "debt_usd");
    for (k, (n, usd)) in by {
        println!("{k:<40} {n:>5} {usd:>12.0}");
    }
}
