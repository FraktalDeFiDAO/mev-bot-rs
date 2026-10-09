//! The engine on real and constructed transactions: what matches, what each action does, and every way a rule can
//! be wrong.

use alloy::{
    primitives::{Address, B256, Bytes, U256, address},
    sol,
    sol_types::SolCall,
};
use mev_decode::calls::MULTICALL3;
use mev_protocol_uniswap_router::SWAP_ROUTER_02;
use seqscope_core::{ActionKind, Engine, Mode, Profile, Rule, RuleSet, Tx, preview};

sol! {
    struct P { address tokenIn; address tokenOut; uint24 fee; address recipient; uint256 amountIn; uint256 amountOutMinimum; uint160 sqrtPriceLimitX96; }
    function exactInputSingle(P params) external payable returns (uint256 amountOut);
    function multicall(bytes[] data) external payable returns (bytes[] results);
    struct C3 { address target; bool allowFailure; bytes callData; }
    function aggregate3(C3[] calls) external payable returns (bytes[] returnData);
}

fn tx(hash: u8, to: Option<Address>, input: Vec<u8>) -> Tx {
    Tx {
        hash: B256::repeat_byte(hash),
        seq: 1_000 + u64::from(hash),
        t_ms: 1_700_000_000_000,
        from: Some(Address::with_last_byte(0xf0)),
        to,
        tx_type: 2,
        value: U256::ZERO,
        gas_limit: 300_000,
        max_fee: 10_000_000,
        tip: Some(1),
        nonce: 7,
        input: Bytes::from(input),
    }
}

fn swap_call(amount: u64) -> Vec<u8> {
    exactInputSingleCall {
        params: P {
            tokenIn: Address::with_last_byte(0xa1),
            tokenOut: Address::with_last_byte(0xb2),
            fee: alloy::primitives::Uint::<24, 1>::from(500u32),
            recipient: Address::with_last_byte(1),
            amountIn: U256::from(amount),
            amountOutMinimum: U256::ZERO,
            sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
        },
    }
    .abi_encode()
}

/// Two SwapRouter02 swaps inside a router multicall inside Multicall3, plus a call nothing reads.
fn wrapped() -> Tx {
    let router = multicallCall {
        data: vec![swap_call(1_000).into(), swap_call(2_000_000).into()],
    }
    .abi_encode();
    let outer = aggregate3Call {
        calls: vec![
            C3 {
                target: SWAP_ROUTER_02,
                allowFailure: false,
                callData: router.into(),
            },
            C3 {
                target: Address::with_last_byte(0x44),
                allowFailure: true,
                callData: vec![9, 9, 9, 9].into(),
            },
        ],
    }
    .abi_encode();
    tx(1, Some(MULTICALL3), outer)
}

fn ur_v4() -> Vec<(Tx, B256)> {
    include_str!("../../mev-protocol-uniswap-router/tests/fixtures/ur_v4_rows.csv")
        .lines()
        .skip(1)
        .enumerate()
        .map(|(i, row)| {
            let f: Vec<&str> = row.split(',').collect();
            let input = alloy::hex::decode(f[2].trim_start_matches("0x")).unwrap();
            (
                tx(10 + i as u8, Some(f[1].parse().unwrap()), input),
                f[3].parse().unwrap(),
            )
        })
        .collect()
}

fn rule(name: &str, when: &str, action: ActionKind) -> Rule {
    Rule {
        name: name.into(),
        enabled: true,
        when: when.into(),
        action,
        tag: None,
        sample: 1,
    }
}

fn one(rules: Vec<Rule>, mode: Mode) -> RuleSet {
    RuleSet {
        version: 1,
        profile: vec![Profile {
            name: "p".into(),
            enabled: true,
            mode,
            rule: rules,
        }],
    }
}

