//! Transaction building, signing, broadcast (to several endpoints at once), and receipt tracking.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use alloy::{
    eips::eip2718::Encodable2718,
    network::{EthereumWallet, NetworkTransactionBuilder, TransactionBuilder},
    primitives::{Address, TxHash, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};
use anyhow::{Context, Result, bail};
use mev_bindings::{ArbExecutor, to_sol_call, to_sol_hop};
use mev_lending::Plan;
use mev_types::{Hop, Opportunity};
use tracing::warn;

pub struct Sender {
    wallet: EthereumWallet,
    pub address: Address,
    reader: DynProvider,
    broadcast: Vec<DynProvider>,
    executor: Address,
    chain_id: u64,
    nonce: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub success: bool,
    pub gas_used: u64,
    pub effective_gas_price: u128,
    pub block: Option<u64>,
    /// Position in the block (race telemetry: who was ahead of us).
    pub tx_index: Option<u64>,
}

/// A broadcast transaction and its timing.
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    pub hash: TxHash,
    /// When it was signed (just before broadcasting).
    pub signed: std::time::Instant,
    /// When the first endpoint accepted it (the sequencer usually; the time that matters).
    pub first_accept: Option<std::time::Instant>,
}

impl Sender {
    /// `extra_broadcast` = additional raw-tx endpoints (e.g. the Arbitrum sequencer).
    pub async fn new(
        private_key: &str,
        reader: DynProvider,
        extra_broadcast: &[String],
        executor: Address,
        chain_id: u64,
    ) -> Result<Self> {
        let signer: PrivateKeySigner = private_key.trim().parse().context("invalid PRIVATE_KEY")?;
        let address = signer.address();
        let mut broadcast = vec![reader.clone()];
        for url in extra_broadcast {
            broadcast.push(ProviderBuilder::new().connect_http(url.parse()?).erased());
        }
        let nonce = reader.get_transaction_count(address).pending().await?;
        Ok(Self {
            wallet: EthereumWallet::from(signer),
            address,
            reader,
            broadcast,
            executor,
            chain_id,
            nonce: AtomicU64::new(nonce),
        })
    }

    pub async fn eth_balance(&self) -> Result<U256> {
        Ok(self.reader.get_balance(self.address).await?)
    }

    /// Send an arbitrage through the executor. `gas_price` (wei): a cached value saves a round
    /// trip on the latency-critical path; `None` fetches one.
    pub async fn send(
        &self,
        opp: &Opportunity,
        min_profit: U256,
        gas_limit: u64,
        gas_price: Option<u128>,
    ) -> Result<Sent> {
        let calldata = ArbExecutor::executeCall {
            token: opp.loan_token,
            amount: opp.amount_in,
            minProfit: min_profit,
            hops: opp.hops.iter().map(to_sol_hop).collect(),
        }
        .abi_encode();
        self.send_calldata(calldata, gas_limit, gas_price).await
    }

    /// Send a liquidation (any protocol) through the executor.
    pub async fn send_liquidation(
        &self,
        plan: &Plan,
        hops: &[Hop],
        min_profit: U256,
        gas_limit: u64,
    ) -> Result<TxHash> {
        self.send_calldata(liquidate_calldata(plan, hops, min_profit), gas_limit, None)
            .await
            .map(|s| s.hash)
    }

    /// Latency-critical liquidation (backrun): uses the caller's cached `gas_price` (wei)
    /// instead of fetching one first.
    pub async fn send_liquidation_fast(
        &self,
        plan: &Plan,
        hops: &[Hop],
        min_profit: U256,
        gas_limit: u64,
        gas_price: u128,
    ) -> Result<Sent> {
        self.send_calldata(
            liquidate_calldata(plan, hops, min_profit),
            gas_limit,
            Some(gas_price),
        )
        .await
    }

