//! The replay event stream (JSONL): format, strict loader and ground truth.
//!
//! One header line, then one event per line. Ordering is `(t_ns, seq)`; file order is irrelevant, so
//! shuffled same-timestamp input loads identically. The loader rejects anything it does not
//! understand (unknown fields, bad addresses, out-of-range blocks, labels without matching state).

use std::collections::BTreeMap;

use alloy::primitives::{Address, B256, U256, keccak256};
use mev_liq_state::WAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_EVENTS: usize = 100_000;

/// serde_json::Value alone would silently keep the last duplicate object key.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueValue, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueValue, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element::<UniqueValue>()? {
                    out.push(v.0);
                }
                Ok(UniqueValue(Value::Array(out)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueValue, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some((k, v)) = a.next_entry::<String, UniqueValue>()? {
                    if out.insert(k.clone(), v.0).is_some() {
                        return Err(serde::de::Error::custom(format!("duplicate key `{k}`")));
                    }
                }
                Ok(UniqueValue(Value::Object(out)))
            }
        }
        d.deserialize_any(Visitor)
    }
}

mod dec {
    use alloy::primitives::U256;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &U256, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<U256, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom(format!(
                "not a decimal integer: {s:?}"
            )));
        }
        U256::from_str_radix(&s, 10).map_err(serde::de::Error::custom)
    }
}

/// Dataset-level facts. `synthetic` must be true for generated data; reports repeat it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub format_version: u32,
    pub dataset_id: String,
    pub synthetic: bool,
    pub scenario: String,
    /// Free text: where the data came from and how time was derived.
    pub provenance: String,
    pub chain_id: u64,
    pub pool: Address,
    pub source_block_start: u64,
    pub source_block_end: u64,
    /// Simulated length of the replay; the scheduler ticks from 0 to this (inclusive).
    pub duration_ns: u64,
    /// Episodes starting before this are warm-up and excluded from metrics.
    pub measure_from_ns: u64,
    pub seed: Option<u64>,
}

/// What happened. State/Eligible are ground truth (never shown to the policy except through a
/// completed recheck); the rest are observable triggers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Body {
    /// Borrower becomes tracked (marked `Discovered`).
    Register { borrower: Address },
    /// A borrower-touching pool event (supply/borrow/repay/withdraw/collateral toggle/e-mode).
    BorrowerEvent { borrower: Address },
    /// Reserve parameters or rates changed.
    ReserveChanged { asset: Address },
    /// An oracle price for `asset` moved (or its source changed).
    OracleMove { asset: Address, move_bps: u32 },
    /// The true on-chain account state from this time on (what a recheck reads).
    State {
        borrower: Address,
        #[serde(with = "dec")]
        health_factor_wad: U256,
        #[serde(with = "dec")]
        total_debt_base: U256,
        #[serde(with = "dec")]
        total_collateral_base: U256,
        reserve_asset: Option<Address>,
    },
    /// Ground-truth label: an eligible episode starts now (must coincide with an eligible `State`).
    Eligible { borrower: Address, actionable: bool },
    /// Chain reorganisation back to `ancestor_block`.
    Reorg { ancestor_block: u64 },
    /// Logs for these blocks were not obtained.
    Gap { from_block: u64, to_block: u64 },
    /// Backfill of a previously opened gap completed.
    GapClosed { from_block: u64, to_block: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub t_ns: u64,
    pub block: u64,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("replay stream: {}{message}", .line.map_or(String::new(), |l| format!("line {l}: ")))]
pub struct LoadError {
    pub line: Option<usize>,
    pub message: String,
}

fn err<T>(line: usize, m: impl Into<String>) -> Result<T, LoadError> {
    Err(LoadError {
        line: Some(line),
        message: m.into(),
    })
}

fn gerr<T>(m: impl Into<String>) -> Result<T, LoadError> {
    Err(LoadError {
        line: None,
        message: m.into(),
    })
}

/// A validated stream: events sorted by `(t_ns, seq)`.
#[derive(Debug, Clone)]
pub struct ReplayStream {
    pub header: Header,
    pub events: Vec<Event>,
    /// keccak of the canonical JSONL (identical for any file order).
    pub digest: B256,
}