fn engine(rules: Vec<Rule>, mode: Mode) -> Engine {
    Engine::compile(&one(rules, mode)).unwrap_or_else(|e| {
        panic!(
            "{}",
            e.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn errors(when: &str) -> String {
    match Engine::compile(&one(vec![rule("r", when, ActionKind::Store)], Mode::All)) {
        Ok(_) => panic!("{when:?} compiled"),
        Err(e) => e
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

#[test]
fn real_universal_router_v4_swaps_match_on_decoded_fields() {
    let txs = ur_v4();
    assert_eq!(txs.len(), 3);
    let e = engine(
        vec![rule(
            "big",
            "kind == swap && protocol == uniswap_router && swap.amount_in >= 100e6",
            ActionKind::Store,
        )],
        Mode::All,
    );
    let rows: Vec<_> = txs.iter().map(|(t, _)| e.process(t)).collect();
    assert_eq!(rows[0].len(), 1, "110,000,000 qualifies");
    assert_eq!(rows[1].len(), 1, "0.43 ETH qualifies");
    assert_eq!(rows[2].len(), 0, "78,974,100 is below the threshold");
}

#[test]
fn the_pool_filter_uses_the_id_the_chain_logged() {
    let txs = ur_v4();
    for (t, id) in &txs {
        let stand_in = Address::from_slice(&id[12..]);
        let e = engine(
            vec![rule(
                "pool",
                &format!("swap.pool == {stand_in:#x}"),
                ActionKind::Store,
            )],
            Mode::All,
        );
        assert_eq!(e.process(t).len(), 1, "{id}");
        let other = engine(
            vec![rule(
                "pool",
                "swap.pool == 0x0000000000000000000000000000000000000001",
                ActionKind::Store,
            )],
            Mode::All,
        );
        assert!(other.process(t).is_empty());
    }
}

#[test]
fn wrapped_calls_are_judged_one_by_one_with_their_wrapper_path() {
    let t = wrapped();
    let all = engine(vec![rule("all", "true", ActionKind::Store)], Mode::All).process(&t);
    assert_eq!(all.len(), 3, "two swaps and one unread call");
    let swaps = engine(
        vec![rule(
            "s",
            "kind == swap && via contains multicall3 && via contains self_multicall",
            ActionKind::Store,
        )],
        Mode::All,
    )
    .process(&t);
    assert_eq!(swaps.len(), 2);
    assert_eq!(swaps[0].fields["swap.amount_in"], "1000");
    assert_eq!(swaps[1].fields["swap.amount_in"], "2000000");
    assert_eq!(swaps[0].via, vec!["multicall3", "self_multicall"]);
    assert_eq!(
        swaps[0].tx_to.as_deref(),
        Some(format!("{MULTICALL3:#x}").as_str())
    );
    assert_eq!(
        swaps[0].to.as_deref(),
        Some(format!("{SWAP_ROUTER_02:#x}").as_str())
    );
    assert_eq!(swaps[0].status, "pending");
    let direct = engine(vec![rule("d", "depth == 0", ActionKind::Store)], Mode::All).process(&t);
    assert!(
        direct.is_empty(),
        "nothing in this transaction is a direct call"
    );
    let gaps = engine(
        vec![rule(
            "g",
            "kind == unknown && selector == 0x09090909",
            ActionKind::Store,
        )],
        Mode::All,
    )
    .process(&t);
    assert_eq!(gaps.len(), 1);
    let big = engine(
        vec![rule(
            "b",
            "swap.amount_in > 1e6 && depth >= 2",
            ActionKind::Store,
        )],
        Mode::All,
    )
    .process(&t);
    assert_eq!(big.len(), 1);
}

#[test]
fn modes_actions_and_sampling() {
    let t = wrapped();
    // all: every matching rule applies.
    let e = engine(
        vec![
            rule("a", "kind == swap", ActionKind::Store),
            rule("b", "kind == swap", ActionKind::Raw),
        ],
        Mode::All,
    );
    assert_eq!(e.process(&t).len(), 4);
    // first: the first matching rule per call stops the profile.
    let e = engine(
        vec![
            rule("a", "kind == swap", ActionKind::Store),
            rule("b", "kind == swap", ActionKind::Raw),
        ],
        Mode::First,
    );
    let rows = e.process(&t);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.rule == "a" && r.input.is_none()));
    // drop stops the profile for that call and stores nothing; later rules do not see it.
    let e = engine(
        vec![
            rule("drop", "kind == swap", ActionKind::Drop),
            rule("rest", "true", ActionKind::Store),
        ],
        Mode::All,
    );
    let rows = e.process(&t);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "unknown");
    // count stores nothing but counts.
    let e = engine(
        vec![rule("c", "kind == swap", ActionKind::Count)],
        Mode::All,
    );
    assert!(e.process(&t).is_empty());
    assert_eq!(e.stats()[0].matches, 2);
    // raw carries the calldata; alert flags the row.
    let rows = engine(vec![rule("r", "kind == swap", ActionKind::Raw)], Mode::All).process(&t);
    assert!(
        rows.iter()
            .all(|r| r.input.as_deref().is_some_and(|i| i.starts_with("0x")))
    );
    let rows = engine(
        vec![rule("a", "kind == swap", ActionKind::Alert)],
        Mode::All,
    )
    .process(&t);
    assert!(rows.iter().all(|r| r.alert));
}

#[test]
fn sampling_is_deterministic_and_keeps_about_one_in_n() {
    let mut r = rule("s", "true", ActionKind::Store);
    r.sample = 4;
    let e = engine(vec![r.clone()], Mode::All);
    let kept: Vec<bool> = (0..=255u8)
        .map(|h| {
            !e.process(&tx(h, Some(Address::with_last_byte(5)), vec![1, 2, 3, 4]))
                .is_empty()
        })
        .collect();
    let n = kept.iter().filter(|k| **k).count();
    assert!((32..=96).contains(&n), "kept {n} of 256");
    let again = engine(vec![r], Mode::All);
    let kept2: Vec<bool> = (0..=255u8)
        .map(|h| {
            !again
                .process(&tx(h, Some(Address::with_last_byte(5)), vec![1, 2, 3, 4]))
                .is_empty()
        })
        .collect();
    assert_eq!(kept, kept2, "the same transactions are kept every time");
}

#[test]
fn disabled_rules_and_profiles_do_nothing() {
    let t = wrapped();
    let mut set = one(vec![rule("a", "true", ActionKind::Store)], Mode::All);
    set.profile[0].rule[0].enabled = false;
    assert!(Engine::compile(&set).unwrap().process(&t).is_empty());
    let mut set = one(vec![rule("a", "true", ActionKind::Store)], Mode::All);
    set.profile[0].enabled = false;
    assert!(Engine::compile(&set).unwrap().process(&t).is_empty());
    // A disabled rule with a broken `when` does not stop the others from loading.
    let mut set = one(
        vec![
            rule("good", "true", ActionKind::Store),
            rule("broken", "nonsense ==", ActionKind::Store),
        ],
        Mode::All,
    );
    set.profile[0].rule[1].enabled = false;
    assert!(Engine::compile(&set).is_ok());
}

#[test]
fn a_field_the_call_does_not_have_never_matches_even_with_not_equal() {
    let t = wrapped();
    for when in [
        "liq.token_id == 5",
        "liq.token_id != 5",
        "liquidation.amount > 0",
        "liquidation.borrower != 0x0000000000000000000000000000000000000001",
    ] {
        assert!(
            engine(vec![rule("r", when, ActionKind::Store)], Mode::All)
                .process(&t)
                .is_empty(),
            "{when}"
        );
    }
    // set() is the way to ask.
    let rows = engine(
        vec![rule(
            "r",
            "set(swap.pool) || set(swap.amount_in)",
            ActionKind::Store,
        )],
        Mode::All,
    )
    .process(&t);
    assert_eq!(rows.len(), 2);
    let rows = engine(
        vec![rule("r", "!set(swap.amount_in)", ActionKind::Store)],
        Mode::All,
    )
    .process(&t);
    assert_eq!(rows.len(), 1);
}

#[test]
fn a_contract_creation_is_one_call_with_no_destination() {
    let t = tx(3, None, vec![0x60, 0x80]);
    let rows = engine(vec![rule("r", "true", ActionKind::Store)], Mode::All).process(&t);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].to.as_deref(), rows[0].kind.as_str()),
        (None, "unknown")
    );
    assert!(
        engine(vec![rule("r", "set(to)", ActionKind::Store)], Mode::All)
            .process(&t)
            .is_empty()
    );
}

