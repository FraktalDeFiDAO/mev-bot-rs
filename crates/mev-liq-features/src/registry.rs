//! Pinned token classification registry (`policies/liq_token_classes_v1.toml`).

use std::collections::BTreeMap;

use alloy::primitives::Address;
use serde::Deserialize;

use crate::bands::AssetClass;

const PINNED: &str = include_str!("../../../policies/liq_token_classes_v1.toml");

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("registry toml: {0}")]
    Parse(String),
    #[error(
        "unsupported class `{0}` (allowed: stablecoin, volatile_eth_correlated, volatile_btc_correlated, other_volatile)"
    )]
    BadClass(String),
    #[error("invalid address `{0}`")]
    BadAddress(String),
    #[error("duplicate asset {0}")]
    Duplicate(Address),
}

#[derive(Deserialize)]
struct RawRegistry {
    registry_version: u32,
    chain_id: u64,
    #[serde(default)]
    asset: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    address: String,
    class: String,
}

/// Immutable asset -> class map for one chain. Unlisted assets are `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassRegistry {
    version: u32,
    chain_id: u64,
    classes: BTreeMap<Address, AssetClass>,
}

impl ClassRegistry {
    pub fn from_toml_str(s: &str) -> Result<Self, RegistryError> {
        let raw: RawRegistry =
            toml::from_str(s).map_err(|e| RegistryError::Parse(e.to_string()))?;
        let mut classes = BTreeMap::new();
        for a in raw.asset {
            let addr: Address = a
                .address
                .parse()
                .map_err(|_| RegistryError::BadAddress(a.address.clone()))?;
            let class = match a.class.as_str() {
                "stablecoin" => AssetClass::Stablecoin,
                "volatile_eth_correlated" => AssetClass::VolatileEthCorrelated,
                "volatile_btc_correlated" => AssetClass::VolatileBtcCorrelated,
                "other_volatile" => AssetClass::OtherVolatile,
                other => return Err(RegistryError::BadClass(other.to_string())),
            };
            if classes.insert(addr, class).is_some() {
                return Err(RegistryError::Duplicate(addr));
            }
        }
        Ok(Self {
            version: raw.registry_version,
            chain_id: raw.chain_id,
            classes,
        })
    }

    /// The registry checked in at `policies/liq_token_classes_v1.toml`, embedded at build time.
    pub fn pinned() -> Result<Self, RegistryError> {
        Self::from_toml_str(PINNED)
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// Class of `asset` on `chain_id`; `Unknown` for another chain or an unlisted asset.
    pub fn classify(&self, chain_id: u64, asset: &Address) -> AssetClass {
        if chain_id != self.chain_id {
            return AssetClass::Unknown;
        }
        self.classes
            .get(asset)
            .copied()
            .unwrap_or(AssetClass::Unknown)
    }
}
