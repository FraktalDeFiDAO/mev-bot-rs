//! Startup check of the Aave v3 deployment against the pinned manifest (JEV-LIQ-002): chain id,
//! Pool and Oracle as resolved through the PoolAddressesProvider, and the Pool implementation's
//! runtime code hash. Read-only; the adapter's plan/sim/send behavior does not depend on it.

use alloy::{
    primitives::{Address, B256, U256, keccak256},
    providers::{DynProvider, Provider},
    sol,
};
use mev_config::{AaveConfig, AaveManifest};

sol! {
    #[sol(rpc)]
    interface IPoolAddressesProvider {
        function getPool() external view returns (address);
        function getPriceOracle() external view returns (address);
    }
}

/// EIP-1967 implementation slot: `bytes32(uint256(keccak256("eip1967.proxy.implementation")) - 1)`.
const IMPL_SLOT: B256 = B256::new(alloy::hex!(
    "360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
));

/// What the chain reported, all read at one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedDeployment {
    pub block: u64,
    pub chain_id: u64,
    pub provider_pool: Address,
    pub provider_oracle: Address,
    pub pool_implementation: Address,
    pub pool_implementation_code_hash: B256,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeploymentError {
    #[error("[aave.manifest] is missing: the Aave deployment is not pinned")]
    NoManifest,
    #[error("wrong chain: RPC reports {observed}, manifest pins {expected}")]
    ChainId { expected: u64, observed: u64 },
    #[error("PoolAddressesProvider resolves Pool {observed}, manifest pins {expected}")]
    Pool {
        expected: Address,
        observed: Address,
    },
    #[error("config [aave].pool {config} differs from manifest pool {manifest}")]
    ConfigPool { config: Address, manifest: Address },
    #[error("PoolAddressesProvider resolves Oracle {observed}, manifest pins {expected}")]
    Oracle {
        expected: Address,
        observed: Address,
    },
    #[error("config [aave].oracle {config} differs from manifest oracle {manifest}")]
    ConfigOracle { config: Address, manifest: Address },
    #[error("Pool implementation is {observed}, manifest pins {expected}")]
    Implementation {
        expected: Address,
        observed: Address,
    },
    #[error("Pool implementation code hash is {observed}, manifest pins {expected}")]
    CodeHash { expected: B256, observed: B256 },
    #[error("deployment query failed: {0}")]
    Rpc(String),
}

/// Pure comparison of config + manifest with what was observed. Order: chain, config vs
/// manifest, provider-resolved Pool/Oracle, implementation, code hash.
pub fn compare_deployment(
    cfg: &AaveConfig,
    observed: &ObservedDeployment,
) -> Result<(), DeploymentError> {
    let m = cfg.manifest.as_ref().ok_or(DeploymentError::NoManifest)?;
    if observed.chain_id != m.chain_id {
        return Err(DeploymentError::ChainId {
            expected: m.chain_id,
            observed: observed.chain_id,
        });
    }
    if cfg.pool != m.pool {
        return Err(DeploymentError::ConfigPool {
            config: cfg.pool,
            manifest: m.pool,
        });
    }
    if cfg.oracle != m.oracle {
        return Err(DeploymentError::ConfigOracle {
            config: cfg.oracle,
            manifest: m.oracle,
        });
    }
    if observed.provider_pool != m.pool {
        return Err(DeploymentError::Pool {
            expected: m.pool,
            observed: observed.provider_pool,
        });
    }
    if observed.provider_oracle != m.oracle {
        return Err(DeploymentError::Oracle {
            expected: m.oracle,
            observed: observed.provider_oracle,
        });
    }
    if observed.pool_implementation != m.pool_implementation {
        return Err(DeploymentError::Implementation {
            expected: m.pool_implementation,
            observed: observed.pool_implementation,
        });
    }
    if observed.pool_implementation_code_hash != m.pool_implementation_code_hash {
        return Err(DeploymentError::CodeHash {
            expected: m.pool_implementation_code_hash,
            observed: observed.pool_implementation_code_hash,
        });
    }
    Ok(())
}

/// Read the deployment from chain at one block (the head when called).
pub async fn observe_deployment(
    rpc: &DynProvider,
    m: &AaveManifest,
) -> Result<ObservedDeployment, DeploymentError> {
    let e =
        |what: &str, err: &dyn std::fmt::Display| DeploymentError::Rpc(format!("{what}: {err}"));
    let chain_id = rpc.get_chain_id().await.map_err(|x| e("eth_chainId", &x))?;
    let block = rpc
        .get_block_number()
        .await
        .map_err(|x| e("eth_blockNumber", &x))?;
    let at = block.into();
    let provider = IPoolAddressesProvider::new(m.pool_addresses_provider, rpc);
    let provider_pool = provider
        .getPool()
        .block(at)
        .call()
        .await
        .map_err(|x| e("getPool", &x))?;
    let provider_oracle = provider
        .getPriceOracle()
        .block(at)
        .call()
        .await
        .map_err(|x| e("getPriceOracle", &x))?;
    // The implementation behind the Pool the provider resolved (not the configured one).
    let slot = rpc
        .get_storage_at(provider_pool, U256::from_be_bytes(IMPL_SLOT.0))
        .block_id(at)
        .await
        .map_err(|x| e("implementation slot", &x))?;
    let pool_implementation = Address::from_word(B256::from(slot));
    let code = rpc
        .get_code_at(pool_implementation)
        .block_id(at)
        .await
        .map_err(|x| e("eth_getCode", &x))?;
    Ok(ObservedDeployment {
        block,
        chain_id,
        provider_pool,
        provider_oracle,
        pool_implementation,
        pool_implementation_code_hash: keccak256(&code),
    })
}