    /// Sign once and broadcast the same raw tx to every endpoint; first success wins.
    async fn send_calldata(
        &self,
        calldata: Vec<u8>,
        gas_limit: u64,
        gas_price: Option<u128>,
    ) -> Result<Sent> {
        let base_fee = match gas_price {
            Some(g) => g,
            None => self.reader.get_gas_price().await?,
        };
        match self
            .sign_and_broadcast(&calldata, gas_limit, base_fee)
            .await
        {
            // The wallet was used outside the bot (or a tx was dropped): the cached nonce is stale.
            // Resync from the chain and re-sign once.
            Err(e) if is_nonce_error(&e) => {
                let n = self
                    .reader
                    .get_transaction_count(self.address)
                    .pending()
                    .await?;
                warn!(resynced_nonce = n, error = %e, "stale nonce; resyncing and retrying once");
                self.nonce.store(n, Ordering::SeqCst);
                self.sign_and_broadcast(&calldata, gas_limit, base_fee)
                    .await
            }
            other => other,
        }
    }

    async fn sign_and_broadcast(
        &self,
        calldata: &[u8],
        gas_limit: u64,
        base_fee: u128,
    ) -> Result<Sent> {
        let nonce = self.nonce.fetch_add(1, Ordering::SeqCst);
        let tx = TransactionRequest::default()
            .with_from(self.address)
            .with_to(self.executor)
            .with_input(calldata.to_vec())
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(gas_limit)
            .with_max_fee_per_gas(base_fee * 2)
            .with_max_priority_fee_per_gas(0);
        let envelope = tx.build(&self.wallet).await.context("signing failed")?;
        let raw = envelope.encoded_2718();
        let hash = *envelope.tx_hash();
        let signed = std::time::Instant::now();

        let sends = self.broadcast.iter().map(|p| {
            let raw = raw.clone();
            async move {
                let r = p.send_raw_transaction(&raw).await.map(|_| ());
                (r, std::time::Instant::now())
            }
        });
        // All endpoints at once: the sequencer endpoint must not wait behind the RPC.
        let timed = futures::future::join_all(sends).await;
        let first_accept = timed
            .iter()
            .filter(|(r, _)| r.is_ok())
            .map(|(_, t)| *t)
            .min();
        let results: Vec<_> = timed.into_iter().map(|(r, _)| r).collect();
        if results.iter().all(|r| r.is_err()) {
            // Nothing accepted it — give the nonce back.
            self.nonce.fetch_sub(1, Ordering::SeqCst);
            bail!(
                "all broadcasts failed: {:?}",
                results.into_iter().filter_map(|r| r.err()).next()
            );
        }
        for e in results.into_iter().filter_map(|r| r.err()) {
            let msg = e.to_string().to_lowercase();
            if msg.contains("nonce too low") || msg.contains("already known") {
                // Another endpoint got it there first (seen live with the sequencer): benign.
                tracing::debug!(error = %e, "broadcast endpoint already had the tx");
            } else {
                warn!(error = %e, "a broadcast endpoint rejected the tx (others accepted)");
            }
        }
        Ok(Sent {
            hash,
            signed,
            first_accept,
        })
    }

