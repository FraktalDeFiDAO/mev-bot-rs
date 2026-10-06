//! Read-only readiness check for the Atlas solver path. Needs no funds and sends nothing:
//!
//!   mev-atlas-check [--rpc URL] [--listen SECS]
//!
//! 1. reads `AtlasVerification.eip712Domain()` and compares it with our signing domain;
//! 2. builds a sample solver operation and compares our EIP-712 digest with the contract's
//!    `getSolverPayload` (proves the signing code matches the deployed verifier);
//! 3. with `--listen`, connects to Chainlink's auction feed and prints what arrives (no bids).

use std::time::Duration;

use alloy::{
    primitives::{Address, B256, Bytes, U256, address},
    providers::ProviderBuilder,
    sol,
};
use anyhow::{Context, Result};
use mev_atlas::{ATLAS, ATLAS_VERIFICATION, ATLAS_VERSION, SOLVER_WS, client, sign};
use tokio::sync::mpsc;

sol! {
    #[sol(rpc)]
    interface IAtlasVerification {
        function eip712Domain() external view returns (bytes1 fields, string name, string version, uint256 chainId, address verifyingContract, bytes32 salt, uint256[] extensions);
        function getSolverPayload(SolverOp solverOp) external view returns (bytes32 payload);
    }
    struct SolverOp {
        address from; address to; uint256 value; uint256 gas; uint256 maxFeePerGas; uint256 deadline;
        address solver; address control; bytes32 userOpHash; address bidToken; uint256 bidAmount;
        bytes data; bytes signature;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut rpc = "https://arb1.arbitrum.io/rpc".to_string();
    let mut listen = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rpc" => rpc = args.next().context("--rpc needs a URL")?,
            "--listen" => listen = args.next().context("--listen needs seconds")?.parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let provider = ProviderBuilder::new().connect_http(rpc.parse()?);
    let verifier = IAtlasVerification::new(ATLAS_VERIFICATION, &provider);

    let d = verifier
        .eip712Domain()
        .call()
        .await
        .context("eip712Domain()")?;
    println!(
        "on-chain domain: name={} version={} chainId={} verifier={}",
        d.name, d.version, d.chainId, d.verifyingContract
    );
    anyhow::ensure!(d.name == "AtlasVerification", "domain name differs");
    anyhow::ensure!(
        d.version == ATLAS_VERSION,
        "domain version differs from {ATLAS_VERSION}"
    );
    anyhow::ensure!(d.chainId == U256::from(42161), "chain id differs");
    anyhow::ensure!(
        d.verifyingContract == ATLAS_VERIFICATION,
        "verifying contract differs"
    );
    println!("domain: OK");

    let op = sign::SolverOperation {
        from: address!("1111111111111111111111111111111111111111"),
        to: ATLAS,
        value: U256::ZERO,
        gas: U256::from(500_000u64),
        maxFeePerGas: U256::from(39_820_200u64),
        deadline: U256::from(512_000_000u64),
        solver: address!("2222222222222222222222222222222222222222"),
        control: address!("e15bba987c002ecc3586e81244517877d294d291"),
        userOpHash: B256::repeat_byte(0x33),
        bidToken: Address::ZERO,
        bidAmount: U256::from(123_456_789u64),
        data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef, 0x01]),
    };
    let ours = sign::digest(&op, 42161);
    let theirs = verifier
        .getSolverPayload(SolverOp {
            from: op.from,
            to: op.to,
            value: op.value,
            gas: op.gas,
            maxFeePerGas: op.maxFeePerGas,
            deadline: op.deadline,
            solver: op.solver,
            control: op.control,
            userOpHash: op.userOpHash,
            bidToken: op.bidToken,
            bidAmount: op.bidAmount,
            data: op.data.clone(),
            signature: Bytes::new(),
        })
        .call()
        .await
        .context("getSolverPayload")?;
    println!("digest ours={ours} chain={theirs}");
    anyhow::ensure!(
        ours == theirs,
        "OUR EIP-712 DIGEST DIFFERS FROM THE DEPLOYED VERIFIER: do not bid"
    );
    println!("digest: OK (signing matches the deployed AtlasVerification)");

    if listen > 0 {
        println!("listening to {SOLVER_WS} for {listen}s (no bids)");
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (e_tx, mut e_rx) = mpsc::unbounded_channel();
        let (_s_tx, s_rx) = mpsc::unbounded_channel();
        tokio::spawn(client::run(SOLVER_WS.to_string(), a_tx, e_tx, s_rx));
        let end = tokio::time::sleep(Duration::from_secs(listen));
        tokio::pin!(end);
        let mut n = 0;
        loop {
            tokio::select! {
                _ = &mut end => break,
                Some(a) = a_rx.recv() => {
                    n += 1;
                    println!("auction {} chain={} feed={:?} price={:?} max_fee={} deadline={} control={}",
                        a.id, a.chain_id, a.hints.as_ref().map(|h| h.aggregator), a.hints.as_ref().map(|h| h.median_price), a.max_fee_per_gas, a.deadline, a.control);
                }
                Some(e) = e_rx.recv() => println!("event: {e:?}"),
            }
        }
        println!("{n} auctions seen");
    }
    Ok(())
}