/// Canonical serialisation: header, then events sorted by `(t_ns, seq)`.
pub fn to_jsonl(header: &Header, events: &[Event]) -> String {
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| (e.t_ns, e.seq));
    let mut out = String::new();
    let mut h = serde_json::to_value(header).expect("header contains JSON-compatible fields");
    if let Value::Object(m) = &mut h {
        let mut with_kind = serde_json::Map::new();
        with_kind.insert("kind".into(), Value::String("Header".into()));
        with_kind.extend(std::mem::take(m));
        h = Value::Object(with_kind);
    }
    out.push_str(&h.to_string());
    out.push('\n');
    for e in sorted {
        out.push_str(&serde_json::to_string(e).expect("event contains JSON-compatible fields"));
        out.push('\n');
    }
    out
}

/// Parse and validate a JSONL stream.
pub fn parse_jsonl(text: &str) -> Result<ReplayStream, LoadError> {
    if text.len() > MAX_INPUT_BYTES {
        return gerr("input exceeds 16 MiB limit");
    }
    let mut header: Option<Header> = None;
    let mut events: Vec<(usize, Event)> = vec![];
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() {
            return err(n, "blank line");
        }
        let v = serde_json::from_str::<UniqueValue>(line)
            .or_else(|e| err(n, format!("bad JSON: {e}")))?
            .0;
        let Value::Object(mut m) = v else {
            return err(n, "line is not a JSON object");
        };
        let Some(kind) = m.get("kind").and_then(Value::as_str).map(str::to_owned) else {
            return err(n, "missing string field `kind`");
        };
        if kind == "Header" {
            if n != 1 {
                return err(n, "header must be the first line");
            }
            m.remove("kind");
            header = Some(
                serde_json::from_value(Value::Object(m))
                    .or_else(|e| err(n, format!("bad header: {e}")))?,
            );
            continue;
        }
        if header.is_none() {
            return err(n, "event before header");
        }
        let ev: Event = serde_json::from_value(Value::Object(m.clone()))
            .or_else(|e| err(n, format!("bad event: {e}")))?;
        let back = serde_json::to_value(&ev).or_else(|e| err(n, e.to_string()))?;
        if let Value::Object(b) = back {
            for k in m.keys() {
                if !b.contains_key(k) {
                    return err(n, format!("unknown field `{k}` for kind {kind}"));
                }
            }
        }
        events.push((n, ev));
        if events.len() > MAX_EVENTS {
            return err(n, "event count exceeds 100000 limit");
        }
    }
    let Some(header) = header else {
        return gerr("missing header line");
    };
    validate(header, events)
}

pub fn load_file(path: &std::path::Path) -> Result<ReplayStream, LoadError> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| {
            f.take((MAX_INPUT_BYTES + 1) as u64)
                .read_to_string(&mut text)
        })
        .or_else(|e| gerr(format!("{}: {e}", path.display())))?;
    parse_jsonl(&text)
}

