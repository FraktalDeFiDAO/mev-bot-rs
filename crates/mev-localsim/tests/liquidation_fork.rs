//! JEV-LIQ-015 fork tests. All `#[ignore = "network"]`.
//!
//! * `real_*`: public Arbitrum RPCs only (recent blocks; no archive state is assumed).
//! * `anvil_*`: need `scripts/test-revm-liquidation-fork.sh`, which starts an anvil fork, deploys
//!   `ArbExecutor`, opens a borrower position, installs a mock price source over the WETH Aave
//!   source and exports the `REVM_FORK_*` variables read here.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::{
    primitives::{Address, B256, Bytes, U256, address},
    providers::{DynProvider, Provider},
    rpc::types::Filter,
    sol_types::{SolCall, SolEvent},
};
use mev_bindings::IAavePool;
use mev_localsim::liquidation::{
    BlockPin, EthCallOutcome, LiquidationSimulator, RpcStateSource, SimulationRequest,
    SimulationResult, Status, compare,
};
use mev_types::{Hop, Plan, ProtocolCall};

const POOL: Address = address!("794a61358D6845594F94dc1DB02A252b5b4814aD");
const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
/// Uniswap V3 WETH/USDC 0.05 %.
const V3_WETH_USDC: Address = address!("C6962004f452bE9203591991D15f6b388e09E8D0");
const WAD: u128 = 1_000_000_000_000_000_000;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn provider(url: &str) -> DynProvider {
    mev_rpc::http_provider(url).unwrap()
}

async fn pin_back(p: &DynProvider, back: u64) -> BlockPin {
    let head = p.get_block_number().await.unwrap();
    pin_number(p, head - back).await
}

async fn pin_number(p: &DynProvider, n: u64) -> BlockPin {
    let b = p.get_block_by_number(n.into()).await.unwrap().unwrap();
    BlockPin {
        number: b.header.number,
        hash: b.header.hash,
    }
}

fn account_data_req(pin: BlockPin, user: Address) -> SimulationRequest {
    SimulationRequest::new(
        pin,
        IAavePool::getUserAccountDataCall { user }.abi_encode(),
        Address::ZERO,
        POOL,
        U256::ZERO,
        10_000_000,
        now() + 600,
    )
}

#[tokio::test]
#[ignore = "network"]
async fn real_borrowers_revm_account_data_equals_block_pinned_eth_call() {
    let state_url = std::env::var("MEV_IT_RPC")
        .unwrap_or_else(|_| "https://arbitrum-one-rpc.publicnode.com".into());
    let logs_url =
        std::env::var("MEV_IT_LOG_RPC").unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".into());
    let (sp, lp) = (provider(&state_url), provider(&logs_url));
    let head = lp.get_block_number().await.unwrap();
    let f = Filter::new()
        .address(POOL)
        .event_signature(IAavePool::Borrow::SIGNATURE_HASH)
        .from_block(head - 20_000)
        .to_block(head);
    let mut users: Vec<Address> = lp
        .get_logs(&f)
        .await
        .unwrap()
        .iter()
        .rev()
        .filter_map(|l| l.topics().get(2).map(|t| Address::from_word(*t)))
        .collect();
    users.dedup();
    users.truncate(3);
    assert!(!users.is_empty(), "borrows in the last 20k blocks");

    let sim = LiquidationSimulator::new(RpcStateSource::new(sp.clone()), 42161);
    for user in users {
        // Public nodes keep only ~128 recent states and the reads take seconds: re-pin per user.
        let pin = pin_back(&sp, 1).await;
        eprintln!("PINNED block {} {}", pin.number, pin.hash);
        let req = account_data_req(pin, user);
        let r = sim.simulate(&req, now()).await.expect("simulation");
        let eth = sim.source.eth_call(&req).await;
        let mm = compare(&r, &eth);
        let Status::Success { output } = &r.status else {
            panic!("revm status {:?}", r.status)
        };
        let d = IAavePool::getUserAccountDataCall::abi_decode_returns(output).unwrap();
        eprintln!(
            "user {user}: hf={} debt_base={} collateral_base={} reads={} gas={} mismatches={mm:?}",
            d.healthFactor, d.totalDebtBase, d.totalCollateralBase, r.state_reads, r.gas_used
        );
        assert!(mm.is_empty(), "REVM vs eth_call at {}: {mm:?}", pin.number);
        assert_eq!(r.input_state_id, pin.hash);
    }
}

