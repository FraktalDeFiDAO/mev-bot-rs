//! New pools announced by exchange factories (`PoolCreated`, `PairCreated`, Algebra `Pool`). Pure: a log and the
//! configured exchanges in, a described pool out. Nothing here does I/O; the engine records every one of them and
//! decides whether to track it.

use alloy::{
    primitives::{Address, B256},
    rpc::types::Log,
    sol_types::SolEvent,
};
use mev_bindings::{IAlgebraFactory, ISolidlyFactory, IUniswapV2Factory, IUniswapV3Factory};
use mev_config::{DexConfig, DexType};
use mev_types::{PoolInfo, PoolKind};

/// A pool a factory just created.
#[derive(Debug, Clone)]
pub struct Created {
    pub info: PoolInfo,
    /// V3-style fee tier (pips) or V2 fixed fee (bps) when the event carries one.
    pub fee: Option<u32>,
    pub tick_spacing: Option<i32>,
    pub block: Option<u64>,
    pub tx: Option<B256>,
    pub log_index: Option<u64>,
}

/// Every creation topic worth subscribing to.
pub fn creation_topics() -> Vec<B256> {
    vec![
        IUniswapV3Factory::PoolCreated::SIGNATURE_HASH,
        IUniswapV2Factory::PairCreated::SIGNATURE_HASH,
        IAlgebraFactory::Pool::SIGNATURE_HASH,
        ISolidlyFactory::PoolCreated::SIGNATURE_HASH,
    ]
}

/// The factory addresses of the configured exchanges.
pub fn factories(dexes: &[DexConfig]) -> Vec<Address> {
    dexes.iter().map(|d| d.factory).collect()
}

