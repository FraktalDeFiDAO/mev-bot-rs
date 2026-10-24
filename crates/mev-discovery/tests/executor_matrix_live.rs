//! Live ground truth (ignored; read-only `eth_call` at one pinned block): two-hop and triangular routes over
//! every pool the bot discovers among its configured tokens, every exchange type, every decimal mix the chain
//! offers, run through the **deployed executor** and compared with the bot's exact integer prediction.
//!
//! The executor is asked for an impossible `minProfit`, so every route reverts with
//! `NotProfitable(balance, required)`: `balance` minus the executor's balance before the call is what the real
//! pools actually paid out at the end of the route (flash loan, every hop's transfer and callback, the chain's
//! own rounding). That must equal the model to the unit on exact hops, and be at least the model on bounded
//! (Algebra) hops.
//!
//!   MEV_IT_CHAIN=base BASE_HTTP_URL=https://base-rpc.publicnode.com BASE_WS_URL=wss://base-rpc.publicnode.com \
//!     cargo test -p mev-discovery --test executor_matrix_live -- --ignored --nocapture
//!   MEV_IT_CHAIN=arbitrum ARBITRUM_HTTP_URL=... ARBITRUM_WS_URL=... (same command)
//! `MEV_IT_V4=0` skips Uniswap V4 discovery (it scans recent logs).

use std::collections::{BTreeMap, HashMap};

use alloy::{
    primitives::{Address, U256, address},
    providers::Provider,
    rpc::types::TransactionRequest,
    sol,
};
use mev_dex::Pool;
use mev_discovery::{discover_pools, load_pools_at, ticks::load_ticks_at, token_decimals, v4};
use mev_types::{Hop, Opportunity, PoolKind};

sol! {
    #[sol(rpc)]
    interface IErc20 { function balanceOf(address) external view returns (uint256); }
}

/// `NotProfitable(uint256 balance, uint256 required)`.
const NOT_PROFITABLE: [u8; 4] = [0x84, 0x3e, 0x30, 0xec];

struct Chain {
    config: &'static str,
    http: &'static str,
    ws: &'static str,
    executor: Address,
    owner: Address,
}

fn chain() -> Chain {
    let owner = address!("e7322F17Ed69B9236f5103f8F62B302Cbf984447");
    match std::env::var("MEV_IT_CHAIN").as_deref().unwrap_or("base") {
        "arbitrum" => Chain {
            config: "../../config/arbitrum.toml",
            http: "ARBITRUM_HTTP_URL",
            ws: "ARBITRUM_WS_URL",
            executor: address!("17357E33012e8c05E654a7D1CC7D9dedd6B63acf"),
            owner,
        },
        _ => Chain {
            config: "../../config/base.toml",
            http: "BASE_HTTP_URL",
            ws: "BASE_WS_URL",
            executor: address!("5099a477b5989afA8fC061462506855930B83B2E"),
            owner,
        },
    }
}

fn kind_name(k: &PoolKind) -> &'static str {
    match k {
        PoolKind::UniV2 { .. } => "v2",
        PoolKind::CamelotV2 => "camelot-v2",
        PoolKind::SolidlyV2 { .. } => "solidly",
        PoolKind::UniV3 { .. } => "v3",
        PoolKind::PancakeV3 { .. } => "pancake-v3",
        PoolKind::SlipstreamV3 { .. } => "slipstream",
        PoolKind::AlgebraV3 => "algebra",
        PoolKind::UniV4 { flipped: false, .. } => "v4",
        PoolKind::UniV4 { flipped: true, .. } => "v4-flipped",
    }
}

fn hop(p: &Pool, tin: Address) -> Hop {
    let tout = p.info.other(tin);
    Hop {
        kind: p.info.kind.hop_kind(),
        pool: p.info.address,
        token_in: tin,
        token_out: tout,
        zero_for_one: tin == p.info.token0,
        fee_bps: match p.info.kind {
            PoolKind::UniV2 { fee_bps } => fee_bps,
            PoolKind::UniV4 { fee, .. } => u16::try_from(fee).unwrap_or(0),
            _ => 0,
        },
        spacing: match p.info.kind {
            PoolKind::UniV4 { tick_spacing, .. } => tick_spacing,
            _ => 0,
        },
    }
}

