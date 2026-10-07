//! Live read-only check (ignored): `simulate_after` runs a trigger and a second call in one simulated
//! block and reports the second call's result. Needs ARBITRUM_HTTP_URL (an endpoint with eth_simulateV1).

use alloy::{
    primitives::{Address, address},
    providers::{Provider, ProviderBuilder},
};
use serde_json::json;

const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
const SENDER: Address = address!("0000000000000000000000000000000000c0ffee");

#[tokio::test]
#[ignore = "network"]
async fn a_second_call_is_verified_on_top_of_the_first() {
    let url = std::env::var("ARBITRUM_HTTP_URL").expect("ARBITRUM_HTTP_URL");
    let p = ProviderBuilder::new()
        .connect_http(url.parse().unwrap())
        .erased();
    // Trigger: a harmless call. Then: WETH.symbol() -> "WETH".
    let ok_trigger = json!({ "from": SENDER, "to": WETH, "input": "0x06fdde03", "gas": "0x30d40" });
    let then = json!({ "from": SENDER, "to": WETH, "input": "0x95d89b41" });
    let r = mev_presim::simulate_after(&p, &ok_trigger, then.clone())
        .await
        .expect("simulates");
    assert!(r.ok, "second call succeeds: {}", r.error);
    assert!(r.return_data.len() >= 64, "symbol() returns an ABI string");
    // A trigger that reverts (transfer of tokens the sender does not have): nothing is verified.
    // transfer(0x…2222, 0xffff), assembled from parts.
    let transfer_input = format!("0xa9059cbb{:0>64}{:0>64}", "22".repeat(20), "ffff");
    let bad = json!({ "from": SENDER, "to": WETH,
        "input": transfer_input,
        "gas": "0x30d40" });
    let r = mev_presim::simulate_after(&p, &bad, then)
        .await
        .expect("simulates");
    assert!(
        !r.ok && r.error.contains("trigger"),
        "a reverting trigger does not verify: {r:?}"
    );
}