/// Validate header + events (any input order) and return the sorted stream.
pub fn validate(
    header: Header,
    mut events: Vec<(usize, Event)>,
) -> Result<ReplayStream, LoadError> {
    if events.len() > MAX_EVENTS {
        return gerr("event count exceeds 100000 limit");
    }
    if header.format_version != FORMAT_VERSION {
        return gerr(format!(
            "unsupported format_version {} (want {FORMAT_VERSION})",
            header.format_version
        ));
    }
    if header.dataset_id.is_empty() || header.scenario.is_empty() || header.provenance.is_empty() {
        return gerr("dataset_id, scenario and provenance must be non-empty");
    }
    if header.chain_id != 42161 {
        return gerr("chain_id must be Arbitrum One (42161)");
    }
    if header.source_block_start > header.source_block_end {
        return gerr("source_block_start > source_block_end");
    }
    if header.duration_ns == 0 || header.measure_from_ns > header.duration_ns {
        return gerr("duration_ns must be > 0 and >= measure_from_ns");
    }
    if header.pool == Address::ZERO {
        return gerr("pool must be non-zero");
    }
    events.sort_by_key(|(_, e)| (e.t_ns, e.seq));
    for w in events.windows(2) {
        if w[0].1.seq == w[1].1.seq {
            return err(w[1].0, format!("duplicate seq {}", w[1].1.seq));
        }
    }
    // seq uniqueness across non-adjacent (sorted by t first): check globally.
    let mut seen = std::collections::BTreeSet::new();
    for (n, e) in &events {
        if !seen.insert(e.seq) {
            return err(*n, format!("duplicate seq {}", e.seq));
        }
    }
    let mut floor = header.source_block_start;
    let mut prev_reorg = false;
    let mut open_gaps: Vec<(u64, u64)> = vec![];
    let mut registered = std::collections::BTreeSet::new();
    for (n, e) in &events {
        if e.seq > i64::MAX as u64 || e.block > i64::MAX as u64 {
            return err(*n, "seq/block exceeds supported integer range");
        }
        if e.t_ns > header.duration_ns {
            return err(*n, "t_ns beyond header duration_ns");
        }
        if e.block < header.source_block_start || e.block > header.source_block_end {
            return err(*n, "block outside header source block range");
        }
        if e.block < floor && !prev_reorg {
            return err(*n, "block went backwards without a Reorg");
        }
        if e.block < floor {
            return err(*n, "block is below the reorg ancestor");
        }
        prev_reorg = false;
        floor = e.block;
        match &e.body {
            Body::Register { borrower } => {
                if *borrower == Address::ZERO {
                    return err(*n, "borrower must be non-zero");
                }
                registered.insert(*borrower);
            }
            Body::State {
                borrower,
                reserve_asset,
                ..
            } => {
                if !registered.contains(borrower) {
                    return err(*n, "State for an unregistered borrower");
                }
                if *reserve_asset == Some(Address::ZERO) {
                    return err(*n, "reserve_asset must be non-zero or null");
                }
            }
            Body::BorrowerEvent { borrower } | Body::Eligible { borrower, .. } => {
                if !registered.contains(borrower) {
                    return err(*n, "event for an unregistered borrower");
                }
            }
            Body::Reorg { ancestor_block } => {
                if *ancestor_block < header.source_block_start || *ancestor_block > e.block {
                    return err(*n, "reorg ancestor outside [source_block_start, block]");
                }
                floor = *ancestor_block;
                prev_reorg = true;
            }
            Body::Gap {
                from_block,
                to_block,
            } => {
                if from_block > to_block
                    || *from_block < header.source_block_start
                    || *to_block > e.block
                    || open_gaps.contains(&(*from_block, *to_block))
                {
                    return err(*n, "invalid, future or duplicate gap range");
                }
                open_gaps.push((*from_block, *to_block));
            }
            Body::GapClosed {
                from_block,
                to_block,
            } => {
                let Some(i) = open_gaps
                    .iter()
                    .position(|g| *g == (*from_block, *to_block))
                else {
                    return err(*n, "GapClosed without a matching open Gap");
                };
                open_gaps.remove(i);
            }
            Body::OracleMove { move_bps, .. } if *move_bps > 100_000 => {
                return err(*n, "move_bps out of range (> 100000)");
            }
            _ => {}
        }
    }
    let evs: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
    let lines: BTreeMap<u64, usize> = events.iter().map(|(n, e)| (e.seq, *n)).collect();
    GroundTruth::build(&evs).map_err(|(seq, m)| LoadError {
        line: lines.get(&seq).copied(),
        message: m,
    })?;
    let digest = keccak256(to_jsonl(&header, &evs).as_bytes());
    Ok(ReplayStream {
        header,
        events: evs,
        digest,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruthPoint {
    pub t_ns: u64,
    pub block: u64,
    pub available: bool,
    pub health_factor_wad: U256,
    pub total_debt_base: U256,
    pub total_collateral_base: U256,
    pub reserve_asset: Option<Address>,
}

impl TruthPoint {
    pub fn eligible(&self) -> bool {
        self.available && self.health_factor_wad < WAD && !self.total_debt_base.is_zero()
    }
}

/// A labelled eligible interval `[start_ns, end_ns)`; `end_ns` None = still open at stream end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub borrower: Address,
    pub block: u64,
    pub start_ns: u64,
    pub end_ns: Option<u64>,
    pub actionable: bool,
    pub invalidated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GroundTruth {
    pub states: BTreeMap<Address, Vec<TruthPoint>>,
    pub episodes: Vec<Episode>,
}

impl GroundTruth {
    /// Build from events sorted by `(t_ns, seq)`; `Err((seq, message))` on an inconsistent label.
    pub fn build(events: &[Event]) -> Result<Self, (u64, String)> {
        let mut gt = Self::default();
        let mut open: BTreeMap<Address, usize> = BTreeMap::new();
        let mut unlabeled: BTreeMap<Address, (u64, u64)> = BTreeMap::new();
        let mut canonical: BTreeMap<Address, Vec<TruthPoint>> = BTreeMap::new();
        for e in events {
            match &e.body {
                Body::State {
                    borrower,
                    health_factor_wad,
                    total_debt_base,
                    total_collateral_base,
                    reserve_asset,
                } => {
                    let p = TruthPoint {
                        t_ns: e.t_ns,
                        block: e.block,
                        available: true,
                        health_factor_wad: *health_factor_wad,
                        total_debt_base: *total_debt_base,
                        total_collateral_base: *total_collateral_base,
                        reserve_asset: *reserve_asset,
                    };
                    let eligible = p.eligible();
                    canonical.entry(*borrower).or_default().push(p.clone());
                    gt.states.entry(*borrower).or_default().push(p);
                    if eligible {
                        if !open.contains_key(borrower)
                            && unlabeled
                                .insert(*borrower, (e.t_ns, e.seq))
                                .is_some_and(|(t, _)| t != e.t_ns)
                        {
                            return Err((e.seq, "eligible State missing its episode label".into()));
                        }
                    } else {
                        if let Some((t, seq)) = unlabeled.remove(borrower)
                            && t != e.t_ns
                        {
                            return Err((
                                seq,
                                "eligible State never labelled with Eligible".into(),
                            ));
                        }
                        if let Some(i) = open.remove(borrower) {
                            gt.episodes[i].end_ns = Some(e.t_ns);
                        }
                    }
                }
                Body::Eligible {
                    borrower,
                    actionable,
                } => {
                    if open.contains_key(borrower) {
                        return Err((e.seq, "Eligible label inside an open episode".into()));
                    }
                    match unlabeled.remove(borrower) {
                        Some((t, _)) if t == e.t_ns => {}
                        _ => {
                            return Err((
                                e.seq,
                                "Eligible label without an eligible State at the same t_ns".into(),
                            ));
                        }
                    }
                    open.insert(*borrower, gt.episodes.len());
                    gt.episodes.push(Episode {
                        borrower: *borrower,
                        block: e.block,
                        start_ns: e.t_ns,
                        end_ns: None,
                        actionable: *actionable,
                        invalidated: false,
                    });
                }
                Body::Reorg { ancestor_block } => {
                    if let Some((_, (_, seq))) = unlabeled.iter().next() {
                        return Err((*seq, "eligible State never labelled with Eligible".into()));
                    }
                    for (_, i) in std::mem::take(&mut open) {
                        gt.episodes[i].end_ns = Some(e.t_ns);
                    }
                    for episode in &mut gt.episodes {
                        if episode.block > *ancestor_block {
                            episode.invalidated = true;
                        }
                    }
                    for (borrower, history) in &mut canonical {
                        history.retain(|p| p.block <= *ancestor_block);
                        let mut restored = history.last().cloned().unwrap_or(TruthPoint {
                            t_ns: e.t_ns,
                            block: *ancestor_block,
                            available: false,
                            health_factor_wad: U256::ZERO,
                            total_debt_base: U256::ZERO,
                            total_collateral_base: U256::ZERO,
                            reserve_asset: None,
                        });
                        restored.t_ns = e.t_ns;
                        // An eligible restored state needs an explicit as-of episode label.
                        if restored.eligible() {
                            unlabeled.insert(*borrower, (e.t_ns, e.seq));
                        }
                        gt.states.entry(*borrower).or_default().push(restored);
                    }
                }
                _ => {}
            }
        }
        if let Some((_, (_, seq))) = unlabeled.into_iter().next() {
            return Err((seq, "eligible State never labelled with Eligible".into()));
        }
        Ok(gt)
    }

    /// Latest state with `t_ns <= t`.
    pub fn at(&self, borrower: &Address, t: u64) -> Option<&TruthPoint> {
        let v = self.states.get(borrower)?;
        let i = v.partition_point(|p| p.t_ns <= t);
        i.checked_sub(1).map(|i| &v[i]).filter(|p| p.available)
    }
}