// ------------------------------------------------------------------ anvil fork tests

fn env(k: &str) -> String {
    std::env::var(k)
        .unwrap_or_else(|_| panic!("{k} not set; run scripts/test-revm-liquidation-fork.sh"))
}

/// One anvil chain, shared mutable state (price, time): tests take turns.
static CHAIN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fork {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    p: DynProvider,
    sim: LiquidationSimulator<RpcStateSource>,
    executor: Address,
    owner: Address,
    borrower: Address,
    receiver: Address,
    price_src: Address,
    px0: U256,
}

impl Fork {
    async fn load() -> Self {
        let guard = CHAIN.lock().await;
        let p = provider(&env("REVM_FORK_RPC"));
        Self {
            _guard: guard,
            sim: LiquidationSimulator::new(RpcStateSource::new(p.clone()), 31337),
            p,
            executor: env("REVM_FORK_EXECUTOR").parse().unwrap(),
            owner: env("REVM_FORK_OWNER").parse().unwrap(),
            borrower: env("REVM_FORK_BORROWER").parse().unwrap(),
            receiver: env("REVM_FORK_RECEIVER").parse().unwrap(),
            price_src: env("REVM_FORK_PRICE_SRC").parse().unwrap(),
            px0: env("REVM_FORK_PX0").parse().unwrap(),
        }
    }

