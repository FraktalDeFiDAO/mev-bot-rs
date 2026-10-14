//! `LiquidationSimulator` (JEV-LIQ-015): exact-calldata REVM simulation of one executor call on the
//! state of ONE pinned block, fetched lazily over EIP-1898 (block hash).
//!
//! Contract (spec §0 invariant 4, §3):
//! * the request carries the block pin `(number, hash)`, the exact calldata and its keccak hash; a
//!   request whose hash does not match its bytes, or whose expiry has passed, is refused;
//! * every state read is made at the pin's HASH; a provider that answers for any other block, or a
//!   pin that is unknown / not canonical any more, makes the whole result a refusal (never a
//!   result computed from mixed heights);
//! * the result's `input_state_id` is the block hash it was computed on; `check_fresh` refuses it
//!   once the chain has moved to another state (stale opportunity);
//! * the L1 data fee is NOT guessed: it is either `NotIncluded` or a separately sourced
//!   `NodeInterface` component (`gasEstimateL1Component`), never added into `gas_used`;
//! * integer math only.
//!
//! Known limits (compare against `eth_call` with `compare`): Arbitrum precompiles (0x64.. range,
//! ArbSys etc.) are not emulated; `BLOCKHASH` returns zero; `NUMBER` is the L2 block number.

use std::{
    collections::HashSet,
    fmt,
    future::{Future, IntoFuture},
};

use alloy::{
    eips::BlockId,
    primitives::{Address, B256, Bytes, U256, keccak256},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    sol_types::{Panic, Revert, SolError},
};
use futures::{StreamExt, TryStreamExt};
use revm::{
    Context, ExecuteEvm, MainBuilder, MainContext,
    context::{
        TxEnv,
        result::{ExecutionResult, Output},
    },
    primitives::TxKind,
};

use crate::{Key, SnapshotDb, StateCache};

/// `NodeInterface` virtual contract (only reachable through `eth_call`; docs/arbitrum-facts.md R5).
pub const NODE_INTERFACE: Address = Address::new([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc8,
]);

/// State reads run in rounds (run, collect misses, fetch, rerun); a call touching more distinct
/// round-trips than this is refused rather than looped forever.
const MAX_ROUNDS: usize = 64;
const FETCH_CONCURRENCY: usize = 16;

/// The block the simulation is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPin {
    pub number: u64,
    pub hash: B256,
}

#[derive(Debug, Clone)]
pub struct SimulationRequest {
    pub block_pin: BlockPin,
    /// The exact bytes that would be signed as the transaction input.
    pub calldata: Bytes,
    /// Must equal `keccak256(calldata)`.
    pub calldata_hash: B256,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub gas_limit: u64,
    /// Unix seconds after which the request must not be simulated.
    pub expiry: u64,
}

impl SimulationRequest {
    /// Build a request, computing the calldata hash from the bytes.
    pub fn new(
        block_pin: BlockPin,
        calldata: Vec<u8>,
        from: Address,
        to: Address,
        value: U256,
        gas_limit: u64,
        expiry: u64,
    ) -> Self {
        let calldata: Bytes = calldata.into();
        Self {
            block_pin,
            calldata_hash: keccak256(&calldata),
            calldata,
            from,
            to,
            value,
            gas_limit,
            expiry,
        }
    }

    pub fn validate(&self, now_unix: u64) -> Result<(), Refusal> {
        let actual = keccak256(&self.calldata);
        if actual != self.calldata_hash {
            return Err(Refusal::CalldataHashMismatch {
                declared: self.calldata_hash,
                actual,
            });
        }
        if now_unix > self.expiry {
            return Err(Refusal::Expired {
                expiry: self.expiry,
                now: now_unix,
            });
        }
        Ok(())
    }
}