/// Describe `log` if one of the configured factories emitted it.
pub fn parse(log: &Log, dexes: &[DexConfig]) -> Option<Created> {
    let dex = dexes.iter().find(|d| d.factory == log.address())?;
    let topic0 = *log.topic0()?;
    let (token0, token1, pool, kind, fee, tick_spacing) = match (&dex.kind, topic0) {
        (DexType::Univ3 | DexType::Pancakev3, t)
            if t == IUniswapV3Factory::PoolCreated::SIGNATURE_HASH =>
        {
            let ev = IUniswapV3Factory::PoolCreated::decode_log_data(log.data()).ok()?;
            let fee = ev.fee.to::<u32>();
            let kind = match &dex.kind {
                DexType::Pancakev3 => PoolKind::PancakeV3 { fee },
                _ => PoolKind::UniV3 { fee },
            };
            (
                ev.token0,
                ev.token1,
                ev.pool,
                kind,
                Some(fee),
                Some(ev.tickSpacing.as_i32()),
            )
        }
        (DexType::Univ2 | DexType::Camelot, t)
            if t == IUniswapV2Factory::PairCreated::SIGNATURE_HASH =>
        {
            let ev = IUniswapV2Factory::PairCreated::decode_log_data(log.data()).ok()?;
            let kind = match &dex.kind {
                DexType::Camelot => PoolKind::CamelotV2,
                _ => PoolKind::UniV2 {
                    fee_bps: dex.fee_bps,
                },
            };
            (
                ev.token0,
                ev.token1,
                ev.pair,
                kind,
                Some(u32::from(dex.fee_bps)),
                None,
            )
        }
        (DexType::Solidly, t) if t == ISolidlyFactory::PoolCreated::SIGNATURE_HASH => {
            let ev = ISolidlyFactory::PoolCreated::decode_log_data(log.data()).ok()?;
            // Stable pools use the x3y+y3x curve, which the model does not price.
            if ev.stable {
                return None;
            }
            (
                ev.token0,
                ev.token1,
                ev.pool,
                PoolKind::SolidlyV2 {
                    factory: dex.factory,
                },
                None,
                None,
            )
        }
        (DexType::Algebra, t) if t == IAlgebraFactory::Pool::SIGNATURE_HASH => {
            let ev = IAlgebraFactory::Pool::decode_log_data(log.data()).ok()?;
            (
                ev.token0,
                ev.token1,
                ev.pool,
                PoolKind::AlgebraV3,
                None,
                None,
            )
        }
        _ => return None,
    };
    if pool.is_zero() {
        return None;
    }
    Some(Created {
        info: PoolInfo {
            address: pool,
            dex: dex.name.clone(),
            kind,
            token0,
            token1,
        },
        fee,
        tick_spacing,
        block: log.block_number,
        tx: log.transaction_hash,
        log_index: log.log_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{
        U256,
        aliases::{I24, U24},
    };

    const F3: Address = Address::repeat_byte(0xf3);
    const F2: Address = Address::repeat_byte(0xf2);
    const FA: Address = Address::repeat_byte(0xfa);
    const FS: Address = Address::repeat_byte(0xf5);
    const A: Address = Address::with_last_byte(0xa);
    const B: Address = Address::with_last_byte(0xb);
    const POOL: Address = Address::with_last_byte(0x99);

    fn dexes() -> Vec<DexConfig> {
        let d = |name: &str, kind, factory, fee_bps| DexConfig {
            name: name.into(),
            kind,
            factory,
            fee_bps,
            fees: vec![],
        };
        vec![
            d("uniswap_v3", DexType::Univ3, F3, 0),
            d("sushi_v2", DexType::Univ2, F2, 30),
            d("camelot_v3", DexType::Algebra, FA, 0),
            d("aerodrome", DexType::Solidly, FS, 0),
        ]
    }

    fn log(emitter: Address, data: alloy::primitives::LogData) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: emitter,
                data,
            },
            block_number: Some(7),
            transaction_hash: Some(B256::repeat_byte(5)),
            log_index: Some(3),
            ..Default::default()
        }
    }

    #[test]
    fn each_factory_style_describes_the_pool_it_created() {
        let v3 = IUniswapV3Factory::PoolCreated {
            token0: A,
            token1: B,
            fee: U24::from(500),
            tickSpacing: I24::try_from(10).unwrap(),
            pool: POOL,
        };
        let c = parse(&log(F3, v3.encode_log_data()), &dexes()).unwrap();
        assert_eq!(c.info.kind, PoolKind::UniV3 { fee: 500 });
        assert_eq!(
            (
                c.info.address,
                c.info.token0,
                c.info.token1,
                c.tick_spacing,
                c.block
            ),
            (POOL, A, B, Some(10), Some(7))
        );

        let v2 = IUniswapV2Factory::PairCreated {
            token0: A,
            token1: B,
            pair: POOL,
            index: U256::from(1),
        };
        let c = parse(&log(F2, v2.encode_log_data()), &dexes()).unwrap();
        assert_eq!(c.info.kind, PoolKind::UniV2 { fee_bps: 30 });

        let alg = IAlgebraFactory::Pool {
            token0: A,
            token1: B,
            pool: POOL,
        };
        let c = parse(&log(FA, alg.encode_log_data()), &dexes()).unwrap();
        assert_eq!(c.info.kind, PoolKind::AlgebraV3);
        assert_eq!(c.info.dex, "camelot_v3");

        let solidly = |stable| ISolidlyFactory::PoolCreated {
            token0: A,
            token1: B,
            stable,
            pool: POOL,
            index: U256::from(1),
        };
        let c = parse(&log(FS, solidly(false).encode_log_data()), &dexes()).unwrap();
        assert_eq!(c.info.kind, PoolKind::SolidlyV2 { factory: FS });
        assert_eq!((c.info.address, c.info.dex.as_str()), (POOL, "aerodrome"));
        assert!(
            parse(&log(FS, solidly(true).encode_log_data()), &dexes()).is_none(),
            "stable (x3y+y3x) pools are not priced"
        );
    }

    #[test]
    fn logs_from_strangers_or_with_the_wrong_shape_are_ignored() {
        let v3 = IUniswapV3Factory::PoolCreated {
            token0: A,
            token1: B,
            fee: U24::from(500),
            tickSpacing: I24::try_from(10).unwrap(),
            pool: POOL,
        };
        // Not one of our factories.
        assert!(
            parse(
                &log(Address::repeat_byte(1), v3.encode_log_data()),
                &dexes()
            )
            .is_none()
        );
        // A V3 event from a V2 factory does not match its style.
        assert!(parse(&log(F2, v3.encode_log_data()), &dexes()).is_none());
        // Zero pool address.
        let zero = IAlgebraFactory::Pool {
            token0: A,
            token1: B,
            pool: Address::ZERO,
        };
        assert!(parse(&log(FA, zero.encode_log_data()), &dexes()).is_none());
    }
}
