use alloy::primitives::{Address, U256};
use mev_liq_replay::{ReplayConfig, event::*, replay};
use mev_liq_state::WAD;
use proptest::prelude::*;

const MS: u64 = 1_000_000;

fn borrower(n: u8) -> Address {
    Address::repeat_byte(n)
}

fn header() -> Header {
    Header {
        format_version: 1,
        dataset_id: "synthetic-tests".into(),
        synthetic: true,
        scenario: "behavior tests".into(),
        provenance: "hand-written synthetic fixtures".into(),
        chain_id: 42161,
        pool: Address::repeat_byte(0xaa),
        source_block_start: 100,
        source_block_end: 200,
        duration_ns: 1_000 * MS,
        measure_from_ns: 0,
        seed: Some(1),
    }
}

fn event(seq: u64, ms: u64, block: u64, body: Body) -> Event {
    Event {
        seq,
        t_ns: ms * MS,
        block,
        body,
    }
}

fn state(n: u8, hf: U256) -> Body {
    Body::State {
        borrower: borrower(n),
        health_factor_wad: hf,
        total_debt_base: U256::from(100),
        total_collateral_base: U256::from(200),
        reserve_asset: Some(borrower(0xee)),
    }
}

fn stream(events: Vec<Event>) -> ReplayStream {
    parse_jsonl(&to_jsonl(&header(), &events)).unwrap()
}

fn healthy(n: u8) -> Vec<Event> {
    vec![
        event(
            1,
            0,
            100,
            Body::Register {
                borrower: borrower(n),
            },
        ),
        event(2, 0, 100, state(n, WAD * U256::from(2))),
    ]
}

fn config() -> ReplayConfig {
    let mut c = ReplayConfig::default();
    c.policy.background_after_ns = 200 * MS;
    c.policy.max_account_age_ns = 500 * MS;
    c.policy.expiring_window_ns = 100 * MS;
    c.policy.eligible_refresh_ns = 100 * MS;
    c
}

#[test]
fn truth_only_changes_never_trigger_checks_or_leak_future_values() {
    let mut events = healthy(1);
    events.push(event(3, 30, 101, state(1, WAD - U256::from(1))));
    events.push(event(
        4,
        30,
        101,
        Body::Eligible {
            borrower: borrower(1),
            actionable: true,
        },
    ));
    let r = replay(&stream(events), &config(), "test").unwrap();
    assert!(!r.ordered_checks[0].eligible);
    assert_eq!(r.ordered_checks[0].started_ns, 0);
    assert!(r.ordered_checks.iter().all(|r| r.started_ns != 30 * MS));
    assert_eq!(r.detected_episodes, 1);
    assert_eq!(r.eligibility_detection_latency.p50_ns, Some(190 * MS));
}

#[test]
fn completion_uses_pinned_start_state_and_does_not_credit_a_closed_episode() {
    let mut events = healthy(1);
    events[1].body = state(1, WAD - U256::from(1));
    events.push(event(
        3,
        0,
        100,
        Body::Eligible {
            borrower: borrower(1),
            actionable: true,
        },
    ));
    events.push(event(4, 10, 101, state(1, WAD * U256::from(2))));
    let r = replay(&stream(events), &config(), "test").unwrap();
    assert!(r.ordered_checks[0].eligible);
    assert_eq!(r.detected_episodes, 0);
    assert_eq!(r.invalidated_label_episodes, 0);
    assert_eq!(r.detected_actionable_label_episodes, 0);
}

#[test]
fn reorg_restores_ancestor_truth_and_invalidates_inflight_work() {
    let mut events = healthy(1);
    events.push(event(3, 0, 101, state(1, WAD - U256::from(1))));
    events.push(event(
        4,
        0,
        101,
        Body::Eligible {
            borrower: borrower(1),
            actionable: true,
        },
    ));
    events.push(event(
        5,
        10,
        102,
        Body::Reorg {
            ancestor_block: 100,
        },
    ));
    let s = stream(events);
    assert_eq!(
        GroundTruth::build(&s.events)
            .unwrap()
            .at(&borrower(1), 11 * MS)
            .unwrap()
            .health_factor_wad,
        WAD * U256::from(2)
    );
    let r = replay(&s, &config(), "test").unwrap();
    assert_eq!(r.reorgs, 1);
    assert_eq!(r.invalidated_checks, 1);
    assert!(r.ordered_checks[0].invalidated);
    assert!(!r.ordered_checks[0].eligible);
    assert_eq!(r.invalidated_label_episodes, 1);
    assert_eq!(r.detected_episodes, 0);
    assert_eq!(r.ordered_checks[1].state_block, 100);
    assert!(!r.ordered_checks[1].eligible);
}

