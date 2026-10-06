//! Chainlink SVR / FastLane Atlas solver protocol: the auction feed, solver operations, their
//! EIP-712 signatures and the submission frames. Pure protocol, no strategy: what to bid is the
//! caller's decision (`mev-engine`). Spec: docs.chain.link/data-feeds/svr-feeds/searcher-onboarding-atlas
//! (Atlas v1.6.4); the signing rules are checked against the deployed `AtlasVerification` by
//! `mev-atlas-check`.

pub mod client;
pub mod sign;

use alloy::primitives::{Address, B256, Bytes, U256, address};
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};

/// Atlas v1.6.4 on Arbitrum One (docs.atlasevm.com deployments).
pub const ATLAS: Address = address!("8ad1aE9D97C79aA68A0a151E83ff3942f68F86C1");
pub const ATLAS_VERIFICATION: Address = address!("AC116AbB948E26B023c9C4815ab001845Fbf54fF");
/// `ChainlinkOevDAppControl` on Arbitrum One (the control of the SVR auctions there).
pub const ARBITRUM_CONTROL: Address = address!("e15BBa987C002ecc3586e81244517877D294d291");
/// Version string in `AtlasVerification`'s EIP-712 domain.
pub const ATLAS_VERSION: &str = "1.6.4";
/// Chainlink's solver auction endpoint.
pub const SOLVER_WS: &str = "wss://svr-bid-endpoint.chain.link/ws/solver";

/// What an auction notification tells a solver about the oracle update it wraps.
#[derive(Debug, Clone, PartialEq)]
pub struct Hints {
    /// The aggregator (feed) being updated.
    pub aggregator: Address,
    /// The new median answer, raw aggregator units (what `report_answer` calls the answer).
    pub median_price: U256,
    pub raw_report: Bytes,
}

/// One auction: the user operation (the oracle update) solvers bid to run after.
#[derive(Debug, Clone, PartialEq)]
pub struct Auction {
    pub id: String,
    pub chain_id: u64,
    pub user_op_hash: B256,
    pub to: Address,
    pub gas: U256,
    /// The gas price every solver operation must sign for, exactly.
    pub max_fee_per_gas: U256,
    /// Block number deadline.
    pub deadline: U256,
    pub dapp: Address,
    pub control: Address,
    pub value: U256,
    pub data: Bytes,
    pub from: Address,
    pub hints: Option<Hints>,
}

#[derive(Deserialize)]
struct RawHints {
    aggregator: Address,
    #[serde(rename = "medianPrice")]
    median_price: U256,
    #[serde(rename = "rawReport", default)]
    raw_report: Bytes,
}

#[derive(Deserialize)]
struct RawOp {
    #[serde(rename = "chainId")]
    chain_id: U256,
    #[serde(rename = "userOpHash")]
    user_op_hash: B256,
    to: Address,
    gas: U256,
    #[serde(rename = "maxFeePerGas")]
    max_fee_per_gas: U256,
    deadline: U256,
    dapp: Address,
    control: Address,
    #[serde(default)]
    value: U256,
    #[serde(default)]
    data: Bytes,
    from: Address,
    hints: Option<RawHints>,
}

/// The subscribe request.
pub fn subscribe_frame(id: u64) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": "solver_subscribe", "params": ["userOperations"] })
        .to_string()
}

/// Parse a websocket text frame: `Some(auction)` for a `solver_subscription` notification, `None`
/// for anything else (subscribe ack, errors, pings), `Err` for a malformed notification.
pub fn parse_notification(text: &str) -> Result<Option<Auction>> {
    let v: Value = serde_json::from_str(text).context("not JSON")?;
    if v["method"] != "solver_subscription" {
        return Ok(None);
    }
    let result = &v["params"]["result"];
    let id = result["auction_id"]
        .as_str()
        .ok_or_else(|| anyhow!("no auction_id"))?
        .to_string();
    let op: RawOp = serde_json::from_value(result["partial_user_operation"].clone())
        .context("bad partial_user_operation")?;
    Ok(Some(Auction {
        id,
        chain_id: op.chain_id.try_into().context("chainId")?,
        user_op_hash: op.user_op_hash,
        to: op.to,
        gas: op.gas,
        max_fee_per_gas: op.max_fee_per_gas,
        deadline: op.deadline,
        dapp: op.dapp,
        control: op.control,
        value: op.value,
        data: op.data,
        from: op.from,
        hints: op.hints.map(|h| Hints {
            aggregator: h.aggregator,
            median_price: h.median_price,
            raw_report: h.raw_report,
        }),
    }))
}

/// The `solver_submitSolverOperation` request for a signed operation.
pub fn submit_frame(id: u64, auction_id: &str, op: &sign::SignedSolverOp) -> String {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "solver_submitSolverOperation",
        "params": [{ "auction_id": auction_id, "auction_solution": op.to_json() }],
    })
    .to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn notification() -> String {
        format!(
            r#"{{
      "jsonrpc": "2.0", "method": "solver_subscription",
      "params": {{ "subscription": "0xabc", "result": {{
        "auction_id": "6f1f4f5e-0000-4000-8000-000000000001",
        "partial_user_operation": {{
          "chainId": "0xa4b1",
          "userOpHash": "0x{h}",
          "to": "0x8ad1ae9d97c79aa68a0a151e83ff3942f68f86c1",
          "gas": "0x30d40", "maxFeePerGas": "0x25f3b84",
          "deadline": "0x1e8ee9b0",
          "dapp": "0xe7c522c6c0a8ea4a8d8d0a3a6f6b8fd28b2f6c11",
          "control": "0xe15bba987c002ecc3586e81244517877d294d291",
          "value": "0x0", "data": "0xdeadbeef",
          "from": "0x8ada73a5c0dd4cf7b2f3c8d9a1d5b2f4e6a7b8c9",
          "hints": {{ "aggregator": "0xe7c522c6c0a8ea4a8d8d0a3a6f6b8fd28b2f6c11",
                     "medianPrice": "0x2540be400", "rawReport": "0x01" }} }} }} }} }}"#,
            h = "11".repeat(32)
        )
    }

    #[test]
    fn parses_an_auction_notification() {
        let a = parse_notification(&notification()).unwrap().unwrap();
        assert_eq!(a.id, "6f1f4f5e-0000-4000-8000-000000000001");
        assert_eq!(a.chain_id, 42161);
        assert_eq!(a.to, ATLAS);
        assert_eq!(a.max_fee_per_gas, U256::from(0x25f3b84u64));
        assert_eq!(a.deadline, U256::from(0x1e8ee9b0u64));
        let h = a.hints.unwrap();
        assert_eq!(h.median_price, U256::from(10_000_000_000u64));
        assert_eq!(
            h.aggregator,
            address!("e7c522c6c0a8ea4a8d8d0a3a6f6b8fd28b2f6c11")
        );
    }

    #[test]
    fn ignores_non_notifications_and_rejects_malformed_ones() {
        assert_eq!(
            parse_notification(r#"{"jsonrpc":"2.0","id":1,"result":"0xsub"}"#).unwrap(),
            None
        );
        assert!(parse_notification("not json").is_err());
        let bad = r#"{"method":"solver_subscription","params":{"result":{"auction_id":"x","partial_user_operation":{}}}}"#;
        assert!(parse_notification(bad).is_err());
    }

    #[test]
    fn subscribe_frame_matches_the_spec() {
        let v: Value = serde_json::from_str(&subscribe_frame(1)).unwrap();
        assert_eq!(v["method"], "solver_subscribe");
        assert_eq!(v["params"], json!(["userOperations"]));
    }
}
