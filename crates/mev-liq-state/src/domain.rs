use alloy::primitives::{Address, B256, U256, keccak256};

/// 1e18: Aave's health factor is a WAD; a position is liquidatable strictly below it.
pub const WAD: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountKey {
    pub chain_id: u64,
    pub pool: Address,
    pub borrower: Address,
}

/// Whether the block the snapshot was read at is known to be on the canonical chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChainSource {
    Canonical,
    Provisional,
}

impl ChainSource {
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::Provisional => 0,
            Self::Canonical => 1,
        }
    }
    pub(crate) fn from_code(c: i64) -> Option<Self> {
        match c {
            0 => Some(Self::Provisional),
            1 => Some(Self::Canonical),
            _ => None,
        }
    }
}

/// The one block a snapshot was read at (never a mix of heights).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainRef {
    pub block_number: u64,
    pub block_hash: B256,
    pub source: ChainSource,
    pub observed_at_ns: u64,
}

/// A block position used for checkpoints and reorg ancestors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainPoint {
    pub block_number: u64,
    pub block_hash: B256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DirtyReason {
    PriceMove,
    ReserveConfig,
    BorrowerEvent,
    Reorg,
    Stale,
    Manual,
}

impl DirtyReason {
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::PriceMove => 1,
            Self::ReserveConfig => 2,
            Self::BorrowerEvent => 3,
            Self::Reorg => 4,
            Self::Stale => 5,
            Self::Manual => 6,
        }
    }
    pub(crate) fn from_code(c: i64) -> Option<Self> {
        Some(match c {
            1 => Self::PriceMove,
            2 => Self::ReserveConfig,
            3 => Self::BorrowerEvent,
            4 => Self::Reorg,
            5 => Self::Stale,
            6 => Self::Manual,
            _ => return None,
        })
    }
}

/// Raw per-reserve balances in the reserve's own token units. `getUserAccountData` does not return
/// these, so the Aave rechecker leaves the list empty until a later task reads them
/// (data-provider `getUserReserveData` at the same block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReserveBalance {
    pub asset: Address,
    pub collateral_raw: U256,
    pub debt_raw: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    Valid,
    /// Rolled back by a reorg; must be rechecked before any use.
    Invalidated,
}

/// Immutable view of one borrower at one block. `revision` is assigned by the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowerSnapshot {
    pub key: AccountKey,
    pub revision: u64,
    pub chain: ChainRef,
    pub health_factor_wad: U256,
    pub total_debt_base: U256,
    pub total_collateral_base: U256,
    pub available_borrows_base: U256,
    pub liquidation_threshold: U256,
    pub ltv: U256,
    pub reserves: Vec<ReserveBalance>,
    pub risk_metadata_version: u64,
    pub dirty_reasons: Vec<DirtyReason>,
    pub last_successful_check_ns: Option<u64>,
    pub status: SnapshotStatus,
    pub provenance: String,
    pub feature_digest: B256,
}

impl BorrowerSnapshot {
    /// Exact liquidation eligibility: `HF < 1e18` and debt > 0. Aave reports `U256::MAX` for
    /// zero-debt accounts, which this also rejects via the debt check. Pure math; ignores `status`.
    pub fn eligible(&self) -> bool {
        self.health_factor_wad < WAD && !self.total_debt_base.is_zero()
    }

    /// Hash of the canonical data fields (account, block, source, amounts, reserves, risk version).
    /// Deliberately excludes bookkeeping that changes without the chain data changing: revision,
    /// observation time, dirty reasons, last check, status, provenance. Deterministic: fixed-width
    /// big-endian fields, length-prefixed lists, domain-separated.
    pub fn compute_digest(&self) -> B256 {
        let mut b: Vec<u8> = Vec::with_capacity(256 + self.reserves.len() * 96);
        b.extend_from_slice(b"mev-liq-state/borrower-snapshot/v1");
        b.extend_from_slice(&self.key.chain_id.to_be_bytes());
        b.extend_from_slice(self.key.pool.as_slice());
        b.extend_from_slice(self.key.borrower.as_slice());
        b.extend_from_slice(&self.chain.block_number.to_be_bytes());
        b.extend_from_slice(self.chain.block_hash.as_slice());
        b.push(self.chain.source.code() as u8);
        for v in [
            self.health_factor_wad,
            self.total_debt_base,
            self.total_collateral_base,
            self.available_borrows_base,
            self.liquidation_threshold,
            self.ltv,
        ] {
            b.extend_from_slice(&v.to_be_bytes::<32>());
        }
        b.extend_from_slice(&(self.reserves.len() as u64).to_be_bytes());
        for r in &self.reserves {
            b.extend_from_slice(r.asset.as_slice());
            b.extend_from_slice(&r.collateral_raw.to_be_bytes::<32>());
            b.extend_from_slice(&r.debt_raw.to_be_bytes::<32>());
        }
        b.extend_from_slice(&self.risk_metadata_version.to_be_bytes());
        keccak256(&b)
    }

    /// Recompute and store `feature_digest`.
    pub fn seal(mut self) -> Self {
        self.feature_digest = self.compute_digest();
        self
    }

    pub fn digest_matches(&self) -> bool {
        self.feature_digest == self.compute_digest()
    }
}