/// Why a request produced no result at all. A revert is a RESULT, not a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    CalldataHashMismatch {
        declared: B256,
        actual: B256,
    },
    Expired {
        expiry: u64,
        now: u64,
    },
    /// The provider does not know the pinned hash.
    UnknownBlock(B256),
    /// The header found for the hash disagrees with the pin's number.
    PinNumberMismatch {
        pinned: u64,
        header: u64,
    },
    /// A read was answered for another block than the pin (mixed heights).
    MixedHeight {
        expected: B256,
        answered: B256,
    },
    /// The pinned number is no longer the pinned hash on the canonical chain.
    Reorged {
        pinned: B256,
        canonical: Option<B256>,
    },
    /// The result was computed on another state than the one being checked.
    StaleState {
        simulated: B256,
        current: B256,
    },
    /// The result does not belong to the request being checked.
    CalldataMismatch {
        request: B256,
        result: B256,
    },
    TooManyRounds,
    Provider(String),
    Evm(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Refusal {}

/// Decoded revert data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertReason {
    Empty,
    /// `Error(string)` (Aave reverts with numeric strings such as "45").
    Message(String),
    /// `Panic(uint256)`.
    Panic(U256),
    /// A custom error: only the selector is known.
    Custom([u8; 4]),
    /// Fewer than 4 bytes of data.
    Malformed,
}

pub fn decode_revert(data: &[u8]) -> RevertReason {
    if data.is_empty() {
        return RevertReason::Empty;
    }
    if data.len() < 4 {
        return RevertReason::Malformed;
    }
    if data.starts_with(&Revert::SELECTOR)
        && let Ok(r) = Revert::abi_decode(data)
    {
        return RevertReason::Message(r.reason);
    }
    if data.starts_with(&Panic::SELECTOR)
        && let Ok(p) = Panic::abi_decode(data)
    {
        return RevertReason::Panic(p.code);
    }
    RevertReason::Custom([data[0], data[1], data[2], data[3]])
}

/// Parameterless custom errors of the verified Aave V3.6 `L2PoolInstance`
/// (0xf05f..2c9a on Arbitrum, Sourcify full match, `Errors.sol`), the ones a liquidation can hit.
const AAVE_ERRORS: &[&str] = &[
    "HealthFactorNotBelowThreshold",
    "HealthFactorLowerThanLiquidationThreshold",
    "CollateralCannotBeLiquidated",
    "SpecifiedCurrencyNotBorrowedByUser",
    "NoDebtOfSelectedType",
    "MustNotLeaveDust",
    "SelfLiquidation",
    "LiquidationGraceSentinelCheckFailed",
    "ReserveInactive",
    "ReserveFrozen",
    "ReservePaused",
    "InvalidAmount",
    "UnderlyingBalanceZero",
    "UserCannotHaveDebt",
];

