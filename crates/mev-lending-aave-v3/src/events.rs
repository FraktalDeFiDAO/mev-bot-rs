//! Aave v3 events that move borrower or reserve risk (JEV-LIQ-004), decoded exactly.
//!
//! ABI provenance: the event signatures below were checked against the VERIFIED source/ABI of the
//! Pool implementation pinned in `[aave.manifest]` (`0xF05Fd3cC...C9A`, Aave v3.6: it has
//! `DeficitCreated` and position-manager events), fetched from Sourcify
//! (`https://sourcify.dev/server/v2/contract/42161/<impl>?fields=abi`, runtime+creation `match`),
//! and against real Arbitrum logs emitted by the Pool proxy
//! (`tests/fixtures/arbitrum_events.json`, one per event, block and tx recorded there). The proxy
//! `Upgraded(address)` event is declared by the verified Pool proxy ABI; the oracle events by the
//! verified AaveOracle (`0xb56c...C7C7`, Sourcify `exact_match`).

use alloy::{
    primitives::{Address, B256},
    rpc::types::Log,
    sol_types::SolEvent,
};

mod abi {
    alloy::sol! {
        event Supply(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint16 indexed referralCode);
        event Withdraw(address indexed reserve, address indexed user, address indexed to, uint256 amount);
        event Borrow(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint8 interestRateMode, uint256 borrowRate, uint16 indexed referralCode);
        event Repay(address indexed reserve, address indexed user, address indexed repayer, uint256 amount, bool useATokens);
        event LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken);
        event ReserveUsedAsCollateralEnabled(address indexed reserve, address indexed user);
        event ReserveUsedAsCollateralDisabled(address indexed reserve, address indexed user);
        event UserEModeSet(address indexed user, uint8 categoryId);
        event ReserveDataUpdated(address indexed reserve, uint256 liquidityRate, uint256 stableBorrowRate, uint256 variableBorrowRate, uint256 liquidityIndex, uint256 variableBorrowIndex);
        event DeficitCreated(address indexed user, address indexed debtAsset, uint256 amountCreated);
        event Upgraded(address indexed implementation);
        event AssetSourceUpdated(address indexed asset, address indexed source);
        event FallbackOracleUpdated(address indexed fallbackOracle);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolEvent {
    Supply {
        reserve: Address,
        on_behalf_of: Address,
    },
    Withdraw {
        reserve: Address,
        user: Address,
    },
    Borrow {
        reserve: Address,
        on_behalf_of: Address,
    },
    Repay {
        reserve: Address,
        user: Address,
    },
    Liquidation {
        collateral: Address,
        debt: Address,
        user: Address,
        liquidator: Address,
        receive_a_token: bool,
    },
    CollateralEnabled {
        reserve: Address,
        user: Address,
    },
    CollateralDisabled {
        reserve: Address,
        user: Address,
    },
    EModeSet {
        user: Address,
    },
    ReserveDataUpdated {
        reserve: Address,
    },
    DeficitCreated {
        user: Address,
        debt_asset: Address,
    },
    /// Pool proxy implementation changed.
    Upgraded {
        implementation: Address,
    },
    AssetSourceUpdated {
        asset: Address,
        source: Address,
    },
    FallbackOracleUpdated,
}

#[derive(Debug, thiserror::Error)]
#[error("malformed {event} log: {reason}")]
pub struct DecodeError {
    pub event: &'static str,
    pub reason: String,
}

/// topic0 of every event the indexer subscribes to.
pub fn topics() -> Vec<B256> {
    vec![
        abi::Supply::SIGNATURE_HASH,
        abi::Withdraw::SIGNATURE_HASH,
        abi::Borrow::SIGNATURE_HASH,
        abi::Repay::SIGNATURE_HASH,
        abi::LiquidationCall::SIGNATURE_HASH,
        abi::ReserveUsedAsCollateralEnabled::SIGNATURE_HASH,
        abi::ReserveUsedAsCollateralDisabled::SIGNATURE_HASH,
        abi::UserEModeSet::SIGNATURE_HASH,
        abi::ReserveDataUpdated::SIGNATURE_HASH,
        abi::DeficitCreated::SIGNATURE_HASH,
        abi::Upgraded::SIGNATURE_HASH,
        abi::AssetSourceUpdated::SIGNATURE_HASH,
        abi::FallbackOracleUpdated::SIGNATURE_HASH,
    ]
}

/// Name -> topic0, for tests and diagnostics.
pub fn signature_hashes() -> Vec<(&'static str, B256)> {
    vec![
        ("Supply", abi::Supply::SIGNATURE_HASH),
        ("Withdraw", abi::Withdraw::SIGNATURE_HASH),
        ("Borrow", abi::Borrow::SIGNATURE_HASH),
        ("Repay", abi::Repay::SIGNATURE_HASH),
        ("LiquidationCall", abi::LiquidationCall::SIGNATURE_HASH),
        (
            "ReserveUsedAsCollateralEnabled",
            abi::ReserveUsedAsCollateralEnabled::SIGNATURE_HASH,
        ),
        (
            "ReserveUsedAsCollateralDisabled",
            abi::ReserveUsedAsCollateralDisabled::SIGNATURE_HASH,
        ),
        ("UserEModeSet", abi::UserEModeSet::SIGNATURE_HASH),
        (
            "ReserveDataUpdated",
            abi::ReserveDataUpdated::SIGNATURE_HASH,
        ),
        ("DeficitCreated", abi::DeficitCreated::SIGNATURE_HASH),
        ("Upgraded", abi::Upgraded::SIGNATURE_HASH),
        (
            "AssetSourceUpdated",
            abi::AssetSourceUpdated::SIGNATURE_HASH,
        ),
        (
            "FallbackOracleUpdated",
            abi::FallbackOracleUpdated::SIGNATURE_HASH,
        ),
    ]
}

macro_rules! dec {
    ($ev:ident, $log:expr) => {
        abi::$ev::decode_log(&$log.inner)
            .map(|l| l.data)
            .map_err(|e| DecodeError {
                event: stringify!($ev),
                reason: e.to_string(),
            })?
    };
}

/// Decode one log. `Ok(None)` for anything that is not one of ours (wrong emitter or topic0).
/// Pool events must come from `pool`; oracle events from `oracle`.
pub fn decode(log: &Log, pool: Address, oracle: Address) -> Result<Option<PoolEvent>, DecodeError> {
    let Some(t0) = log.topics().first().copied() else {
        return Ok(None);
    };
    let from = log.address();
    let from_pool = from == pool;
    let from_oracle = from == oracle;
    Ok(Some(match t0 {
        t if from_pool && t == abi::Supply::SIGNATURE_HASH => {
            let e = dec!(Supply, log);
            PoolEvent::Supply {
                reserve: e.reserve,
                on_behalf_of: e.onBehalfOf,
            }
        }
        t if from_pool && t == abi::Withdraw::SIGNATURE_HASH => {
            let e = dec!(Withdraw, log);
            PoolEvent::Withdraw {
                reserve: e.reserve,
                user: e.user,
            }
        }
        t if from_pool && t == abi::Borrow::SIGNATURE_HASH => {
            let e = dec!(Borrow, log);
            PoolEvent::Borrow {
                reserve: e.reserve,
                on_behalf_of: e.onBehalfOf,
            }
        }
        t if from_pool && t == abi::Repay::SIGNATURE_HASH => {
            let e = dec!(Repay, log);
            PoolEvent::Repay {
                reserve: e.reserve,
                user: e.user,
            }
        }
        t if from_pool && t == abi::LiquidationCall::SIGNATURE_HASH => {
            let e = dec!(LiquidationCall, log);
            PoolEvent::Liquidation {
                collateral: e.collateralAsset,
                debt: e.debtAsset,
                user: e.user,
                liquidator: e.liquidator,
                receive_a_token: e.receiveAToken,
            }
        }
        t if from_pool && t == abi::ReserveUsedAsCollateralEnabled::SIGNATURE_HASH => {
            let e = dec!(ReserveUsedAsCollateralEnabled, log);
            PoolEvent::CollateralEnabled {
                reserve: e.reserve,
                user: e.user,
            }
        }
        t if from_pool && t == abi::ReserveUsedAsCollateralDisabled::SIGNATURE_HASH => {
            let e = dec!(ReserveUsedAsCollateralDisabled, log);
            PoolEvent::CollateralDisabled {
                reserve: e.reserve,
                user: e.user,
            }
        }
        t if from_pool && t == abi::UserEModeSet::SIGNATURE_HASH => {
            let e = dec!(UserEModeSet, log);
            PoolEvent::EModeSet { user: e.user }
        }
        t if from_pool && t == abi::ReserveDataUpdated::SIGNATURE_HASH => {
            let e = dec!(ReserveDataUpdated, log);
            PoolEvent::ReserveDataUpdated { reserve: e.reserve }
        }
        t if from_pool && t == abi::DeficitCreated::SIGNATURE_HASH => {
            let e = dec!(DeficitCreated, log);
            PoolEvent::DeficitCreated {
                user: e.user,
                debt_asset: e.debtAsset,
            }
        }
        t if from_pool && t == abi::Upgraded::SIGNATURE_HASH => {
            let e = dec!(Upgraded, log);
            PoolEvent::Upgraded {
                implementation: e.implementation,
            }
        }
        t if from_oracle && t == abi::AssetSourceUpdated::SIGNATURE_HASH => {
            let e = dec!(AssetSourceUpdated, log);
            PoolEvent::AssetSourceUpdated {
                asset: e.asset,
                source: e.source,
            }
        }
        t if from_oracle && t == abi::FallbackOracleUpdated::SIGNATURE_HASH => {
            let _ = dec!(FallbackOracleUpdated, log);
            PoolEvent::FallbackOracleUpdated
        }
        _ => return Ok(None),
    }))
}
