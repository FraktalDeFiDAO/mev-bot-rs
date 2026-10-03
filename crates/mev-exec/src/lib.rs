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
use mev_bindings::{ArbExecutor, to_sol_hop};
use mev_types::{LiquidationCall, Opportunity};
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

    /// Send an arbitrage through the executor.
    pub async fn send(
        &self,
        opp: &Opportunity,
        min_profit: U256,
        gas_limit: u64,
    ) -> Result<TxHash> {
        let calldata = ArbExecutor::executeCall {
            token: opp.loan_token,
            amount: opp.amount_in,
            minProfit: min_profit,
            hops: opp.hops.iter().map(to_sol_hop).collect(),
        }
        .abi_encode();
        self.send_calldata(calldata, gas_limit).await
    }

    /// Send an Aave liquidation through the executor.
    pub async fn send_liquidation(
        &self,
        c: &LiquidationCall,
        min_profit: U256,
        gas_limit: u64,
    ) -> Result<TxHash> {
        let calldata = ArbExecutor::liquidateCall {
            collateral: c.collateral,
            debt: c.debt,
            user: c.user,
            flashAmount: c.flash_amount,
            minProfit: min_profit,
            hops: c.hops.iter().map(to_sol_hop).collect(),
        }
        .abi_encode();
        self.send_calldata(calldata, gas_limit).await
    }

    /// Sign once and broadcast the same raw tx to every endpoint; first success wins.
    async fn send_calldata(&self, calldata: Vec<u8>, gas_limit: u64) -> Result<TxHash> {
        let base_fee = self.reader.get_gas_price().await?;
        let nonce = self.nonce.fetch_add(1, Ordering::SeqCst);
        let tx = TransactionRequest::default()
            .with_from(self.address)
            .with_to(self.executor)
            .with_input(calldata)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(gas_limit)
            .with_max_fee_per_gas(base_fee * 2)
            .with_max_priority_fee_per_gas(0);
        let envelope = tx.build(&self.wallet).await.context("signing failed")?;
        let raw = envelope.encoded_2718();
        let hash = *envelope.tx_hash();

        let sends = self.broadcast.iter().map(|p| {
            let raw = raw.clone();
            async move { p.send_raw_transaction(&raw).await.map(|_| ()) }
        });
        let results = futures_join_all(sends).await;
        if results.iter().all(|r| r.is_err()) {
            // Nothing accepted it — reuse the nonce.
            self.nonce.fetch_sub(1, Ordering::SeqCst);
            bail!(
                "all broadcasts failed: {:?}",
                results.into_iter().filter_map(|r| r.err()).next()
            );
        }
        for e in results.into_iter().filter_map(|r| r.err()) {
            warn!(error = %e, "a broadcast endpoint rejected the tx (others accepted)");
        }
        Ok(hash)
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

async fn futures_join_all<F: std::future::Future>(
    futs: impl IntoIterator<Item = F>,
) -> Vec<F::Output> {
    let handles: Vec<_> = futs.into_iter().collect();
    let mut out = Vec::with_capacity(handles.len());
    for f in handles {
        out.push(f.await);
    }
    out
}