/// Name of an Aave V3.6 custom error from its 4-byte selector.
pub fn aave_error_name(selector: [u8; 4]) -> Option<&'static str> {
    AAVE_ERRORS.iter().copied().find(|n| {
        let h = keccak256(format!("{n}()"));
        h[..4] == selector
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Success { output: Bytes },
    Revert { data: Bytes, reason: RevertReason },
    Halt { reason: String },
}

/// The L1 data cost, kept apart from `gas_used` and never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L1DataFee {
    /// Nobody asked or no source was available: `gas_used` is L2 execution gas only.
    NotIncluded,
    /// `NodeInterface.gasEstimateL1Component(to, false, calldata)` called at the pinned block.
    /// `gas_estimate_for_l1` is in L2 gas units; `base_fee` the L2 base fee (wei);
    /// `l1_base_fee_estimate` the L1 price per byte-unit the node used (wei).
    NodeInterface {
        gas_estimate_for_l1: u64,
        base_fee: U256,
        l1_base_fee_estimate: U256,
        at: B256,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationResult {
    /// Hash of the block whose state the call ran on.
    pub input_state_id: B256,
    pub block_number: u64,
    pub calldata_hash: B256,
    pub status: Status,
    /// EVM execution gas (L2 only; see `l1_data_fee`).
    pub gas_used: u64,
    pub l1_data_fee: L1DataFee,
    /// The 32-byte word the executor returned on success (ArbExecutor entrypoints return the
    /// profit as `uint256`), `None` for any other output shape or a failure.
    pub executor_reported_profit: Option<U256>,
    /// Distinct accounts + slots fetched from the provider.
    pub state_reads: usize,
}

impl SimulationResult {
    pub fn success(&self) -> bool {
        matches!(self.status, Status::Success { .. })
    }

    /// The result answers exactly this request (same calldata).
    pub fn check_binds(&self, req: &SimulationRequest) -> Result<(), Refusal> {
        if self.calldata_hash != req.calldata_hash {
            return Err(Refusal::CalldataMismatch {
                request: req.calldata_hash,
                result: self.calldata_hash,
            });
        }
        if self.input_state_id != req.block_pin.hash {
            return Err(Refusal::StaleState {
                simulated: self.input_state_id,
                current: req.block_pin.hash,
            });
        }
        Ok(())
    }

    /// Refuse a result whose state is not the state the caller is about to act on.
    pub fn check_fresh(&self, current: &BlockPin) -> Result<(), Refusal> {
        if self.input_state_id != current.hash {
            return Err(Refusal::StaleState {
                simulated: self.input_state_id,
                current: current.hash,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HeaderInfo {
    pub hash: B256,
    pub number: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct AccountState {
    pub balance: U256,
    pub nonce: u64,
    pub code: Bytes,
}

/// A value together with the block hash the provider says it answered for.
#[derive(Debug, Clone)]
pub struct Answered<T> {
    pub value: T,
    pub block_hash: B256,
}

/// Where pinned state comes from. Implemented over RPC for production and by fixtures in tests.
pub trait StateSource: Send + Sync {
    fn header_by_hash(
        &self,
        hash: B256,
    ) -> impl Future<Output = anyhow::Result<Option<HeaderInfo>>> + Send;
    fn canonical_hash_at(
        &self,
        number: u64,
    ) -> impl Future<Output = anyhow::Result<Option<B256>>> + Send;
    fn account(
        &self,
        addr: Address,
        at: B256,
    ) -> impl Future<Output = anyhow::Result<Answered<AccountState>>> + Send;
    fn slot(
        &self,
        addr: Address,
        slot: U256,
        at: B256,
    ) -> impl Future<Output = anyhow::Result<Answered<U256>>> + Send;
    /// Separately sourced L1 data fee component; the default is the explicit "not included".
    fn l1_data_fee(
        &self,
        _req: &SimulationRequest,
    ) -> impl Future<Output = anyhow::Result<L1DataFee>> + Send {
        async { Ok(L1DataFee::NotIncluded) }
    }
}

/// Production source: any JSON-RPC provider, every read pinned with an EIP-1898 hash object.
#[derive(Clone)]
pub struct RpcStateSource {
    provider: DynProvider,
    /// Also ask `NodeInterface` for the L1 component of each request.
    pub with_l1_component: bool,
}

impl RpcStateSource {
    pub fn new(provider: DynProvider) -> Self {
        Self {
            provider,
            with_l1_component: false,
        }
    }

    pub fn provider(&self) -> &DynProvider {
        &self.provider
    }

    fn at(hash: B256) -> BlockId {
        BlockId::hash_canonical(hash)
    }

    /// `eth_call` of the exact request at the pinned hash (the comparison baseline).
    pub async fn eth_call(&self, req: &SimulationRequest) -> EthCallOutcome {
        let tx = TransactionRequest::default()
            .from(req.from)
            .to(req.to)
            .value(req.value)
            .gas_limit(req.gas_limit)
            .input(req.calldata.clone().into());
        match self
            .provider
            .call(tx.clone())
            .block(Self::at(req.block_pin.hash))
            .await
        {
            Ok(out) => {
                let gas = self
                    .provider
                    .estimate_gas(tx)
                    .block(Self::at(req.block_pin.hash))
                    .await
                    .ok();
                EthCallOutcome::Success { output: out, gas }
            }
            Err(e) => match e.as_error_resp().and_then(|r| r.as_revert_data()) {
                Some(data) => EthCallOutcome::Revert { data },
                None => EthCallOutcome::Error(e.to_string()),
            },
        }
    }
}

alloy::sol! {
    interface INodeInterface {
        function gasEstimateL1Component(address to, bool contractCreation, bytes data)
            external payable returns (uint64 gasEstimateForL1, uint256 baseFee, uint256 l1BaseFeeEstimate);
    }
}

impl StateSource for RpcStateSource {
    async fn header_by_hash(&self, hash: B256) -> anyhow::Result<Option<HeaderInfo>> {
        Ok(self
            .provider
            .get_block_by_hash(hash)
            .await?
            .map(|b| HeaderInfo {
                hash: b.header.hash,
                number: b.header.number,
                timestamp: b.header.timestamp,
            }))
    }

    async fn canonical_hash_at(&self, number: u64) -> anyhow::Result<Option<B256>> {
        Ok(self
            .provider
            .get_block_by_number(number.into())
            .await?
            .map(|b| b.header.hash))
    }

    async fn account(&self, addr: Address, at: B256) -> anyhow::Result<Answered<AccountState>> {
        let id = Self::at(at);
        let (balance, nonce, code) = futures::try_join!(
            self.provider.get_balance(addr).block_id(id).into_future(),
            self.provider
                .get_transaction_count(addr)
                .block_id(id)
                .into_future(),
            self.provider.get_code_at(addr).block_id(id).into_future(),
        )?;
        Ok(Answered {
            value: AccountState {
                balance,
                nonce,
                code,
            },
            block_hash: at,
        })
    }

    async fn slot(&self, addr: Address, slot: U256, at: B256) -> anyhow::Result<Answered<U256>> {
        let v = self
            .provider
            .get_storage_at(addr, slot)
            .block_id(Self::at(at))
            .await?;
        Ok(Answered {
            value: v,
            block_hash: at,
        })
    }

    async fn l1_data_fee(&self, req: &SimulationRequest) -> anyhow::Result<L1DataFee> {
        if !self.with_l1_component {
            return Ok(L1DataFee::NotIncluded);
        }
        use alloy::sol_types::SolCall;
        let call = INodeInterface::gasEstimateL1ComponentCall {
            to: req.to,
            contractCreation: false,
            data: req.calldata.clone(),
        };
        let tx = TransactionRequest::default()
            .from(req.from)
            .to(NODE_INTERFACE)
            .input(call.abi_encode().into());
        let raw = self
            .provider
            .call(tx)
            .block(Self::at(req.block_pin.hash))
            .await?;
        let r = INodeInterface::gasEstimateL1ComponentCall::abi_decode_returns(&raw)?;
        Ok(L1DataFee::NodeInterface {
            gas_estimate_for_l1: r.gasEstimateForL1,
            base_fee: r.baseFee,
            l1_base_fee_estimate: r.l1BaseFeeEstimate,
            at: req.block_pin.hash,
        })
    }
}

/// What a plain `eth_call` (and `estimate_gas`) said at the same pinned block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EthCallOutcome {
    Success { output: Bytes, gas: Option<u64> },
    Revert { data: Bytes },
    Error(String),
}

/// A disagreement between REVM and the node. Recorded, never reconciled silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// One succeeded, the other did not.
    Status {
        revm_success: bool,
        eth_success: bool,
    },
    /// Both succeeded with different output (so a different executor-reported profit).
    Output { revm: Bytes, eth: Bytes },
    /// Both reverted with different data.
    RevertData { revm: Bytes, eth: Bytes },
    /// The node errored without revert data: nothing to compare.
    NodeError(String),
}

/// Outcome equality only: gas legitimately differs (REVM = L2 execution gas; `estimate_gas`
/// includes the L1 poster component and a search margin), so it is reported, not flagged.
pub fn compare(revm: &SimulationResult, eth: &EthCallOutcome) -> Vec<Mismatch> {
    match (&revm.status, eth) {
        (_, EthCallOutcome::Error(e)) => vec![Mismatch::NodeError(e.clone())],
        (Status::Success { output: a }, EthCallOutcome::Success { output: b, .. }) => {
            if a == b {
                vec![]
            } else {
                vec![Mismatch::Output {
                    revm: a.clone(),
                    eth: b.clone(),
                }]
            }
        }
        (Status::Revert { data: a, .. }, EthCallOutcome::Revert { data: b }) => {
            if a == b {
                vec![]
            } else {
                vec![Mismatch::RevertData {
                    revm: a.clone(),
                    eth: b.clone(),
                }]
            }
        }
        (s, e) => vec![Mismatch::Status {
            revm_success: matches!(s, Status::Success { .. }),
            eth_success: matches!(e, EthCallOutcome::Success { .. }),
        }],
    }
}

/// REVM simulation of exact executor calls over a pinned-block state source.
pub struct LiquidationSimulator<S> {
    pub source: S,
    pub chain_id: u64,
}

impl<S: StateSource> LiquidationSimulator<S> {
    pub fn new(source: S, chain_id: u64) -> Self {
        Self { source, chain_id }
    }

    pub async fn simulate(
        &self,
        req: &SimulationRequest,
        now_unix: u64,
    ) -> Result<SimulationResult, Refusal> {
        req.validate(now_unix)?;
        let pin = req.block_pin;
        let header = self
            .source
            .header_by_hash(pin.hash)
            .await
            .map_err(|e| Refusal::Provider(e.to_string()))?
            .ok_or(Refusal::UnknownBlock(pin.hash))?;
        if header.hash != pin.hash {
            return Err(Refusal::MixedHeight {
                expected: pin.hash,
                answered: header.hash,
            });
        }
        if header.number != pin.number {
            return Err(Refusal::PinNumberMismatch {
                pinned: pin.number,
                header: header.number,
            });
        }

        let mut cache = StateCache::new(u64::MAX / 2);
        cache.head = pin.number;
        let mut reads = 0usize;
        let mut outcome = None;
        for _ in 0..MAX_ROUNDS {
            let (result, missing) = self.run(&cache, req, &header)?;
            if missing.is_empty() {
                outcome = Some(result);
                break;
            }
            reads += missing.len();
            let fetched: Vec<Fetch> = futures::stream::iter(missing)
                .map(|k| self.fetch(k, pin.hash))
                .buffer_unordered(FETCH_CONCURRENCY)
                .try_collect()
                .await?;
            for f in fetched {
                match f {
                    Fetch::Account(a, s) => {
                        cache.insert_account(a, s.balance, s.nonce, s.code, pin.number)
                    }
                    Fetch::Slot(a, i, v) => cache.insert_slot(a, i, v, pin.number),
                }
            }
        }
        let (status, gas_used) = outcome.ok_or(Refusal::TooManyRounds)?;

        // The pin must still be the canonical block after all reads.
        let canonical = self
            .source
            .canonical_hash_at(pin.number)
            .await
            .map_err(|e| Refusal::Provider(e.to_string()))?;
        if canonical != Some(pin.hash) {
            return Err(Refusal::Reorged {
                pinned: pin.hash,
                canonical,
            });
        }
        let l1 = self
            .source
            .l1_data_fee(req)
            .await
            .map_err(|e| Refusal::Provider(format!("l1 data fee: {e}")))?;
        let profit = match &status {
            Status::Success { output } if output.len() == 32 => Some(U256::from_be_slice(output)),
            _ => None,
        };
        Ok(SimulationResult {
            input_state_id: pin.hash,
            block_number: pin.number,
            calldata_hash: req.calldata_hash,
            status,
            gas_used,
            l1_data_fee: l1,
            executor_reported_profit: profit,
            state_reads: reads,
        })
    }

    async fn fetch(&self, k: Key, at: B256) -> Result<Fetch, Refusal> {
        let perr = |e: anyhow::Error| Refusal::Provider(e.to_string());
        match k {
            Key::Account(a) => {
                let r = self.source.account(a, at).await.map_err(perr)?;
                check_height(at, r.block_hash)?;
                Ok(Fetch::Account(a, r.value))
            }
            Key::Slot(a, s) => {
                let r = self.source.slot(a, s, at).await.map_err(perr)?;
                check_height(at, r.block_hash)?;
                Ok(Fetch::Slot(a, s, r.value))
            }
        }
    }

    fn run(
        &self,
        cache: &StateCache,
        req: &SimulationRequest,
        header: &HeaderInfo,
    ) -> Result<((Status, u64), Vec<Key>), Refusal> {
        let db = SnapshotDb {
            cache,
            overlay: None,
            missing: Vec::new(),
            stale: Vec::new(),
        };
        let chain_id = self.chain_id;
        let mut evm = Context::mainnet()
            .with_db(db)
            .modify_cfg_chained(|c| {
                c.chain_id = chain_id;
                c.tx_chain_id_check = false;
                c.disable_nonce_check = true;
                c.disable_balance_check = true;
                c.disable_base_fee = true;
                c.disable_block_gas_limit = true;
            })
            .modify_block_chained(|b| {
                // eth_call at block N runs in N's own block context.
                b.number = U256::from(header.number);
                b.timestamp = U256::from(header.timestamp);
                b.basefee = 0;
                b.gas_limit = u64::MAX / 2;
            })
            .build_mainnet();
        let env = TxEnv::builder()
            .caller(req.from)
            .kind(TxKind::Call(req.to))
            .data(req.calldata.clone())
            .value(req.value)
            .gas_limit(req.gas_limit)
            .gas_price(0)
            .chain_id(Some(chain_id))
            .build_fill();
        let result = evm
            .transact(env)
            .map_err(|e| Refusal::Evm(format!("{e:?}")))?;
        let missing = std::mem::take(&mut evm.ctx.journaled_state.database.missing);
        let mut seen = HashSet::new();
        let missing: Vec<Key> = missing.into_iter().filter(|k| seen.insert(*k)).collect();
        let out = match result.result {
            ExecutionResult::Success { output, gas, .. } => {
                let bytes = match output {
                    Output::Call(b) => b,
                    Output::Create(b, _) => b,
                };
                (Status::Success { output: bytes }, gas.tx_gas_used())
            }
            ExecutionResult::Revert { output, gas, .. } => {
                let reason = decode_revert(&output);
                (
                    Status::Revert {
                        data: output,
                        reason,
                    },
                    gas.tx_gas_used(),
                )
            }
            ExecutionResult::Halt { reason, gas, .. } => (
                Status::Halt {
                    reason: format!("{reason:?}"),
                },
                gas.tx_gas_used(),
            ),
        };
        Ok((out, missing))
    }
}

enum Fetch {
    Account(Address, AccountState),
    Slot(Address, U256, U256),
}

fn check_height(expected: B256, answered: B256) -> Result<(), Refusal> {
    if expected == answered {
        Ok(())
    } else {
        Err(Refusal::MixedHeight { expected, answered })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    fn h(n: u8) -> B256 {
        B256::repeat_byte(n)
    }

    /// In-memory chain: one state, a movable canonical tip, an optional lying slot reader.
    struct Fixture {
        pin: BlockPin,
        accounts: HashMap<Address, AccountState>,
        slots: HashMap<(Address, U256), U256>,
        canonical: Mutex<B256>,
        slot_answers_hash: Option<B256>,
        header_hash: Option<B256>,
    }

    impl StateSource for Fixture {
        async fn header_by_hash(&self, hash: B256) -> anyhow::Result<Option<HeaderInfo>> {
            if hash != self.pin.hash {
                return Ok(None);
            }
            Ok(Some(HeaderInfo {
                hash: self.header_hash.unwrap_or(hash),
                number: self.pin.number,
                timestamp: 1_700_000_000,
            }))
        }
        async fn canonical_hash_at(&self, _n: u64) -> anyhow::Result<Option<B256>> {
            Ok(Some(*self.canonical.lock().unwrap()))
        }
        async fn account(&self, a: Address, at: B256) -> anyhow::Result<Answered<AccountState>> {
            Ok(Answered {
                value: self.accounts.get(&a).cloned().unwrap_or(AccountState {
                    balance: U256::ZERO,
                    nonce: 0,
                    code: Bytes::new(),
                }),
                block_hash: at,
            })
        }
        async fn slot(&self, a: Address, s: U256, at: B256) -> anyhow::Result<Answered<U256>> {
            Ok(Answered {
                value: self.slots.get(&(a, s)).copied().unwrap_or_default(),
                block_hash: self.slot_answers_hash.unwrap_or(at),
            })
        }
    }

    const CALLER: Address = Address::new([1; 20]);
    const TARGET: Address = Address::new([2; 20]);

    fn fixture(code: &[u8]) -> Fixture {
        let mut accounts = HashMap::new();
        accounts.insert(
            TARGET,
            AccountState {
                balance: U256::ZERO,
                nonce: 1,
                code: Bytes::copy_from_slice(code),
            },
        );
        let mut slots = HashMap::new();
        slots.insert((TARGET, U256::ZERO), U256::from(777u64));
        Fixture {
            pin: BlockPin {
                number: 100,
                hash: h(0xAA),
            },
            accounts,
            slots,
            canonical: Mutex::new(h(0xAA)),
            slot_answers_hash: None,
            header_hash: None,
        }
    }

    /// SLOAD(0) -> MSTORE(0) -> RETURN(0, 32)
    const READ_SLOT0: [u8; 11] = [
        0x60, 0x00, 0x54, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
    ];
    /// REVERT(0, 0)
    const REVERT_EMPTY: [u8; 5] = [0x60, 0x00, 0x60, 0x00, 0xfd];

    fn req(f: &Fixture, expiry: u64) -> SimulationRequest {
        SimulationRequest::new(
            f.pin,
            vec![0xde, 0xad, 0xbe, 0xef],
            CALLER,
            TARGET,
            U256::ZERO,
            1_000_000,
            expiry,
        )
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn lazily_fetches_pinned_state_and_reports_executor_word() {
        let f = fixture(&READ_SLOT0);
        let sim = LiquidationSimulator::new(f, 42161);
        let r = block_on(sim.simulate(&req(&sim.source, 10), 5)).unwrap();
        assert!(r.success());
        assert_eq!(r.executor_reported_profit, Some(U256::from(777u64)));
        assert_eq!(r.input_state_id, sim.source.pin.hash);
        assert_eq!(r.l1_data_fee, L1DataFee::NotIncluded);
        assert!(r.state_reads >= 2, "account + slot were fetched lazily");
        assert!(r.gas_used > 21_000);
        r.check_binds(&req(&sim.source, 10)).unwrap();
    }

    #[test]
    fn revert_is_a_result_with_decoded_reason() {
        let sim = LiquidationSimulator::new(fixture(&REVERT_EMPTY), 42161);
        let r = block_on(sim.simulate(&req(&sim.source, 10), 5)).unwrap();
        assert!(!r.success());
        assert_eq!(
            r.status,
            Status::Revert {
                data: Bytes::new(),
                reason: RevertReason::Empty
            }
        );
        assert_eq!(r.executor_reported_profit, None);
    }

    #[test]
    fn revert_decoding() {
        let msg = Revert::from("45").abi_encode();
        assert_eq!(decode_revert(&msg), RevertReason::Message("45".into()));
        let p = Panic {
            code: U256::from(0x11u64),
        }
        .abi_encode();
        assert_eq!(decode_revert(&p), RevertReason::Panic(U256::from(0x11u64)));
        assert_eq!(
            decode_revert(&[1, 2, 3, 4, 5]),
            RevertReason::Custom([1, 2, 3, 4])
        );
        assert_eq!(decode_revert(&[1, 2]), RevertReason::Malformed);
        // Observed from the deployed pool on a fork: the eligibility refusal.
        assert_eq!(
            aave_error_name([0x93, 0x0b, 0xb7, 0x71]),
            Some("HealthFactorNotBelowThreshold")
        );
        assert_eq!(aave_error_name([1, 2, 3, 4]), None);
        assert_eq!(decode_revert(&[]), RevertReason::Empty);
        // Truncated Error(string) is not mistaken for a message.
        assert_eq!(
            decode_revert(&msg[..10]),
            RevertReason::Custom([0x08, 0xc3, 0x79, 0xa0])
        );
    }

    #[test]
    fn calldata_hash_is_bound_and_expiry_enforced() {
        let sim = LiquidationSimulator::new(fixture(&READ_SLOT0), 42161);
        let mut bad = req(&sim.source, 10);
        bad.calldata = Bytes::from_static(&[1, 2, 3]);
        assert!(matches!(
            block_on(sim.simulate(&bad, 5)),
            Err(Refusal::CalldataHashMismatch { .. })
        ));
        assert!(matches!(
            block_on(sim.simulate(&req(&sim.source, 10), 11)),
            Err(Refusal::Expired { .. })
        ));
        // A result for other calldata does not bind to this request.
        let r = block_on(sim.simulate(&req(&sim.source, 10), 5)).unwrap();
        let mut other = req(&sim.source, 10);
        other.calldata_hash = h(1);
        assert!(matches!(
            r.check_binds(&other),
            Err(Refusal::CalldataMismatch { .. })
        ));
    }

    #[test]
    fn refuses_when_the_provider_answers_for_another_block() {
        let mut f = fixture(&READ_SLOT0);
        f.slot_answers_hash = Some(h(0xBB));
        let sim = LiquidationSimulator::new(f, 42161);
        assert_eq!(
            block_on(sim.simulate(&req(&sim.source, 10), 5)),
            Err(Refusal::MixedHeight {
                expected: h(0xAA),
                answered: h(0xBB)
            })
        );
        let mut g = fixture(&READ_SLOT0);
        g.header_hash = Some(h(0xCC));
        let sim = LiquidationSimulator::new(g, 42161);
        assert!(matches!(
            block_on(sim.simulate(&req(&sim.source, 10), 5)),
            Err(Refusal::MixedHeight { .. })
        ));
    }

    #[test]
    fn refuses_an_unknown_or_reorged_pin() {
        let f = fixture(&READ_SLOT0);
        let sim = LiquidationSimulator::new(f, 42161);
        let mut unknown = req(&sim.source, 10);
        unknown.block_pin.hash = h(0x01);
        assert_eq!(
            block_on(sim.simulate(&unknown, 5)),
            Err(Refusal::UnknownBlock(h(0x01)))
        );
        *sim.source.canonical.lock().unwrap() = h(0xDD);
        assert!(matches!(
            block_on(sim.simulate(&req(&sim.source, 10), 5)),
            Err(Refusal::Reorged { .. })
        ));
    }

    #[test]
    fn stale_opportunity_is_refused_after_the_state_moves() {
        let sim = LiquidationSimulator::new(fixture(&READ_SLOT0), 42161);
        let r = block_on(sim.simulate(&req(&sim.source, 10), 5)).unwrap();
        r.check_fresh(&sim.source.pin).unwrap();
        let next = BlockPin {
            number: 101,
            hash: h(0xEE),
        };
        assert_eq!(
            r.check_fresh(&next),
            Err(Refusal::StaleState {
                simulated: h(0xAA),
                current: h(0xEE)
            })
        );
    }

    #[test]
    fn comparison_flags_every_disagreement() {
        let sim = LiquidationSimulator::new(fixture(&READ_SLOT0), 42161);
        let r = block_on(sim.simulate(&req(&sim.source, 10), 5)).unwrap();
        let out = Bytes::from(U256::from(777u64).to_be_bytes::<32>().to_vec());
        assert!(
            compare(
                &r,
                &EthCallOutcome::Success {
                    output: out.clone(),
                    gas: Some(1)
                }
            )
            .is_empty()
        );
        assert_eq!(
            compare(
                &r,
                &EthCallOutcome::Success {
                    output: Bytes::new(),
                    gas: None
                }
            )
            .len(),
            1
        );
        assert!(matches!(
            compare(&r, &EthCallOutcome::Revert { data: out })[0],
            Mismatch::Status {
                revm_success: true,
                eth_success: false
            }
        ));
        assert!(matches!(
            compare(&r, &EthCallOutcome::Error("x".into()))[0],
            Mismatch::NodeError(_)
        ));
    }
}
