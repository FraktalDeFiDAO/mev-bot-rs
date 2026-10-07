//! Transaction building, signing, broadcast (to several endpoints at once), and receipt tracking.

use std::time::Duration;

use alloy::{
    eips::eip2718::Encodable2718,
    network::{EthereumWallet, NetworkTransactionBuilder, TransactionBuilder},
    primitives::{Address, TxHash, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result, bail};
use mev_lending::Plan;
use mev_types::{Hop, Opportunity};
use tracing::warn;

mod nonce;

pub struct Sender {
    wallet: EthereumWallet,
    pub address: Address,
    reader: DynProvider,
    /// (label, endpoint): the reader is "rpc"; extras are labelled by host.
    broadcast: Vec<(String, DynProvider)>,
    executor: Address,
    chain_id: u64,
    nonce: nonce::NonceManager,
    /// The deployed executor has `executeFlashSwap` (V3-first routes then need no lender).
    pub flash_swaps: bool,
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub success: bool,
    pub gas_used: u64,
    pub effective_gas_price: u128,
    pub block: Option<u64>,
    /// Position in the block (race telemetry: who was ahead of us).
    pub tx_index: Option<u64>,
    /// For a `batch` transaction: whether each item executed (empty otherwise).
    pub batch_ok: Vec<bool>,
}

/// A broadcast transaction and its timing.
#[derive(Debug, Clone)]
pub struct Sent {
    pub hash: TxHash,
    /// When it was signed (just before broadcasting).
    pub signed: std::time::Instant,
    /// When the first endpoint accepted it (the sequencer usually; the time that matters).
    pub first_accept: Option<std::time::Instant>,
    /// Per endpoint: (label, ms from signing to its answer, accepted).
    pub endpoints: Vec<(String, u64, bool)>,
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
        let mut broadcast = vec![("rpc".to_string(), reader.clone())];
        for url in extra_broadcast {
            let parsed: alloy::transports::http::reqwest::Url = url.parse()?;
            let label = parsed.host_str().unwrap_or("endpoint").to_string();
            broadcast.push((label, ProviderBuilder::new().connect_http(parsed).erased()));
        }
        let nonce = reader.get_transaction_count(address).pending().await?;
        Ok(Self {
            wallet: EthereumWallet::from(signer),
            address,
            reader,
            broadcast,
            executor,
            chain_id,
            nonce: nonce::NonceManager::new(nonce),
            flash_swaps: false,
        })
    }

    pub async fn eth_balance(&self) -> Result<U256> {
        Ok(self.reader.get_balance(self.address).await?)
    }

    /// Send an arbitrage through the executor with `fees` (the PGA bid decided at submission
    /// time); `None` fetches the base fee and bids nothing.
    pub async fn send(
        &self,
        opp: &Opportunity,
        min_profit: U256,
        gas_limit: u64,
        fees: Option<Fees>,
    ) -> Result<Sent> {
        let calldata = mev_bindings::arb_calldata(opp, min_profit, self.flash_swaps);
        self.send_calldata(calldata, gas_limit, fees).await
    }

    /// Send a liquidation (any protocol) through the executor.
    pub async fn send_liquidation(
        &self,
        plan: &Plan,
        hops: &[Hop],
        min_profit: U256,
        gas_limit: u64,
        fees: Option<Fees>,
        aave_pool: Option<Address>,
    ) -> Result<TxHash> {
        let data = mev_bindings::liquidation_calldata(plan, hops, min_profit, aave_pool);
        self.send_calldata(data, gas_limit, fees)
            .await
            .map(|s| s.hash)
    }

    /// Latency-critical liquidation (backrun): the caller's `fees` (from a cached base fee and
    /// the bid policy), no fetch first.
    pub async fn send_liquidation_fast(
        &self,
        plan: &Plan,
        hops: &[Hop],
        min_profit: U256,
        gas_limit: u64,
        fees: Fees,
        aave_pool: Option<Address>,
    ) -> Result<Sent> {
        self.send_calldata(
            mev_bindings::liquidation_calldata(plan, hops, min_profit, aave_pool),
            gas_limit,
            Some(fees),
        )
        .await
    }

    /// Send ready-made executor calldata (a single trade built by the caller).
    pub async fn send_prebuilt(
        &self,
        calldata: Vec<u8>,
        gas_limit: u64,
        fees: Fees,
    ) -> Result<Sent> {
        self.send_calldata(calldata, gas_limit, Some(fees)).await
    }

    /// Several trades in one transaction through the executor's `batch` (each item is ready-made
    /// calldata; each succeeds or fails on its own). Needs an executor that has `batch`.
    pub async fn send_batch(
        &self,
        items: Vec<Vec<u8>>,
        gas_limit: u64,
        fees: Fees,
    ) -> Result<Sent> {
        anyhow::ensure!(
            !items.is_empty() && items.len() <= mev_bindings::MAX_BATCH,
            "a batch carries 1..={} trades",
            mev_bindings::MAX_BATCH
        );
        self.send_calldata(mev_bindings::batch_calldata(items), gas_limit, Some(fees))
            .await
    }

    /// Sign once and broadcast the same raw tx to every endpoint; first success wins. `fees`: the
    /// bid decided at submission time; `None` fetches the base fee and bids nothing.
    async fn send_calldata(
        &self,
        calldata: Vec<u8>,
        gas_limit: u64,
        fees: Option<Fees>,
    ) -> Result<Sent> {
        let fees = match fees {
            Some(f) => f,
            None => Fees::no_bid(self.reader.get_gas_price().await?),
        };
        match self.sign_and_broadcast(&calldata, gas_limit, fees).await {
            // The wallet was used outside the bot (or a tx was dropped): the cached nonce is stale.
            // Resync from the chain and re-sign once.
            Err(e) if is_nonce_error(&e) => {
                let n = self
                    .reader
                    .get_transaction_count(self.address)
                    .pending()
                    .await?;
                warn!(resynced_nonce = n, error = %e, "stale nonce; resyncing and retrying once");
                self.nonce.reset(n);
                self.sign_and_broadcast(&calldata, gas_limit, fees).await
            }
            other => other,
        }
    }

    async fn sign_and_broadcast(
        &self,
        calldata: &[u8],
        gas_limit: u64,
        fees: Fees,
    ) -> Result<Sent> {
        let nonce = self.nonce.take();
        let tx = tx_request(
            self.address,
            self.executor,
            calldata,
            nonce,
            self.chain_id,
            gas_limit,
            fees,
        );
        let envelope = tx.build(&self.wallet).await.context("signing failed")?;
        let raw = envelope.encoded_2718();
        let hash = *envelope.tx_hash();
        let signed = std::time::Instant::now();

        let sends = self.broadcast.iter().map(|(_, p)| {
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
        let endpoints: Vec<(String, u64, bool)> = self
            .broadcast
            .iter()
            .zip(&timed)
            .map(|((label, _), (r, t))| {
                // "already known"/"nonce too low" from a later endpoint means an earlier one won.
                let ok = r.is_ok() || r.as_ref().is_err_and(|e| is_already_known(&e.to_string()));
                (
                    label.clone(),
                    t.duration_since(signed).as_millis() as u64,
                    ok,
                )
            })
            .collect();
        let results: Vec<_> = timed.into_iter().map(|(r, _)| r).collect();
        if results.iter().all(|r| r.is_err()) {
            // Nothing accepted it: release the nonce (refilled before any new one, so a concurrent
            // send's nonce is never reused and no gap stalls the ones queued behind it).
            self.nonce.release(nonce);
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
            endpoints,
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
                    batch_ok: mev_bindings::batch_outcomes(r.inner.logs(), self.executor),
                });
            }
            if start.elapsed() > timeout {
                // Resync nonce in case the tx was dropped.
                let n = self
                    .reader
                    .get_transaction_count(self.address)
                    .pending()
                    .await?;
                self.nonce.reset(n);
                bail!("receipt timeout for {hash}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

fn is_already_known(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("already known") || m.contains("nonce too low")
}

/// Node rejections meaning our nonce is out of sync with the chain.
pub fn is_nonce_error(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    s.contains("nonce too low")
        || s.contains("nonce too high")
        || s.contains("invalid nonce")
        || s.contains("already known")
}

/// EIP-1559 fee fields for one transaction. Under Arbitrum's PGA the Sequencer orders each round
/// by `min(max_priority_fee_per_gas, max_fee_per_gas - base_fee)`, so both are always set
/// explicitly and `max_fee_per_gas` always leaves room for the whole tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fees {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

impl Fees {
    /// No priority bid: lowest priority in its round (the anti-starvation boost still includes
    /// it within a few blocks).
    pub fn no_bid(base_fee: u128) -> Self {
        Self {
            max_fee_per_gas: base_fee.saturating_mul(2),
            max_priority_fee_per_gas: 0,
        }
    }

    /// The fee fields of a policy bid.
    pub fn from_bid(b: &mev_risk::bid::Bid) -> Self {
        Self {
            max_fee_per_gas: b.max_fee_per_gas,
            max_priority_fee_per_gas: b.max_priority_fee_per_gas,
        }
    }
}

/// The unsigned EIP-1559 transaction for `calldata` to the executor (pure, unit-tested).
pub fn tx_request(
    from: Address,
    to: Address,
    calldata: &[u8],
    nonce: u64,
    chain_id: u64,
    gas_limit: u64,
    fees: Fees,
) -> TransactionRequest {
    TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_input(calldata.to_vec())
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(gas_limit)
        .with_max_fee_per_gas(fees.max_fee_per_gas.max(fees.max_priority_fee_per_gas))
        .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
}

/// Executor calldata for a Balancer-funded arbitrage (pure; unit-tested against the ABI).
pub fn execute_calldata(opp: &Opportunity, min_profit: U256) -> Vec<u8> {
    mev_bindings::arb_calldata(opp, min_profit, false)
}

/// Executor calldata for a Balancer-funded liquidation (pure; unit-tested against the ABI).
pub fn liquidate_calldata(plan: &Plan, hops: &[Hop], min_profit: U256) -> Vec<u8> {
    mev_bindings::liquidation_calldata(plan, hops, min_profit, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;
    use mev_bindings::ArbExecutor;
    use mev_lending::{AccountRef, ProtocolCall};

    fn hop() -> Hop {
        Hop {
            kind: 2,
            pool: Address::with_last_byte(1),
            token_in: Address::with_last_byte(2),
            token_out: Address::with_last_byte(3),
            zero_for_one: false,
            fee_bps: 0,
            spacing: 0,
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
            exact: true,
            versions: vec![],
        };
        let d =
            ArbExecutor::executeCall::abi_decode(&execute_calldata(&opp, U256::from(9))).unwrap();
        assert_eq!(
            (d.token, d.amount, d.minProfit, d.hops.len()),
            (opp.loan_token, opp.amount_in, U256::from(9), 1)
        );
        assert_eq!(d.hops[0].pool, Address::with_last_byte(1));
    }

    /// Loan token 2: V3 hop 2->3 on pool 1, then a hop 3->2 of `kind` on `pool`.
    fn round_trip(kind: u8, pool: u8) -> Opportunity {
        let back = Hop {
            kind,
            pool: Address::with_last_byte(pool),
            token_in: Address::with_last_byte(3),
            token_out: Address::with_last_byte(2),
            zero_for_one: true,
            fee_bps: 30,
            spacing: 0,
        };
        Opportunity {
            loan_token: Address::with_last_byte(2),
            amount_in: U256::from(5),
            hops: vec![hop(), back],
            est_profit: 1.0,
            est_profit_usd: 1.0,
            buy_pool: Address::ZERO,
            sell_pool: Address::ZERO,
            exact: true,
            versions: vec![],
        }
    }

    #[test]
    fn flash_swaps_only_for_v3_first_routes_when_enabled() {
        use mev_bindings::{arb_calldata, flash_swappable};
        let v3_first = round_trip(0, 4);
        assert!(flash_swappable(&v3_first));
        let d = ArbExecutor::executeFlashSwapCall::abi_decode(&arb_calldata(
            &v3_first,
            U256::from(7),
            true,
        ))
        .unwrap();
        assert_eq!(
            (d.amount, d.minProfit, d.hops.len()),
            (U256::from(5), U256::from(7), 2)
        );
        // Disabled (executor without executeFlashSwap): Balancer.
        assert!(
            ArbExecutor::executeCall::abi_decode(&arb_calldata(&v3_first, U256::ZERO, false))
                .is_ok()
        );
        // V2 first hop, or the first pool reused (locked in its own callback): Balancer.
        let mut v2_first = v3_first.clone();
        v2_first.hops.swap(0, 1);
        v2_first.loan_token = Address::with_last_byte(3);
        assert!(!flash_swappable(&v2_first));
        assert!(!flash_swappable(&round_trip(2, 1)));
        assert!(
            ArbExecutor::executeCall::abi_decode(&arb_calldata(
                &round_trip(2, 1),
                U256::ZERO,
                true
            ))
            .is_ok()
        );
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

        // Aave-funded: same plan through liquidateViaAave, naming the pool.
        let pool = Address::with_last_byte(9);
        let a = ArbExecutor::liquidateViaAaveCall::abi_decode(&mev_bindings::liquidation_calldata(
            &plan,
            &[hop()],
            U256::from(3),
            Some(pool),
        ))
        .unwrap();
        assert_eq!(
            (
                a.aavePool,
                a.debt,
                a.flashAmount,
                a.minProfit,
                a.calls.len(),
                a.hops.len()
            ),
            (
                pool,
                plan.debt_asset,
                plan.flash_amount,
                U256::from(3),
                1,
                1
            )
        );
    }

    #[test]
    fn every_tx_sets_both_fee_fields_and_max_fee_covers_the_tip() {
        let (from, to) = (Address::with_last_byte(1), Address::with_last_byte(2));
        let base = 10_000_000u128; // 0.01 gwei
        // A policy bid: fields carried through unchanged.
        let bid = Fees {
            max_fee_per_gas: 2 * base + 5_000_000,
            max_priority_fee_per_gas: 5_000_000,
        };
        let t = tx_request(from, to, &[0xab], 7, 42161, 900_000, bid);
        assert_eq!(t.max_priority_fee_per_gas, Some(5_000_000));
        assert_eq!(t.max_fee_per_gas, Some(2 * base + 5_000_000));
        assert_eq!(
            (t.nonce, t.gas, t.chain_id),
            (Some(7), Some(900_000), Some(42161))
        );
        assert_eq!(t.input.input().map(|b| b.to_vec()), Some(vec![0xab]));
        // No bid: explicit zero tip, 2x base fee.
        let n = tx_request(from, to, &[], 0, 42161, 1, Fees::no_bid(base));
        assert_eq!(
            (n.max_priority_fee_per_gas, n.max_fee_per_gas),
            (Some(0), Some(2 * base))
        );
        // A malformed bid can't make maxFee < tip (the node would reject it).
        let bad = Fees {
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 9,
        };
        let b = tx_request(from, to, &[], 0, 42161, 1, bad);
        assert!(b.max_fee_per_gas.unwrap() >= b.max_priority_fee_per_gas.unwrap());
        assert_eq!((t.from, t.to), (Some(from), Some(to.into())));
    }
}