#[test]
fn post_read_borrower_event_survives_completion_of_older_check() {
    let mut events = healthy(1);
    events.push(event(3, 10, 101, state(1, WAD - U256::from(1))));
    events.push(event(
        4,
        10,
        101,
        Body::Eligible {
            borrower: borrower(1),
            actionable: false,
        },
    ));
    events.push(event(
        5,
        10,
        101,
        Body::BorrowerEvent {
            borrower: borrower(1),
        },
    ));
    let r = replay(&stream(events), &config(), "test").unwrap();
    assert_eq!(r.ordered_checks[1].started_ns, 20 * MS);
    assert!(r.ordered_checks[1].eligible);
    assert_eq!(r.eligibility_detection_latency.p50_ns, Some(30 * MS));
}

#[test]
fn unknown_dependency_participates_in_oracle_sweep() {
    let mut events = healthy(1);
    if let Body::State { reserve_asset, .. } = &mut events[1].body {
        *reserve_asset = None;
    }
    events.push(event(3, 50, 101, state(1, WAD - U256::from(1))));
    events.push(event(
        4,
        50,
        101,
        Body::Eligible {
            borrower: borrower(1),
            actionable: false,
        },
    ));
    events.push(event(
        5,
        50,
        101,
        Body::OracleMove {
            asset: borrower(0xef),
            move_bps: 100,
        },
    ));
    let r = replay(&stream(events), &config(), "test").unwrap();
    assert_eq!(r.ordered_checks[1].started_ns, 50 * MS);
    assert_eq!(r.detected_episodes, 1);
}

#[test]
fn overload_is_reported_and_accounts_are_not_dropped() {
    let mut events = vec![];
    for n in 1..=12 {
        events.push(event(
            u64::from(n) * 2,
            0,
            100,
            Body::Register {
                borrower: borrower(n),
            },
        ));
        events.push(event(
            u64::from(n) * 2 + 1,
            0,
            100,
            state(n, WAD * U256::from(2)),
        ));
    }
    let mut cfg = config();
    cfg.policy.concurrency = 1;
    cfg.policy.recheck_cost_ns = 100 * MS;
    cfg.policy.p1_deadline_ns = 10 * MS;
    let r = replay(&stream(events), &cfg, "test").unwrap();
    assert!(r.peak_mandatory_backlog >= 11);
    assert!(r.unique_mandatory_deadline_misses > 0);
    assert!(r.coverage_violation_observations > 0);
    assert_eq!(r.completed_checks, 10);
    assert_eq!(r.modeled_account_reads, 10);
    let checked: std::collections::BTreeSet<_> =
        r.ordered_checks.iter().map(|r| r.borrower).collect();
    assert_eq!(checked.len(), 10);
}

#[test]
fn warmup_and_right_censoring_are_explicit() {
    let mut events = healthy(1);
    events[1].body = state(1, WAD - U256::from(1));
    events.push(event(
        3,
        0,
        100,
        Body::Eligible {
            borrower: borrower(1),
            actionable: true,
        },
    ));
    let mut h = header();
    h.duration_ns = 10 * MS;
    h.measure_from_ns = 5 * MS;
    let s = parse_jsonl(&to_jsonl(&h, &events)).unwrap();
    let r = replay(&s, &config(), "test").unwrap();
    assert_eq!(r.pending_checks, 1);
    assert_eq!(r.completed_checks, 0);
    assert_eq!(r.eligible_episodes, 0);
    assert_eq!(r.modeled_busy_ns, 10 * MS);
}

#[test]
fn resource_and_clock_limits_fail_explicitly() {
    let s = stream(healthy(1));
    let mut cfg = config();
    cfg.max_steps = 1;
    assert!(
        replay(&s, &cfg, "test")
            .unwrap_err()
            .to_string()
            .contains("step limit")
    );
    cfg = config();
    cfg.max_checks = 1;
    assert!(
        replay(&s, &cfg, "test")
            .unwrap_err()
            .to_string()
            .contains("record limit")
    );
    cfg = config();
    cfg.tick_ns = 0;
    assert!(replay(&s, &cfg, "test").is_err());
}

#[test]
fn absent_state_is_reported_and_never_becomes_a_successful_safe_read() {
    let s = stream(vec![event(
        1,
        0,
        100,
        Body::Register {
            borrower: borrower(1),
        },
    )]);
    let r = replay(&s, &config(), "test").unwrap();
    assert!(r.completed_checks > 0);
    assert_eq!(r.completed_checks, r.unavailable_state_checks);
    assert!(
        r.ordered_checks
            .iter()
            .all(|r| !r.state_available && !r.eligible)
    );
}