/// Verify the live deployment against the manifest; returns what was observed.
pub async fn verify_deployment(
    rpc: &DynProvider,
    cfg: &AaveConfig,
) -> Result<ObservedDeployment, DeploymentError> {
    let m = cfg.manifest.as_ref().ok_or(DeploymentError::NoManifest)?;
    let observed = observe_deployment(rpc, m).await?;
    compare_deployment(cfg, &observed)?;
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const POOL: Address = address!("794a61358D6845594F94dc1DB02A252b5b4814aD");
    const ORACLE: Address = address!("b56c2F0B653B2e0b10C9b928C8580Ac5Df02C7C7");
    const IMPL: Address = address!("F05Fd3cC911b4c5E36e53c00354F645E22922C9A");

    fn cfg() -> AaveConfig {
        AaveConfig {
            enabled: true,
            name: None,
            pool: POOL,
            data_provider: Address::ZERO,
            oracle: ORACLE,
            from_block: 0,
            scan_step: 1,
            manifest: Some(AaveManifest {
                chain_id: 42161,
                pool_addresses_provider: address!("a97684ead0e402dC232d5A977953DF7ECBaB3CDb"),
                pool: POOL,
                oracle: ORACLE,
                pool_implementation: IMPL,
                pool_implementation_code_hash: B256::repeat_byte(0x11),
                verified_block: 1,
                source: "test".into(),
            }),
        }
    }

    fn observed() -> ObservedDeployment {
        ObservedDeployment {
            block: 1,
            chain_id: 42161,
            provider_pool: POOL,
            provider_oracle: ORACLE,
            pool_implementation: IMPL,
            pool_implementation_code_hash: B256::repeat_byte(0x11),
        }
    }

    #[test]
    fn matching_deployment_passes() {
        assert_eq!(compare_deployment(&cfg(), &observed()), Ok(()));
    }

    #[test]
    fn wrong_chain_id_fails() {
        let o = ObservedDeployment {
            chain_id: 1,
            ..observed()
        };
        assert_eq!(
            compare_deployment(&cfg(), &o),
            Err(DeploymentError::ChainId {
                expected: 42161,
                observed: 1
            })
        );
    }

    #[test]
    fn wrong_pool_fails() {
        let bad = Address::repeat_byte(0xaa);
        let o = ObservedDeployment {
            provider_pool: bad,
            ..observed()
        };
        assert_eq!(
            compare_deployment(&cfg(), &o),
            Err(DeploymentError::Pool {
                expected: POOL,
                observed: bad
            })
        );
    }

    #[test]
    fn wrong_oracle_fails() {
        let bad = Address::repeat_byte(0xbb);
        let o = ObservedDeployment {
            provider_oracle: bad,
            ..observed()
        };
        assert_eq!(
            compare_deployment(&cfg(), &o),
            Err(DeploymentError::Oracle {
                expected: ORACLE,
                observed: bad
            })
        );
    }

    #[test]
    fn wrong_code_hash_fails() {
        let o = ObservedDeployment {
            pool_implementation_code_hash: B256::repeat_byte(0x22),
            ..observed()
        };
        assert_eq!(
            compare_deployment(&cfg(), &o),
            Err(DeploymentError::CodeHash {
                expected: B256::repeat_byte(0x11),
                observed: B256::repeat_byte(0x22)
            })
        );
    }

    #[test]
    fn upgraded_implementation_fails() {
        let bad = Address::repeat_byte(0xcc);
        let o = ObservedDeployment {
            pool_implementation: bad,
            ..observed()
        };
        assert_eq!(
            compare_deployment(&cfg(), &o),
            Err(DeploymentError::Implementation {
                expected: IMPL,
                observed: bad
            })
        );
    }

    #[test]
    fn config_drifting_from_manifest_fails() {
        let mut c = cfg();
        c.pool = Address::repeat_byte(0xdd);
        assert!(matches!(
            compare_deployment(&c, &observed()),
            Err(DeploymentError::ConfigPool { .. })
        ));
        let mut c = cfg();
        c.oracle = Address::repeat_byte(0xee);
        assert!(matches!(
            compare_deployment(&c, &observed()),
            Err(DeploymentError::ConfigOracle { .. })
        ));
    }

    #[test]
    fn missing_manifest_fails() {
        let mut c = cfg();
        c.manifest = None;
        assert_eq!(
            compare_deployment(&c, &observed()),
            Err(DeploymentError::NoManifest)
        );
    }

    /// The decode-layer `POOL` const (mev-protocol-aave-v3) cannot read config; this keeps it
    /// from drifting away from the single configured source.
    #[test]
    fn protocol_pool_const_matches_config() {
        let c = mev_config::ChainConfig::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/arbitrum.toml"),
        )
        .unwrap();
        let a = c.aave.unwrap();
        assert_eq!(a.pool, mev_protocol_aave_v3::POOL);
        assert_eq!(a.manifest.unwrap().pool, mev_protocol_aave_v3::POOL);
    }
}