    pub async fn wait(&self, hash: TxHash, timeout: Duration) -> Result<Landed> {
        let start = std::time::Instant::now();
        loop {
            if let Some(r) = self.reader.get_transaction_receipt(hash).await? {
                return Ok(Landed {
                    success: r.status(),
                    gas_used: r.gas_used,
                    effective_gas_price: r.effective_gas_price,
                    block: r.block_number,
                    tx_index: r.transaction_index,
                });
            }
            if start.elapsed() > timeout {
                // Resync nonce in case the tx was dropped.
                let n = self
                    .reader
                    .get_transaction_count(self.address)
                    .pending()
                    .await?;
                self.nonce.store(n, Ordering::SeqCst);
                bail!("receipt timeout for {hash}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// Node rejections meaning our nonce is out of sync with the chain.
pub fn is_nonce_error(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    s.contains("nonce too low")
        || s.contains("nonce too high")
        || s.contains("invalid nonce")
        || s.contains("already known")
}

/// Executor calldata for an arbitrage (pure; unit-tested against the ABI).
pub fn execute_calldata(opp: &Opportunity, min_profit: U256) -> Vec<u8> {
    ArbExecutor::executeCall {
        token: opp.loan_token,
        amount: opp.amount_in,
        minProfit: min_profit,
        hops: opp.hops.iter().map(to_sol_hop).collect(),
    }
    .abi_encode()
}

/// Executor calldata for a liquidation (pure; unit-tested against the ABI).
pub fn liquidate_calldata(plan: &Plan, hops: &[Hop], min_profit: U256) -> Vec<u8> {
    ArbExecutor::liquidateCall {
        debt: plan.debt_asset,
        collateral: plan.collateral_asset,
        flashAmount: plan.flash_amount,
        minProfit: min_profit,
        calls: plan.calls.iter().map(to_sol_call).collect(),
        hops: hops.iter().map(to_sol_hop).collect(),
    }
    .abi_encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_lending::{AccountRef, ProtocolCall};

    fn hop() -> Hop {
        Hop {
            kind: 2,
            pool: Address::with_last_byte(1),
            token_in: Address::with_last_byte(2),
            token_out: Address::with_last_byte(3),
            zero_for_one: false,
            fee_bps: 0,
        }
    }

    #[test]
    fn recognizes_nonce_errors_from_nodes() {
        let low = anyhow::anyhow!(
            "all broadcasts failed: ErrorResp {{ code: -32003, message: \"nonce too low\" }}"
        );
        assert!(is_nonce_error(&low));
        assert!(is_nonce_error(&anyhow::anyhow!("Invalid nonce")));
        assert!(!is_nonce_error(&anyhow::anyhow!(
            "insufficient funds for gas"
        )));
    }

    #[test]
    fn execute_calldata_decodes_to_the_opportunity() {
        let opp = Opportunity {
            loan_token: Address::with_last_byte(2),
            amount_in: U256::from(123),
            hops: vec![hop()],
            est_profit: 1.0,
            est_profit_usd: 1.0,
            buy_pool: Address::ZERO,
            sell_pool: Address::ZERO,
        };
        let d =
            ArbExecutor::executeCall::abi_decode(&execute_calldata(&opp, U256::from(9))).unwrap();
        assert_eq!(
            (d.token, d.amount, d.minProfit, d.hops.len()),
            (opp.loan_token, opp.amount_in, U256::from(9), 1)
        );
        assert_eq!(d.hops[0].pool, Address::with_last_byte(1));
    }

    #[test]
    fn liquidate_calldata_decodes_to_the_plan() {
        let plan = Plan {
            protocol: "test",
            account: AccountRef {
                market: Address::ZERO,
                account: Address::ZERO,
                sub_id: 0,
            },
            debt_asset: Address::with_last_byte(5),
            collateral_asset: Address::with_last_byte(6),
            flash_amount: U256::from(1_000),
            expected_collateral: 1.0,
            bonus_usd: 1.0,
            calls: vec![ProtocolCall {
                target: Address::with_last_byte(7),
                data: vec![0xaa, 0xbb].into(),
                approve_spender: Address::with_last_byte(7),
                approve_amount: U256::from(1_000),
            }],
        };
        let d = ArbExecutor::liquidateCall::abi_decode(&liquidate_calldata(
            &plan,
            &[hop()],
            U256::from(3),
        ))
        .unwrap();
        assert_eq!(
            (d.debt, d.collateral, d.flashAmount, d.minProfit),
            (
                plan.debt_asset,
                plan.collateral_asset,
                plan.flash_amount,
                U256::from(3)
            )
        );
        assert_eq!(d.calls[0].target, Address::with_last_byte(7));
        assert_eq!(d.calls[0].data.to_vec(), vec![0xaa, 0xbb]);
        assert_eq!(d.hops.len(), 1);
    }
}
