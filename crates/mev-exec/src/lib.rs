//! Transaction building, signing, broadcast (to several endpoints at once), and receipt tracking.

use futures::StreamExt;
use std::{collections::HashMap, sync::Arc, time::Duration};

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

#[derive(Clone)]
pub struct Sender {
    wallet: EthereumWallet,
    pub address: Address,
    reader: DynProvider,
    /// (label, endpoint): the reader is "rpc"; extras are labelled by host.
    broadcast: Vec<(String, DynProvider)>,
    executor: Address,
    chain_id: u64,
    nonce: Arc<nonce::NonceManager>,
    startup_nonce: u64,
    reservation: Option<mev_risk::ledger::Reservation>,
    submission_guard: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    broadcast_enabled: bool,
    signed_nonces: Arc<std::sync::Mutex<HashMap<TxHash, u64>>>,
    /// The deployed executor has `executeFlashSwap` (V3-first routes then need no lender).
    pub flash_swaps: bool,
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub profits: Vec<(Address, U256)>,
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
            nonce: Arc::new(nonce::NonceManager::new(nonce)),
            startup_nonce: nonce,
            reservation: None,
            submission_guard: None,
            broadcast_enabled: true,
            signed_nonces: Default::default(),
            flash_swaps: false,
        })
    }

    /// A disposable signer with no broadcast endpoints and no production nonce reads.
    pub fn shadow(reader: DynProvider, executor: Address, chain_id: u64) -> Self {
        let signer = PrivateKeySigner::random();
        Self {
            address: signer.address(),
            wallet: EthereumWallet::from(signer),
            reader,
            broadcast: vec![],
            executor,
            chain_id,
            nonce: Arc::new(nonce::NonceManager::new(0)),
            startup_nonce: 0,
            reservation: None,
            submission_guard: None,
            broadcast_enabled: false,
            signed_nonces: Default::default(),
            flash_swaps: false,
        }
    }

    /// Build and sign exactly the candidate transaction; signed bytes never enter a transport.
    pub async fn sign_shadow(&self, calldata: &[u8], gas_limit: u64, fees: Fees) -> Result<TxHash> {
        anyhow::ensure!(
            !self.broadcast_enabled,
            "shadow signing requires a disposable signer"
        );
        let tx = tx_request(
            self.address,
            self.executor,
            calldata,
            0,
            self.chain_id,
            gas_limit,
            fees,
        );
        let envelope = tx.build(&self.wallet).await?;
        Ok(*envelope.tx_hash())
    }

    /// Bind an already-admitted liability to this one signed transaction.
    pub fn reserved(&self, reservation: mev_risk::ledger::Reservation) -> Self {
        let mut sender = self.clone();
        sender.reservation = Some(reservation);
        sender
    }

    pub fn guarded(mut self, guard: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.submission_guard = Some(guard);
        self
    }

    fn submission_allowed(&self) -> bool {
        self.reservation.as_ref().is_some_and(|r| r.active())
            && self.submission_guard.as_ref().is_none_or(|guard| guard())
    }

    pub fn restore_pending(&self, hash: TxHash, nonce: u64) {
        self.nonce.hold(nonce);
        self.signed_nonces.lock().unwrap().insert(hash, nonce);
    }

    pub fn reconcile_nonce(&self, settled_floor: u64, known_upper: u64) -> Result<()> {
        anyhow::ensure!(
            self.startup_nonce <= known_upper,
            "wallet nonce exceeds reconciled ledger; prepare fresh receipt accounting"
        );
        self.nonce.reset(settled_floor);
        Ok(())
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
        let data = liquidation_final_calldata(plan, hops, min_profit, aave_pool).bytes;
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
            liquidation_final_calldata(plan, hops, min_profit, aave_pool).bytes,
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
        anyhow::ensure!(self.broadcast_enabled, "shadow signer cannot broadcast");
        anyhow::ensure!(
            self.reservation.is_some() && self.submission_guard.is_some(),
            "live signing requires admission and a final state guard"
        );
        let fees = match fees {
            Some(f) => f,
            None => Fees::no_bid(self.reader.get_gas_price().await?),
        };
        // Acceptance is ambiguous after a transport error. Never construct a second execution.
        self.sign_and_broadcast(&calldata, gas_limit, fees).await
    }

    async fn sign_and_broadcast(
        &self,
        calldata: &[u8],
        gas_limit: u64,
        fees: Fees,
    ) -> Result<Sent> {
        anyhow::ensure!(
            fees.max_priority_fee_per_gas <= fees.max_fee_per_gas,
            "priority fee exceeds max fee"
        );
        self.reservation
            .as_ref()
            .context("transaction admission missing")?
            .verify_terms(
                gas_limit,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
            )?;
        anyhow::ensure!(
            self.submission_allowed(),
            "submission invalidated before signing"
        );
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
        let envelope = match tx.build(&self.wallet).await {
            Ok(e) => e,
            Err(e) => {
                self.nonce.release(nonce);
                return Err(anyhow::anyhow!("signing failed: {e}"));
            }
        };
        let raw = envelope.encoded_2718();
        let hash = *envelope.tx_hash();
        let signed = std::time::Instant::now();

        if !self.submission_allowed() {
            self.nonce.release(nonce);
            bail!("submission invalidated before broadcast");
        }

        if let Some(reservation) = &self.reservation
            && let Err(e) = reservation.attach(hash.to_string(), nonce, self.address)
        {
            self.nonce.release(nonce);
            return Err(e);
        }
        if !self.submission_allowed() {
            if let Some(reservation) = &self.reservation {
                reservation.cancel_before_broadcast(&hash.to_string())?;
            }
            self.nonce.release(nonce);
            bail!("submission invalidated after persistence; no broadcast attempted");
        }
        self.signed_nonces.lock().unwrap().insert(hash, nonce);
        let mut sends = futures::stream::FuturesUnordered::new();
        for (label, provider) in &self.broadcast {
            let (label, p, raw) = (label.clone(), provider.clone(), raw.clone());
            sends.push(async move {
                let result =
                    tokio::time::timeout(Duration::from_secs(3), p.send_raw_transaction(&raw))
                        .await;
                let accepted = match result {
                    Ok(Ok(_)) => true,
                    Ok(Err(e)) => is_already_known(&e.to_string()),
                    Err(_) => false,
                };
                (label, accepted, std::time::Instant::now())
            });
        }
        let mut endpoints = Vec::new();
        let mut first_accept = None;
        while let Some((label, accepted, at)) = sends.next().await {
            endpoints.push((
                label,
                at.duration_since(signed).as_millis() as u64,
                accepted,
            ));
            if accepted {
                first_accept = Some(at);
                break;
            }
        }
        if first_accept.is_some() {
            // Tail telemetry must not delay receipt tracking or another admitted send.
            tokio::spawn(async move {
                while let Some((label, accepted, _)) = sends.next().await {
                    tracing::debug!(endpoint = label, accepted, "remaining broadcast reply");
                }
            });
        } else {
            warn!(%hash, nonce, "broadcast outcome unknown; retaining nonce and reserved liability");
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
            if let Ok(Ok(Some(r))) = tokio::time::timeout(
                Duration::from_secs(3),
                self.reader.get_transaction_receipt(hash),
            )
            .await
            {
                anyhow::ensure!(
                    r.transaction_hash == hash
                        && r.from == self.address
                        && r.block_hash.is_some()
                        && r.block_number.is_some(),
                    "receipt identity or block missing; liability retained"
                );
                if let Some(nonce) = self.signed_nonces.lock().unwrap().remove(&hash) {
                    self.nonce.reset(nonce.saturating_add(1));
                }
                return Ok(Landed {
                    profits: mev_bindings::profits(r.inner.logs(), self.executor),
                    success: r.status(),
                    gas_used: r.gas_used,
                    effective_gas_price: r.effective_gas_price,
                    block: r.block_number,
                    tx_index: r.transaction_index,
                    batch_ok: mev_bindings::batch_outcomes(r.inner.logs(), self.executor),
                });
            }
            if start.elapsed() > timeout {
                // A missing receipt is still an unresolved transaction, never a free nonce.
                bail!("receipt timeout for {hash}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    /// Reconciliation continues through transient provider outages. Liability remains reserved.
    pub async fn wait_final(&self, hash: TxHash) -> Landed {
        loop {
            match self.wait(hash, Duration::from_secs(30)).await {
                Ok(receipt) => return receipt,
                Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
            }
        }
    }
}

fn is_already_known(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("already known")
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

/// The calldata `Sender::send_liquidation` / `send_liquidation_fast` sign for these inputs, with
/// its hash. Both send paths build through this function, so a simulation of `.bytes` (or a
/// matching `.hash`) is a simulation of exactly what would be signed.
pub fn liquidation_final_calldata(
    plan: &Plan,
    hops: &[Hop],
    min_profit: U256,
    aave_pool: Option<Address>,
) -> mev_bindings::FinalCalldata {
    mev_bindings::final_liquidation_calldata(plan, hops, min_profit, aave_pool)
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

    fn mock_risk() -> mev_risk::RiskGuard {
        mev_risk::RiskGuard::new(
            mev_risk::RiskLimits {
                min_profit_usd: 0.1,
                max_gas_usd_per_day: 1.0,
                max_arb_gas_usd_per_day: 1.0,
                max_consecutive_failures: 10,
            },
            std::env::temp_dir().join("unused-exec-test-stop"),
        )
    }

    async fn mocked_sender() -> (Sender, alloy::transports::mock::Asserter) {
        let mock = alloy::transports::mock::Asserter::new();
        mock.push_success(&"0x7");
        let provider = ProviderBuilder::new()
            .connect_mocked_client(mock.clone())
            .erased();
        let key = PrivateKeySigner::random().to_bytes().to_string();
        let sender = Sender::new(&key, provider, &[], Address::with_last_byte(1), 42161)
            .await
            .unwrap();
        let mut risk = mock_risk();
        let reservation = risk.reserve(1.0, 0.00001, true, 100).unwrap();
        reservation.set_native_price(1_000_000).unwrap();
        let sender = sender.reserved(reservation).guarded(Arc::new(|| true));
        (sender, mock)
    }

    #[tokio::test]
    async fn shadow_builds_and_signs_without_any_nonce_or_broadcast_rpc() {
        let mock = alloy::transports::mock::Asserter::new();
        mock.push_failure_msg("shadow must never call RPC");
        let provider = ProviderBuilder::new()
            .connect_mocked_client(mock.clone())
            .erased();
        let sender = Sender::shadow(provider, Address::with_last_byte(1), 42161);
        let hash = sender
            .sign_shadow(&[1, 2, 3], 100_000, Fees::no_bid(10))
            .await
            .unwrap();
        assert_ne!(hash, TxHash::ZERO);
        assert!(
            sender
                .send_prebuilt(vec![], 100_000, Fees::no_bid(10))
                .await
                .is_err()
        );
        assert_eq!(mock.read_q().len(), 1);
    }

    #[tokio::test]
    async fn already_known_is_one_accepted_execution_with_the_original_nonce() {
        let (sender, mock) = mocked_sender().await;
        mock.push_failure_msg("already known");
        mock.push_success(&"must not consume another RPC response");
        let sent = sender
            .send_prebuilt(vec![], 100_000, Fees::no_bid(10))
            .await
            .unwrap();
        assert!(sent.first_accept.is_some());
        assert_eq!(mock.read_q().len(), 1, "no nonce refresh or re-sign");
        assert_eq!(sender.nonce.take(), 8);
    }

    #[tokio::test]
    async fn ambiguous_broadcast_and_receipt_errors_keep_the_nonce_owned() {
        let (sender, mock) = mocked_sender().await;
        mock.push_failure_msg("transport timeout");
        let sent = sender
            .send_prebuilt(vec![], 100_000, Fees::no_bid(10))
            .await
            .unwrap();
        assert!(sent.first_accept.is_none());
        mock.push_failure_msg("temporary receipt service unavailable");
        assert!(sender.wait(sent.hash, Duration::ZERO).await.is_err());
        assert_eq!(sender.nonce.take(), 8, "nonce 7 remains owned");
        assert_eq!(
            sender.signed_nonces.lock().unwrap().get(&sent.hash),
            Some(&7)
        );
    }

    #[tokio::test]
    async fn restart_rejects_unaccounted_wallet_nonces_and_keeps_a_settled_floor() {
        let (sender, _) = mocked_sender().await;
        assert!(
            sender.reconcile_nonce(6, 6).is_err(),
            "startup nonce 7 is absent from the ledger"
        );
        assert!(
            sender.reconcile_nonce(9, 9).is_ok(),
            "lagging RPC cannot lower a proven settled nonce"
        );
        assert_eq!(sender.nonce.take(), 9);
    }

    #[tokio::test]
    async fn invalidation_after_durable_attach_cancels_without_broadcasting() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (sender, mock) = mocked_sender().await;
        mock.push_failure_msg("must never broadcast");
        let mut risk = mock_risk();
        let reservation = risk.reserve(1.0, 0.00001, true, 100).unwrap();
        reservation.set_native_price(1_000_000).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let check = calls.clone();
        let sender = sender
            .reserved(reservation)
            .guarded(Arc::new(move || check.fetch_add(1, Ordering::Relaxed) < 2));
        assert!(
            sender
                .send_prebuilt(vec![], 100_000, Fees::no_bid(10))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert_eq!(mock.read_q().len(), 1);
        assert!(risk.unresolved().is_empty());
        assert_eq!(sender.nonce.take(), 7);
    }

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
            dependencies: vec![],
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

        // Parity (JEV-LIQ-015): the hash a simulation binds to is the hash of the bytes the
        // send paths sign, and a different min_profit (the old sim used 0) is a different hash.
        let fin = liquidation_final_calldata(&plan, &[hop()], U256::from(3), Some(pool));
        assert_eq!(
            fin.bytes,
            mev_bindings::liquidation_calldata(&plan, &[hop()], U256::from(3), Some(pool))
        );
        assert_eq!(fin.hash, alloy::primitives::keccak256(&fin.bytes));
        let zero = liquidation_final_calldata(&plan, &[hop()], U256::ZERO, Some(pool));
        assert_ne!(zero.hash, fin.hash);
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
