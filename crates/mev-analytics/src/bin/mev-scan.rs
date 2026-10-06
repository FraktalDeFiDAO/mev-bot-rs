//! One-shot: bootstrap the engine (shadow), search every pool now, simulate the best routes
//! through the deployed executor, and print model estimate vs on-chain result.
//!   cargo run --release -p mev-analytics --bin mev-scan -- [config/arbitrum.toml] [max_sims=25]

use anyhow::Result;
use mev_config::{ChainConfig, Secrets};
use mev_engine::{Engine, Mode, SimOutcome};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().collect();
    let cfg = ChainConfig::load(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("config/arbitrum.toml")
            .as_ref(),
    )?;
    let max: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25);
    let secrets = Secrets::from_env(&cfg.name.to_uppercase())?;
    let dir = std::env::temp_dir().join("mev-scan");
    let engine = Engine::bootstrap(cfg, secrets, Mode::Shadow, dir).await?;
    let results = engine.scan_once(max).await;
    println!(
        "{} pools, {} candidate routes simulated (no profit floor)\n",
        engine.pool_count(),
        results.len()
    );
    println!(
        "{:>9} {:>11}  {:<7} {:<11} -> {:<11} result",
        "est_usd", "amount", "loan", "buy", "sell"
    );
    for r in results {
        let outcome = match &r.sim {
            Some(SimOutcome::Profitable { profit, gas }) => {
                format!("PROFIT ${:.4} gas={gas}", f64::from(*profit) * r.loan_px)
            }
            Some(SimOutcome::Reverted(why)) => {
                format!("revert: {}", why.chars().take(90).collect::<String>())
            }
            None => "not simulated (no executor)".into(),
        };
        println!(
            "{:>9.4} {:>11.2}  {:<7} {:<11} -> {:<11} {}",
            r.opp.est_profit_usd,
            f64::from(r.opp.amount_in) * r.loan_px,
            r.loan.chars().take(7).collect::<String>(),
            r.buy,
            r.sell,
            outcome
        );
        // The route itself: kind (0/1 V2, 2 V3-style, 3/4 Uniswap V4 ERC20/native) and tokens.
        let hops: Vec<String> = r
            .opp
            .hops
            .iter()
            .map(|h| {
                format!(
                    "k{} {}->{} {}",
                    h.kind,
                    &h.token_in.to_string()[..8],
                    &h.token_out.to_string()[..8],
                    &h.pool.to_string()[..8]
                )
            })
            .collect();
        println!("            route: {}", hops.join(" | "));
    }
    Ok(())
}