#[test]
fn input_contains_finds_an_address_or_bytes_in_the_calldata() {
    let t = wrapped();
    let a = format!("{:#x}", Address::with_last_byte(0xa1));
    assert_eq!(
        engine(
            vec![rule("r", &format!("input contains {a}"), ActionKind::Store)],
            Mode::All
        )
        .process(&t)
        .len(),
        2
    );
    assert!(
        engine(
            vec![rule(
                "r",
                "input contains 0xdeadbeefcafe",
                ActionKind::Store
            )],
            Mode::All
        )
        .process(&t)
        .is_empty()
    );
}

#[test]
fn text_fields_match_by_substring_without_regard_to_case() {
    let (t, _) = ur_v4().remove(0);
    let e = engine(
        vec![rule(
            "r",
            "known contains \"universal router\"",
            ActionKind::Store,
        )],
        Mode::All,
    );
    assert_eq!(e.process(&t).len(), 1);
}

#[test]
fn mistakes_are_reported_by_name_with_a_hint() {
    assert!(
        errors("swap.amount > 1").contains("did you mean swap.amount_in"),
        "{}",
        errors("swap.amount > 1")
    );
    assert!(errors("selector == 0xa415bc").contains("selector must be 4 bytes"));
    assert!(errors("to == 0x1234").contains("an address must be 20 bytes"));
    assert!(errors("to > 0x0000000000000000000000000000000000000001").contains("only == and !="));
    assert!(errors("kind == swop").contains("choose one of: swap, liquidity"));
    assert!(errors("protocol == aave3").contains("aave_v3"));
    assert!(errors("via contains nothing").contains("multicall3"));
    assert!(errors("selector in []").contains("list at least one value"));
    assert!(errors("input >= 1").contains("use `input contains"));
    assert!(errors("value contains 5").contains("`contains` works on"));
    assert!(errors("input contains 0xabc").contains("whole bytes"));
    assert!(errors("swap.amount_in > 1.5e18").contains("not a whole number"));
    assert!(errors("swap.amount_in > abc").contains("does not fit"));
    assert!(errors("nonce == 1e90").contains("too large"));
    assert!(errors("a == 1 &&").contains("expected"));
}