/// Keep `path` unless two routes with the same kind path and decimal path are already kept.
fn keep<'a>(
    path: Vec<(&'a Pool, Address)>,
    routes: &mut Vec<Vec<(&'a Pool, Address)>>,
    seen: &mut HashMap<String, usize>,
    decimals: &HashMap<Address, u8>,
) {
    let n = seen.entry(path_key(&path, decimals)).or_default();
    if *n < 2 {
        *n += 1;
        routes.push(path);
    }
}

fn path_key(path: &[(&Pool, Address)], decimals: &HashMap<Address, u8>) -> String {
    path.iter()
        .map(|(p, tin)| {
            format!(
                "{}:{}>{}",
                kind_name(&p.info.kind),
                decimals[tin],
                decimals[&p.info.other(*tin)]
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The model's output for `path`, and whether every hop was exact (else a guaranteed lower bound).
fn predict(path: &[(&Pool, Address)], amount: U256) -> Option<(U256, bool)> {
    let (mut x, mut exact) = (amount, true);
    for (p, tin) in path {
        let (o, e) = p.exact_amount_out_bound(*tin, x)?;
        x = o;
        exact &= e;
    }
    Some((x, exact))
}

#[tokio::test]
#[ignore = "network"]
async fn executor_output_equals_the_exact_model_over_every_pool_kind_and_decimal_mix() {
    let c = chain();
    let http = std::env::var(c.http).unwrap();
    let ws = std::env::var(c.ws).unwrap();
    let rpc = mev_rpc::Rpc::connect(std::slice::from_ref(&http), &[ws])
        .await
        .unwrap();
    let cfg = mev_config::ChainConfig::load(std::path::Path::new(c.config)).unwrap();
    let tokens: Vec<Address> = cfg.tokens.values().copied().collect();
    let decimals = token_decimals(&rpc, &tokens).await.unwrap();
    let loans: Vec<Address> = cfg
        .loan_tokens
        .iter()
        .map(|s| cfg.token(s).unwrap())
        .collect();

    // Every pool of every configured exchange among the configured tokens (and V4's hookless ones).
    let mut pairs = Vec::new();
    for (i, a) in tokens.iter().enumerate() {
        for b in &tokens[i + 1..] {
            pairs.push((*a, *b));
        }
    }
    let mut infos = discover_pools(&rpc, &cfg, &pairs).await.unwrap();
    if std::env::var("MEV_IT_V4").as_deref() != Ok("0")
        && let Some(v4cfg) = cfg.uniswap_v4.clone().filter(|v| v.enabled)
    {
        let registry = std::env::temp_dir().join(format!("mev-it-v4-{}.json", cfg.chain_id));
        infos.extend(
            v4::discover(&cfg, &v4cfg, &registry)
                .await
                .into_iter()
                .filter(|i| tokens.contains(&i.token0) && tokens.contains(&i.token1)),
        );
    }
    let (block, number) = mev_rpc::snapshot_block(&rpc.http).await.unwrap();
    let mut pools = load_pools_at(&rpc, infos, block).await.unwrap();
    let maps = load_ticks_at(&rpc, &pools, 4000, block).await.unwrap();
    for p in &mut pools {
        p.ticks = maps.get(&p.info.address).cloned().map(std::sync::Arc::new);
    }
    // Pools the model can price at all (exact state loaded, some liquidity).
    pools.retain(|p| {
        let probe = U256::from(10u8).pow(U256::from(decimals[&p.info.token0] / 2));
        p.exact_amount_out_bound(p.info.token0, probe).is_some()
    });
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &pools {
        *kinds.entry(kind_name(&p.info.kind)).or_default() += 1;
    }
    println!("block {number}: {} priceable pools {kinds:?}", pools.len());

    // Routes: L -> X -> L over any two pools of the pair (the same pool both ways included), and
    // L -> X -> Y -> L triangles, at most two per (kind path, decimal path) so every combination is
    // represented without thousands of calls.
    let by_pair = |a: Address, b: Address| -> Vec<&Pool> {
        pools
            .iter()
            .filter(|p| p.info.has(a) && p.info.has(b))
            .collect()
    };
    let mut routes: Vec<Vec<(&Pool, Address)>> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for &l in &loans {
        for p in pools.iter().filter(|p| p.info.has(l)) {
            let x = p.info.other(l);
            // Distinct pools only, as the search builds them: a second hop through the same pool would see
            // the state the first hop left, which a route over one snapshot does not model.
            for q in by_pair(l, x)
                .into_iter()
                .filter(|q| q.info.address != p.info.address)
            {
                keep(vec![(p, l), (q, x)], &mut routes, &mut seen, &decimals);
            }
            for p2 in pools.iter().filter(|p2| p2.info.has(x) && !p2.info.has(l)) {
                let y = p2.info.other(x);
                for p3 in by_pair(y, l)
                    .into_iter()
                    .filter(|p3| p3.info.address != p.info.address)
                {
                    keep(
                        vec![(p, l), (p2, x), (p3, y)],
                        &mut routes,
                        &mut seen,
                        &decimals,
                    );
                }
            }
        }
    }
    println!(
        "{} routes over {} kind/decimal paths",
        routes.len(),
        seen.len()
    );

    // The executor's balance of each loan token before the call: subtracted from `balance`.
    let mut before = HashMap::new();
    // The flash lender's balance caps the loan, as the bot's own loan cap does.
    let mut lendable = HashMap::new();
    for &l in &loans {
        let v = IErc20::new(l, &rpc.http)
            .balanceOf(cfg.balancer_vault)
            .block(block)
            .call()
            .await
            .unwrap();
        lendable.insert(l, v);
        let b = IErc20::new(l, &rpc.http)
            .balanceOf(c.executor)
            .block(block)
            .call()
            .await
            .unwrap();
        before.insert(l, b);
    }
    let min_profit = U256::from(1u8) << 200;
    let (mut equal, mut bounded, mut declined) = (0usize, 0usize, 0usize);
    let mut other_reverts: BTreeMap<String, usize> = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut covered: BTreeMap<String, usize> = BTreeMap::new();
    let limit: usize = std::env::var("MEV_IT_MAX_ROUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    for path in routes.iter().take(limit) {
        let (first, loan) = path[0];
        let dl = decimals[&loan];
        let _ = first;
        // Sizes the model will price: the largest power of ten it accepts (crossing ticks up to the loaded
        // window), a hundredth and a ten-thousandth of it, and one whole loan token if accepted.
        let mut top = None;
        for k in (0..=36u32).rev() {
            let a = U256::from(10u8).pow(U256::from(k));
            if a <= lendable[&loan] / U256::from(2u8) && predict(path, a).is_some() {
                top = Some(a);
                break;
            }
        }
        let Some(top) = top else {
            declined += 1;
            continue;
        };
        let mut sizes: Vec<U256> = [U256::from(1u8), U256::from(100u8), U256::from(10_000u32)]
            .into_iter()
            .map(|d| top / d * U256::from(7u8) / U256::from(5u8) + U256::from(dl + 1))
            .filter(|a| !a.is_zero() && predict(path, *a).is_some())
            .collect();
        let whole = U256::from(10u8).pow(U256::from(dl));
        if whole <= lendable[&loan] / U256::from(2u8) && predict(path, whole).is_some() {
            sizes.push(whole);
        }
        for amount in sizes {
            let Some((want, exact)) = predict(path, amount) else {
                declined += 1; // beyond the loaded ticks: the model refuses, as it does live
                continue;
            };
            let opp = Opportunity {
                loan_token: loan,
                amount_in: amount,
                hops: path.iter().map(|(p, tin)| hop(p, *tin)).collect(),
                est_profit: 0.0,
                est_profit_usd: 0.0,
                buy_pool: Address::ZERO,
                sell_pool: Address::ZERO,
                exact: true,
                versions: vec![],
            };
            let req = TransactionRequest::default()
                .from(c.owner)
                .to(c.executor)
                .input(mev_bindings::arb_calldata(&opp, min_profit, false).into());
            let label = path
                .iter()
                .map(|(p, tin)| {
                    format!(
                        "{}[{}] {}dp>{}dp",
                        kind_name(&p.info.kind),
                        p.info.address,
                        decimals[tin],
                        decimals[&p.info.other(*tin)]
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            // Public endpoints rate-limit: retry transport errors; a revert is an answer.
            let mut data = None;
            let mut last = String::new();
            for attempt in 0..6u64 {
                match rpc.http.call(req.clone()).block(block).await {
                    Ok(_) => panic!("{label}: an impossible minProfit cannot succeed"),
                    Err(e) => match e.as_error_resp() {
                        Some(r) if r.as_revert_data().is_some() || r.message.contains("revert") => {
                            data = r.as_revert_data();
                            last = r.message.to_string();
                            break;
                        }
                        _ => {
                            last = e.to_string();
                            tokio::time::sleep(std::time::Duration::from_millis(
                                500 * (attempt + 1),
                            ))
                            .await;
                        }
                    },
                }
            }
            let Some(data) = data
                .clone()
                .filter(|d| d.len() >= 68 && d[..4] == NOT_PROFITABLE)
            else {
                let reason = data.clone().map_or_else(
                    || {
                        format!(
                            "no revert data ({})",
                            last.chars().take(80).collect::<String>()
                        )
                    },
                    |d| {
                        // Error(string): show the message.
                        if d.len() >= 68 && d[..4] == [0x08, 0xc3, 0x79, 0xa0] {
                            let len = U256::from_be_slice(&d[36..68]).saturating_to::<usize>();
                            String::from_utf8_lossy(&d[68..(68 + len).min(d.len())]).into_owned()
                        } else {
                            format!("0x{}", alloy::hex::encode(&d[..d.len().min(4)]))
                        }
                    },
                );
                let mut x = amount;
                let outs: Vec<String> = path
                    .iter()
                    .map(|(p, tin)| {
                        x = p
                            .exact_amount_out_bound(*tin, x)
                            .map_or(U256::ZERO, |(o, _)| o);
                        x.to_string()
                    })
                    .collect();
                if other_reverts.len() < 40 {
                    println!("revert {reason}: amount {amount} hop outputs {outs:?} on {label}");
                }
                // The model quoted this route, so the chain must not revert for any other reason; only a
                // transport error that outlived the retries is not an answer.
                if data.is_some() {
                    mismatches.push(format!(
                        "{label} amount {amount}: model quotes {outs:?}, executor reverts {reason}"
                    ));
                } else {
                    *other_reverts
                        .entry(format!("{reason} on {label}"))
                        .or_default() += 1;
                }
                continue;
            };
            let balance = U256::from_be_slice(&data[4..36]);
            let got = balance - before[&loan];
            *covered.entry(path_key(path, &decimals)).or_default() += 1;
            if exact {
                if got == want {
                    equal += 1;
                } else {
                    mismatches.push(format!(
                        "{label} amount {amount}: executor {got}, model {want}"
                    ));
                }
            } else if got >= want {
                bounded += 1;
            } else {
                mismatches.push(format!(
                    "{label} amount {amount}: executor {got} below the model's bound {want}"
                ));
            }
        }
    }
    println!(
        "block {number}: {equal} routes equal to the unit, {bounded} at or above the Algebra bound, {declined} declined by the model"
    );
    for (k, n) in &covered {
        println!("  {n:>3}  {k}");
    }
    for (r, n) in &other_reverts {
        println!("  other revert x{n}: {r}");
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    assert!(equal >= 50, "too few routes compared: {equal}");
}