    async fn rpc<T: alloy::rpc::json_rpc::RpcRecv>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> T {
        self.p
            .raw_request(method.into(), params)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// Set the WETH price and mine a block with the SAME timestamp as the previous one (interest
    /// accrual is time-dependent; a frozen clock keeps the HF boundary where it was found).
    async fn set_price_and_mine(&self, price: U256) -> BlockPin {
        let n = self.p.get_block_number().await.unwrap();
        let ts = self
            .p
            .get_block_by_number(n.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        let _: serde_json::Value = self
            .rpc("evm_setNextBlockTimestamp", serde_json::json!([ts]))
            .await;
        let _: serde_json::Value = self
            .rpc(
                "anvil_setStorageAt",
                serde_json::json!([
                    self.price_src,
                    format!("0x{:064x}", 0),
                    format!("0x{:064x}", price)
                ]),
            )
            .await;
        let _: serde_json::Value = self.rpc("evm_mine", serde_json::json!([])).await;
        let n = self.p.get_block_number().await.unwrap();
        pin_number(&self.p, n).await
    }

    async fn account_data(
        &self,
        pin: BlockPin,
    ) -> (
        SimulationResult,
        EthCallOutcome,
        IAavePool::getUserAccountDataReturn,
    ) {
        let req = account_data_req(pin, self.borrower);
        let r = self.sim.simulate(&req, now()).await.expect("revm");
        let eth = self.sim.source.eth_call(&req).await;
        let Status::Success { output } = &r.status else {
            panic!("account data: {:?}", r.status)
        };
        let d = IAavePool::getUserAccountDataCall::abi_decode_returns(output).unwrap();
        (r, eth, d)
    }

    /// Smallest price whose HF is >= 1e18 (binary search over the mock price; eth_call at latest).
    async fn boundary_price(&self) -> U256 {
        let (mut lo, mut hi) = (self.px0 / U256::from(2u8), self.px0); // lo: HF<1, hi: HF>=1
        for _ in 0..80 {
            if hi - lo <= U256::from(1u8) {
                break;
            }
            let mid = (lo + hi) / U256::from(2u8);
            let hf = self.hf_at_price(mid).await;
            if hf >= U256::from(WAD) {
                hi = mid
            } else {
                lo = mid
            }
        }
        assert!(self.hf_at_price(lo).await < U256::from(WAD));
        assert!(self.hf_at_price(hi).await >= U256::from(WAD));
        hi
    }

    /// Smallest price in `[px0/4, ceiling]` whose HF is >= `wad`.
    async fn price_for_hf(&self, ceiling: U256, wad: u128) -> U256 {
        let (mut lo, mut hi) = (self.px0 / U256::from(4u8), ceiling);
        for _ in 0..80 {
            if hi - lo <= U256::from(1u8) {
                break;
            }
            let mid = (lo + hi) / U256::from(2u8);
            if self.hf_at_price(mid).await >= U256::from(wad) {
                hi = mid
            } else {
                lo = mid
            }
        }
        hi
    }

    async fn hf_at_price(&self, price: U256) -> U256 {
        let pin = self.set_price_and_mine(price).await;
        IAavePool::new(POOL, &self.p)
            .getUserAccountData(self.borrower)
            .block(alloy::eips::BlockId::hash(pin.hash))
            .call()
            .await
            .unwrap()
            .healthFactor
    }

    fn liquidation_calldata(&self, flash: U256, min_profit: U256) -> Vec<u8> {
        let data = L::liquidationCallCall {
            c: WETH,
            d: USDC,
            u: self.borrower,
            amt: flash,
            r: false,
        }
        .abi_encode();
        let plan = Plan {
            protocol: "aave-v3",
            account: mev_lending::AccountRef {
                market: POOL,
                account: self.borrower,
                sub_id: 0,
            },
            debt_asset: USDC,
            collateral_asset: WETH,
            flash_amount: flash,
            dependencies: vec![],
            expected_collateral: 0.0,
            bonus_usd: 0.0,
            calls: vec![ProtocolCall {
                target: POOL,
                data: data.into(),
                approve_spender: POOL,
                approve_amount: flash,
            }],
        };
        let hops = vec![Hop {
            kind: 2,
            pool: V3_WETH_USDC,
            token_in: WETH,
            token_out: USDC,
            zero_for_one: true,
            fee_bps: 5,
            spacing: 0,
        }];
        mev_bindings::final_liquidation_calldata(&plan, &hops, min_profit, Some(POOL)).bytes
    }

    async fn usdc(&self, who: Address) -> U256 {
        E::new(USDC, &self.p).balanceOf(who).call().await.unwrap()
    }
}

alloy::sol! {
    #[sol(rpc)]
    interface E { function balanceOf(address) external view returns (uint256); }
    interface L { function liquidationCall(address c, address d, address u, uint256 amt, bool r) external; }
}

/// (b) eligibility boundary: REVM `getUserAccountData` equals the node's, exactly, one price unit
/// either side of HF == 1e18, and the exact liquidation call is refused/accepted accordingly.
#[tokio::test]
#[ignore = "network"]
async fn anvil_eligibility_boundary_is_exact_in_u256() {
    let f = Fork::load().await;
    let star = f.boundary_price().await;
    eprintln!(
        "BOUNDARY price: HF>=1e18 at {star}, HF<1e18 at {}",
        star - U256::from(1u8)
    );
    let one = U256::from(WAD);
    for (label, price, eligible) in [
        ("below", star - U256::from(1u8), true),
        ("at/above", star, false),
    ] {
        let pin = f.set_price_and_mine(price).await;
        let (r, eth, d) = f.account_data(pin).await;
        let mm = compare(&r, &eth);
        eprintln!(
            "B[{label}] block {} hf={} debt={} collateral={} revm_vs_eth_call={mm:?}",
            pin.number, d.healthFactor, d.totalDebtBase, d.totalCollateralBase
        );
        assert!(mm.is_empty());
        assert_eq!(d.healthFactor < one, eligible);
        // The executor call at this exact state.
        let debt = d.totalDebtBase / U256::from(100u8); // 8-dec USD base -> USDC 6 dec (1:1)
        let cd = f.liquidation_calldata(debt / U256::from(2u8), U256::ZERO);
        let req = SimulationRequest::new(
            pin,
            cd,
            f.owner,
            f.executor,
            U256::ZERO,
            5_000_000,
            now() + 600,
        );
        let r = f.sim.simulate(&req, now()).await.expect("revm exec");
        let eth = f.sim.source.eth_call(&req).await;
        eprintln!(
            "B[{label}] executor call: revm={:?} gas={} profit={:?} | eth_call={eth:?} | mismatches={:?}",
            r.status,
            r.gas_used,
            r.executor_reported_profit,
            compare(&r, &eth)
        );
        assert_eq!(
            r.success(),
            eligible,
            "executor call outcome follows eligibility"
        );
        assert!(compare(&r, &eth).is_empty());
    }
}

/// (b2)+(c): close factor 50 % vs 100 % and the exact executor calldata, REVM vs anvil eth_call vs
/// the mined receipt at the same state.
#[tokio::test]
#[ignore = "network"]
async fn anvil_close_factor_and_exact_calldata_revm_vs_eth_call_vs_receipt() {
    let f = Fork::load().await;
    let star = f.boundary_price().await;
    let (_, _, d0) = {
        let pin = f.set_price_and_mine(f.px0).await;
        f.account_data(pin).await
    };
    let debt_usdc = d0.totalDebtBase / U256::from(100u8);
    eprintln!(
        "POSITION debt_base={} collateral_base={}",
        d0.totalDebtBase, d0.totalCollateralBase
    );
    // price -> HF just under 1 (> 0.95) and clearly under 0.95
    let p_just_under = star - U256::from(1u8);
    let p_090 = f.price_for_hf(star, 900_000_000_000_000_000).await;
    let scenarios = [
        (
            "HF~1.0- flash 100% debt",
            p_just_under,
            debt_usdc + debt_usdc / U256::from(100u8),
        ),
        (
            "HF~1.0- flash 50% debt",
            p_just_under,
            debt_usdc / U256::from(2u8),
        ),
        (
            "HF~0.90 flash 100% debt",
            p_090,
            debt_usdc + debt_usdc / U256::from(100u8),
        ),
    ];
    let mut mismatches: Vec<String> = Vec::new();
    for (label, price, flash) in scenarios {
        let snap: String = f.rpc("evm_snapshot", serde_json::json!([])).await;
        let pin = f.set_price_and_mine(price).await;
        let (_, _, d) = f.account_data(pin).await;
        let pin_ts =
            f.p.get_block_by_number(pin.number.into())
                .await
                .unwrap()
                .unwrap()
                .header
                .timestamp;
        let cd = f.liquidation_calldata(flash, U256::ZERO);
        let req = SimulationRequest::new(
            pin,
            cd.clone(),
            f.owner,
            f.executor,
            U256::ZERO,
            8_000_000,
            now() + 600,
        );
        let revm = f.sim.simulate(&req, now()).await.expect("revm");
        let eth = f.sim.source.eth_call(&req).await;
        let cmp = compare(&revm, &eth);
        // Mine the same call, in a block with the pin's timestamp when anvil allows it.
        let ts_try: Result<serde_json::Value, _> =
            f.p.raw_request(
                "evm_setNextBlockTimestamp".into(),
                serde_json::json!([pin_ts]),
            )
            .await;
        let ts_same = ts_try.is_ok();
        if !ts_same {
            let _: serde_json::Value = f
                .rpc("evm_setNextBlockTimestamp", serde_json::json!([pin_ts + 1]))
                .await;
        }
        let before_recv = f.usdc(f.receiver).await;
        let tx = serde_json::json!([{"from": f.owner, "to": f.executor, "data": Bytes::from(cd), "gas": "0x7a1200"}]);
        let hash: B256 = f.rpc("eth_sendTransaction", tx).await;
        let mut rc = f.p.get_transaction_receipt(hash).await.unwrap();
        if rc.is_none() {
            eprintln!("C[{label}] tx {hash} not mined yet; mining");
            let _: serde_json::Value = f.rpc("evm_mine", serde_json::json!([])).await;
            rc = f.p.get_transaction_receipt(hash).await.unwrap();
        }
        let rc = rc.unwrap_or_else(|| panic!("no receipt for {hash}"));
        let recv_delta = f.usdc(f.receiver).await - before_recv;
        let (_, _, after) = f
            .account_data(pin_number(&f.p, rc.block_number.unwrap()).await)
            .await;
        let repaid_base = d.totalDebtBase.saturating_sub(after.totalDebtBase);
        let repaid_pct_x100 = if d.totalDebtBase.is_zero() {
            U256::ZERO
        } else {
            repaid_base * U256::from(10_000u32) / d.totalDebtBase
        };
        eprintln!(
            "C[{label}] pin {} hf={} | revm: {:?} gas={} profit={:?} | eth_call: {:?} | mined: success={} gas={} receiver_delta={} | debt repaid ~{}.{:02}% | ts_same={ts_same} | revm_vs_eth_call={cmp:?}",
            pin.number,
            d.healthFactor,
            match &revm.status {
                Status::Success { .. } => "success".to_string(),
                s => format!("{s:?}"),
            },
            revm.gas_used,
            revm.executor_reported_profit,
            eth,
            rc.status(),
            rc.gas_used,
            recv_delta,
            repaid_pct_x100 / U256::from(100u8),
            repaid_pct_x100 % U256::from(100u8)
        );
        if !cmp.is_empty() {
            mismatches.push(format!("{label}: revm vs eth_call {cmp:?}"));
        }
        if revm.success() != rc.status() {
            mismatches.push(format!(
                "{label}: revm success {} vs receipt {}",
                revm.success(),
                rc.status()
            ));
        }
        if revm.executor_reported_profit != Some(recv_delta) {
            mismatches.push(format!(
                "{label}: revm profit {:?} vs receiver delta {recv_delta}",
                revm.executor_reported_profit
            ));
        }
        if revm.gas_used != rc.gas_used {
            mismatches.push(format!(
                "{label}: revm gas {} vs receipt gas {}",
                revm.gas_used, rc.gas_used
            ));
        }
        let _: bool = f.rpc("evm_revert", serde_json::json!([snap])).await;
    }
    eprintln!("MISMATCHES ({}): {mismatches:#?}", mismatches.len());
}

/// (d) stale opportunity: the state moves after the simulation, the old result no longer matches
/// the current state id and is refused; a fresh simulation sees the new state.
#[tokio::test]
#[ignore = "network"]
async fn anvil_stale_simulation_is_refused_after_the_state_moves() {
    let f = Fork::load().await;
    let star = f.boundary_price().await;
    let pin = f.set_price_and_mine(star - U256::from(1u8)).await;
    let (_, _, d) = f.account_data(pin).await;
    let flash = d.totalDebtBase / U256::from(200u8);
    let cd = f.liquidation_calldata(flash, U256::ZERO);
    let req = SimulationRequest::new(
        pin,
        cd,
        f.owner,
        f.executor,
        U256::ZERO,
        8_000_000,
        now() + 600,
    );
    let old = f.sim.simulate(&req, now()).await.expect("revm");
    assert!(old.success(), "eligible state: {:?}", old.status);
    old.check_fresh(&pin).unwrap();
    // The price recovers: the account is healthy at the next block.
    let next = f.set_price_and_mine(star).await;
    assert_ne!(next.hash, pin.hash);
    let refused = old.check_fresh(&next);
    eprintln!("STALE: {refused:?}");
    assert!(refused.is_err());
    // A fresh simulation of the SAME bytes at the new pin reverts: the opportunity is gone.
    let mut again = req.clone();
    again.block_pin = next;
    let fresh = f.sim.simulate(&again, now()).await.expect("revm");
    eprintln!("STALE fresh result: {:?}", fresh.status);
    assert!(!fresh.success());
    fresh.check_fresh(&next).unwrap();
}
