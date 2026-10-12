//! Create the first durable risk ledger for live mode from canonical chain evidence.
//!
//!   mev-ledger-seed --rpc URL --owner 0x.. --hashes sent-hashes.txt --native-usd 3000 --out data/arbitrum/risk-state.json
//!
//! `--hashes` lists the wallet's transaction hashes (one per line; take them from the `sent` events in
//! `events-*.jsonl`). Receipts of the current UTC day and anything still pending must all be covered, else the
//! tool refuses: a ledger that omits spend already made would re-admit it. The output is a candidate file; the
//! tool never replaces an existing one and never signs or sends anything. Review it before the first live start.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use alloy::{
    consensus::{BlockHeader, Transaction as _},
    eips::BlockNumberOrTag,
    network::{ReceiptResponse, TransactionResponse},
    primitives::{Address, B256},
    providers::Provider,
};
use anyhow::{Context, Result, bail, ensure};
use mev_analytics::seed::{Mined, Params, TxInfo, assemble};
use mev_risk::{cost_microusd, ledger::Ledger};

struct Args {
    rpc: String,
    owner: Address,
    hashes: PathBuf,
    native_usd: f64,
    out: PathBuf,
    chain_id: u64,
}

fn parse() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let (mut rpc, mut owner, mut hashes, mut usd, mut out, mut chain) =
        (None, None, None, None, None, 42161u64);
    while let Some(flag) = it.next() {
        let v = it.next().with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--rpc" => rpc = Some(v),
            "--owner" => owner = Some(v.parse::<Address>().context("invalid --owner")?),
            "--hashes" => hashes = Some(PathBuf::from(v)),
            "--native-usd" => usd = Some(v.parse::<f64>().context("invalid --native-usd")?),
            "--out" => out = Some(PathBuf::from(v)),
            "--chain-id" => chain = v.parse().context("invalid --chain-id")?,
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(Args {
        rpc: rpc.context("--rpc required")?,
        owner: owner.context("--owner required")?,
        hashes: hashes.context("--hashes required")?,
        native_usd: usd.context("--native-usd required (use a conservative high quote)")?,
        out: out.context("--out required")?,
        chain_id: chain,
    })
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{}", mev_types::redact_secrets(&format!("{e:#}")));
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let a = parse()?;
    ensure!(
        a.native_usd.is_finite() && a.native_usd > 0.0,
        "--native-usd must be positive"
    );
    let price = cost_microusd(a.native_usd)?;
    ensure!(
        !a.out.exists(),
        "{} already exists; refusing to replace accounting",
        a.out.display()
    );
    let p = mev_rpc::http_provider(&a.rpc)?;
    ensure!(
        p.get_chain_id().await? == a.chain_id,
        "RPC is not chain {}",
        a.chain_id
    );

    // One canonical reference block; every count and receipt is read relative to it.
    let head = p
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .context("no latest block")?;
    let (block, block_hash, now) = (head.header.number, head.header.hash, head.header.timestamp);
    let confirmed = p
        .get_transaction_count(a.owner)
        .block_id(block.into())
        .await?;
    let pending = p.get_transaction_count(a.owner).pending().await?;

    let mut txs: BTreeMap<u64, TxInfo> = BTreeMap::new();
    let mut mined: HashMap<B256, Mined> = HashMap::new();
    let text = std::fs::read_to_string(&a.hashes).context("reading --hashes")?;
    let mut seen = std::collections::HashSet::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let hash: B256 = line.parse().with_context(|| format!("bad hash {line}"))?;
        if !seen.insert(hash) {
            continue;
        }
        let Some(tx) = p.get_transaction_by_hash(hash).await? else {
            continue; // dropped or replaced: not part of the wallet's state
        };
        if tx.from() != a.owner {
            bail!("{hash} was not sent by {}", a.owner);
        }
        let nonce = tx.nonce();
        let receipt = p.get_transaction_receipt(hash).await?;
        if let Some(r) = &receipt {
            let bn = r.block_number().context("receipt without block")?;
            ensure!(
                bn <= block,
                "receipt {hash} is newer than reference block {block}"
            );
            let b = p
                .get_block_by_number(bn.into())
                .await?
                .context("receipt block missing")?;
            ensure!(
                Some(b.header.hash) == r.block_hash(),
                "receipt {hash} is not on the canonical chain"
            );
            mined.insert(
                hash,
                Mined {
                    block_ts: b.header.timestamp,
                    base_fee: u128::from(b.header.base_fee_per_gas().unwrap_or(0)),
                    gas_used: r.gas_used(),
                    effective_price: r.effective_gas_price(),
                    success: r.status(),
                },
            );
        }
        let info = TxInfo {
            hash,
            nonce,
            from: a.owner,
            gas_limit: tx.gas_limit(),
            max_fee: alloy::consensus::Transaction::max_fee_per_gas(&tx),
            max_priority: alloy::consensus::Transaction::max_priority_fee_per_gas(&tx).unwrap_or(0),
        };
        // Replaced transactions share a nonce; only the one that was mined (or, pending, the live one) holds it.
        match txs.get(&nonce) {
            Some(prev) if mined.contains_key(&prev.hash) && !mined.contains_key(&hash) => {}
            _ => {
                txs.insert(nonce, info);
            }
        }
    }
    let seed = assemble(
        &Params {
            chain_id: a.chain_id,
            owner: a.owner,
            block,
            block_hash,
            now,
            confirmed_nonce: confirmed,
            pending_nonce: pending,
            native_price_microusd: price,
        },
        &txs,
        &mined,
    )?;
    let (receipts, pend, start) = (seed.receipts.len(), seed.pending.len(), seed.starting_nonce);
    Ledger::initialize(a.out.clone(), seed)?;
    println!(
        "wrote {} (block {block}, {receipts} receipts from nonce {start}, {pend} pending). Review before the first live start.",
        a.out.display()
    );
    Ok(())
}