#[test]
fn every_broken_rule_is_reported_at_once_with_its_place() {
    let set = one(
        vec![
            rule("ok", "true", ActionKind::Store),
            rule("one", "bogus == 1", ActionKind::Store),
            rule("two", "kind == ", ActionKind::Store),
        ],
        Mode::All,
    );
    let errs = Engine::compile(&set).err().unwrap();
    assert_eq!(errs.len(), 2);
    assert_eq!((errs[0].rule.as_str(), errs[0].column), ("one", None));
    assert_eq!((errs[1].rule.as_str(), errs[1].column), ("two", Some(9)));
    let mut zero = rule("z", "true", ActionKind::Store);
    zero.sample = 0;
    assert!(
        Engine::compile(&one(vec![zero], Mode::All)).err().unwrap()[0]
            .message
            .contains("at least 1")
    );
}

#[test]
fn the_rule_file_round_trips_and_rejects_typos() {
    let text = r#"
version = 1

[[profile]]
name = "aave-borrows"
mode = "first"

[[profile.rule]]
name = "large borrows"
when = 'protocol == aave_v3 && selector == 0xa415bcad'
action = "raw"
tag = "aave"
sample = 2

[[profile.rule]]
name = "the rest"
when = "true"
"#;
    let set = RuleSet::from_toml(text).unwrap();
    assert_eq!(set.profile[0].mode, Mode::First);
    assert_eq!(set.profile[0].rule[0].action, ActionKind::Raw);
    assert_eq!(
        (
            set.profile[0].rule[1].action,
            set.profile[0].rule[1].sample,
            set.profile[0].rule[1].enabled
        ),
        (ActionKind::Store, 1, true)
    );
    assert_eq!(RuleSet::from_toml(&set.to_toml().unwrap()).unwrap(), set);
    assert!(Engine::compile(&set).is_ok());

    assert!(
        RuleSet::from_toml(&text.replace("action = \"raw\"", "acton = \"raw\"")).is_err(),
        "a misspelt key is an error, not ignored"
    );
    assert!(
        RuleSet::from_toml(&text.replace("version = 1", "version = 9"))
            .unwrap_err()
            .contains("not supported")
    );
    assert!(RuleSet::from_toml(&text.replace("\"first\"", "\"sometimes\"")).is_err());
}

#[test]
fn preview_counts_and_samples_without_saving_anything() {
    let txs: Vec<Tx> = ur_v4()
        .into_iter()
        .map(|(t, _)| t)
        .chain([wrapped()])
        .collect();
    let p = preview("kind == swap", &txs, 2).unwrap();
    assert_eq!(p.transactions, 4);
    assert!(p.matched >= 5 && p.samples.len() == 2 && p.calls >= p.matched);
    assert!(preview("kind ==", &txs, 2).is_err());
}

#[test]
fn processing_is_deterministic() {
    let txs: Vec<Tx> = ur_v4()
        .into_iter()
        .map(|(t, _)| t)
        .chain([wrapped()])
        .collect();
    let rules = vec![
        rule("a", "kind == swap", ActionKind::Raw),
        rule("b", "kind == unknown", ActionKind::Store),
    ];
    let run = || {
        let e = engine(rules.clone(), Mode::All);
        txs.iter().flat_map(|t| e.process(t)).collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn a_transaction_survives_serialisation() {
    let t = wrapped();
    let j = serde_json::to_string(&t).unwrap();
    assert_eq!(serde_json::from_str::<Tx>(&j).unwrap(), t);
}

#[test]
fn the_field_table_is_consistent() {
    use seqscope_core::{catalogue, field::FieldId};
    let mut seen = std::collections::HashSet::new();
    for f in FieldId::ALL {
        assert!(seen.insert(f.name()), "duplicate {}", f.name());
        assert_eq!(FieldId::from_name(f.name()), Some(*f));
        assert!(!f.doc().is_empty());
    }
    let c = catalogue::catalogue();
    assert_eq!(c.fields.len(), FieldId::ALL.len());
    let json = serde_json::to_value(&c).unwrap();
    assert!(
        json["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "aave_v3")
    );
    assert!(
        json["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "kind" && f["values"].is_array())
    );
    assert_eq!(
        catalogue::protocol_of(SWAP_ROUTER_02),
        Some("uniswap_router")
    );
    assert_eq!(
        catalogue::protocol_of(address!("0000000000000000000000000000000000000001")),
        None
    );
}