#[test]
fn gap_forces_a_safety_recheck_without_advisor_or_truth_changes() {
    let mut events = healthy(1);
    events.push(event(
        3,
        50,
        102,
        Body::Gap {
            from_block: 101,
            to_block: 102,
        },
    ));
    events.push(event(
        4,
        90,
        103,
        Body::GapClosed {
            from_block: 101,
            to_block: 102,
        },
    ));
    let r = replay(&stream(events), &config(), "test").unwrap();
    assert_eq!(r.ordered_checks[1].started_ns, 50 * MS);
    assert_eq!(r.ordered_checks[1].tier, mev_liq_sched::Tier::P1);
}

#[test]
fn oversized_input_and_invalid_decimal_money_fail_before_replay() {
    assert!(parse_jsonl(&" ".repeat(MAX_INPUT_BYTES + 1)).is_err());
    let base = to_jsonl(&header(), &healthy(1));
    let invalid_values: Vec<String> =
        serde_json::from_str(include_str!("fixtures/invalid_money.json")).unwrap();
    for invalid in invalid_values {
        let bad = base.replace(
            "\"total_debt_base\":\"100\"",
            &format!("\"total_debt_base\":\"{invalid}\""),
        );
        assert!(parse_jsonl(&bad).is_err(), "{invalid}");
    }
}

#[test]
fn strict_loader_rejects_unknown_fields_bad_labels_gaps_and_order() {
    let base = to_jsonl(&header(), &healthy(1));
    let unknown = base.replace(
        "\"kind\":\"Register\"",
        "\"kind\":\"Register\",\"secret\":true",
    );
    assert!(parse_jsonl(&unknown).is_err());
    assert!(
        parse_jsonl(&base.replace(
            "\"chain_id\":42161",
            "\"chain_id\":42161,\"chain_id\":42161"
        ))
        .is_err()
    );
    assert!(parse_jsonl(&base.replace("42161", "1")).is_err());
    let mut events = healthy(1);
    events.push(event(
        3,
        10,
        101,
        Body::Eligible {
            borrower: borrower(1),
            actionable: true,
        },
    ));
    assert!(parse_jsonl(&to_jsonl(&header(), &events)).is_err());
    events.pop();
    events.push(event(
        3,
        10,
        101,
        Body::GapClosed {
            from_block: 100,
            to_block: 101,
        },
    ));
    assert!(parse_jsonl(&to_jsonl(&header(), &events)).is_err());
    events.pop();
    events.push(event(
        3,
        10,
        99,
        Body::BorrowerEvent {
            borrower: borrower(1),
        },
    ));
    assert!(parse_jsonl(&to_jsonl(&header(), &events)).is_err());
    events.pop();
    events.push(event(
        2,
        10,
        101,
        Body::BorrowerEvent {
            borrower: borrower(1),
        },
    ));
    assert!(parse_jsonl(&to_jsonl(&header(), &events)).is_err());
    let mut h = header();
    h.duration_ns = 0;
    assert!(parse_jsonl(&to_jsonl(&h, &healthy(1))).is_err());
}

#[test]
fn wad_boundary_zero_debt_and_maximum_are_exact() {
    for (hf, debt, eligible) in [
        (WAD, U256::from(1), false),
        (WAD - U256::from(1), U256::from(1), true),
        (WAD + U256::from(1), U256::from(1), false),
        (U256::MAX, U256::ZERO, false),
        (U256::ZERO, U256::ZERO, false),
    ] {
        let mut events = healthy(1);
        if let Body::State {
            health_factor_wad,
            total_debt_base,
            ..
        } = &mut events[1].body
        {
            *health_factor_wad = hf;
            *total_debt_base = debt;
        }
        if eligible {
            events.push(event(
                3,
                0,
                100,
                Body::Eligible {
                    borrower: borrower(1),
                    actionable: false,
                },
            ));
        }
        let r = replay(&stream(events), &config(), "test").unwrap();
        assert_eq!(r.ordered_checks[0].eligible, eligible);
    }
}

proptest! {
    #[test]
    fn input_permutation_and_repetition_produce_identical_bytes(order in prop::collection::vec(any::<u64>(), 8)) {
        let mut events = vec![];
        for n in 1..=4 {
            events.push(event(u64::from(n) * 2, 0, 100, Body::Register { borrower: borrower(n) }));
            events.push(event(u64::from(n) * 2 + 1, 0, 100, state(n, WAD * U256::from(2))));
        }
        let original = stream(events.clone());
        let mut indexed: Vec<_> = events.drain(..).enumerate().collect();
        indexed.sort_by_key(|(i, _)| order[*i]);
        let shuffled = stream(indexed.into_iter().map(|(_, e)| e).collect());
        prop_assert_eq!(original.digest, shuffled.digest);
        let a = serde_json::to_vec(&replay(&original, &config(), "test").unwrap()).unwrap();
        let b = serde_json::to_vec(&replay(&shuffled, &config(), "test").unwrap()).unwrap();
        prop_assert_eq!(a, b);
    }
}
