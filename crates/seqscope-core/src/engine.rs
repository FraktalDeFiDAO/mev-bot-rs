//! Rule sets (the file format), the compiled engine, and the rows it produces.
//!
//! Semantics, so a rule file reads the same everywhere:
//! - Every enabled **profile** is evaluated independently against every **call** of every transaction (a call is the
//!   transaction itself or one leaf inside a multicall, Multicall3, smart account or ERC-4337 bundle).
//! - Inside a profile, rules run in order. `mode = "all"` (default) applies every matching rule; `mode = "first"`
//!   stops at the first match.
//! - A matching rule's **action**: `store` (a row without calldata), `raw` (a row with the calldata), `alert` (a row
//!   marked for notification), `count` (only the counter), `drop` (stop this profile for this call, store nothing).
//! - `sample = N` keeps one match in N, chosen by the transaction hash, so the same input always gives the same output.
//! - A comparison on a field the call does not have (`swap.amount_in` on a liquidation) is false, including `!=`.
//!   Use `set(field)` to test that a field exists.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

use alloy::primitives::B256;
use mev_decode::{
    calls::{Leaf, actions_in},
    known_contract,
};
use serde::{Deserialize, Serialize};

use crate::{
    catalogue::protocol_of,
    compile::{Node, compile},
    expr::parse,
    field::{Ctx, FieldId},
    tx::Tx,
};

/// The current rule-file format version.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    First,
    #[default]
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    #[default]
    Store,
    Raw,
    Count,
    Drop,
    Alert,
}

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The rule text (see `expr`).
    pub when: String,
    #[serde(default, skip_serializing_if = "is_store")]
    pub action: ActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Keep one match in N.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub sample: u32,
}

fn is_true(b: &bool) -> bool {
    *b
}
fn is_store(a: &ActionKind) -> bool {
    *a == ActionKind::Store
}
fn is_one(n: &u32) -> bool {
    *n == 1
}
fn is_all(m: &Mode) -> bool {
    *m == Mode::All
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_all")]
    pub mode: Mode,
    #[serde(default)]
    pub rule: Vec<Rule>,
}

/// Fields to change on a rule; `None` leaves a field alone. An empty `tag` removes the tag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulePatch {
    pub name: Option<String>,
    pub when: Option<String>,
    pub action: Option<ActionKind>,
    pub tag: Option<String>,
    pub sample: Option<u32>,
    pub enabled: Option<bool>,
}

/// A rule's text must parse and type-check, or it is refused before anything changes.
fn check_when(when: &str) -> Result<(), String> {
    let expr = parse(when).map_err(|e| format!("the rule does not parse {e}"))?;
    compile(&expr).map(|_| ()).map_err(|e| e.0)
}

/// The rule file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    pub version: u32,
    #[serde(default)]
    pub profile: Vec<Profile>,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            profile: Vec::new(),
        }
    }
}

impl RuleSet {
    fn profile_mut(&mut self, name: &str) -> Result<&mut Profile, String> {
        self.profile
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("there is no profile '{name}'"))
    }

    /// Add a rule to a profile (the profile is created if it does not exist). Refused when the name is empty or taken
    /// in that profile, or the rule's text is wrong; nothing changes then.
    pub fn add_rule(&mut self, profile: &str, rule: Rule) -> Result<(), String> {
        if rule.name.trim().is_empty() {
            return Err("a rule needs a name".into());
        }
        if rule.sample == 0 {
            return Err("sample must be at least 1".into());
        }
        check_when(&rule.when)?;
        if let Some(p) = self.profile.iter().find(|p| p.name == profile)
            && p.rule.iter().any(|r| r.name == rule.name)
        {
            return Err(format!(
                "there is already a rule named '{}' in '{profile}'",
                rule.name
            ));
        }
        match self.profile.iter_mut().find(|p| p.name == profile) {
            Some(p) => p.rule.push(rule),
            None => self.profile.push(Profile {
                name: profile.to_string(),
                enabled: true,
                mode: Mode::All,
                rule: vec![rule],
            }),
        }
        Ok(())
    }

    /// Change some fields of a rule. The new name must not clash; the new text must be valid.
    pub fn update_rule(
        &mut self,
        profile: &str,
        rule: &str,
        patch: &RulePatch,
    ) -> Result<(), String> {
        if let Some(w) = &patch.when {
            check_when(w)?;
        }
        if patch.sample == Some(0) {
            return Err("sample must be at least 1".into());
        }
        let p = self.profile_mut(profile)?;
        if let Some(n) = &patch.name {
            if n.trim().is_empty() {
                return Err("a rule needs a name".into());
            }
            if n != rule && p.rule.iter().any(|r| &r.name == n) {
                return Err(format!(
                    "there is already a rule named '{n}' in '{profile}'"
                ));
            }
        }
        let r = p
            .rule
            .iter_mut()
            .find(|r| r.name == rule)
            .ok_or_else(|| format!("there is no rule '{rule}' in '{profile}'"))?;
        if let Some(n) = &patch.name {
            r.name = n.clone();
        }
        if let Some(w) = &patch.when {
            r.when = w.clone();
        }
        if let Some(a) = patch.action {
            r.action = a;
        }
        if let Some(t) = &patch.tag {
            r.tag = (!t.trim().is_empty()).then(|| t.trim().to_string());
        }
        if let Some(n) = patch.sample {
            r.sample = n;
        }
        if let Some(e) = patch.enabled {
            r.enabled = e;
        }
        Ok(())
    }

    pub fn set_rule_enabled(
        &mut self,
        profile: &str,
        rule: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.update_rule(
            profile,
            rule,
            &RulePatch {
                enabled: Some(enabled),
                ..Default::default()
            },
        )
    }

    /// Remove a rule (an emptied profile is removed too, so the file does not collect empty shells).
    pub fn remove_rule(&mut self, profile: &str, rule: &str) -> Result<Rule, String> {
        let p = self.profile_mut(profile)?;
        let i = p
            .rule
            .iter()
            .position(|r| r.name == rule)
            .ok_or_else(|| format!("there is no rule '{rule}' in '{profile}'"))?;
        let removed = p.rule.remove(i);
        self.profile.retain(|p| !p.rule.is_empty());
        Ok(removed)
    }

    /// Add what another rule set has that this one does not (matched by profile and rule name); returns how many rules
    /// were added and how many were already there.
    pub fn merge(&mut self, other: &RuleSet) -> (usize, usize) {
        let (mut added, mut skipped) = (0, 0);
        for p in &other.profile {
            for r in &p.rule {
                match self.profile.iter().find(|q| q.name == p.name) {
                    Some(q) if q.rule.iter().any(|x| x.name == r.name) => skipped += 1,
                    Some(_) => {
                        let q = self
                            .profile
                            .iter_mut()
                            .find(|q| q.name == p.name)
                            .expect("just found");
                        q.rule.push(r.clone());
                        added += 1;
                    }
                    None => {
                        self.profile.push(Profile {
                            name: p.name.clone(),
                            enabled: p.enabled,
                            mode: p.mode,
                            rule: vec![r.clone()],
                        });
                        added += 1;
                    }
                }
            }
        }
        (added, skipped)
    }

    /// One profile with one `store` rule: a rule typed on its own (the CLI's `--when`, a UI probe).
    pub fn single(when: &str) -> RuleSet {
        RuleSet {
            version: FORMAT_VERSION,
            profile: vec![Profile {
                name: "probe".into(),
                enabled: true,
                mode: Mode::All,
                rule: vec![Rule {
                    name: "when".into(),
                    enabled: true,
                    when: when.into(),
                    action: ActionKind::Store,
                    tag: None,
                    sample: 1,
                }],
            }],
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let set: RuleSet = toml::from_str(text).map_err(|e| e.to_string())?;
        if set.version != FORMAT_VERSION {
            return Err(format!(
                "rule file version {} is not supported (this build reads version {FORMAT_VERSION})",
                set.version
            ));
        }
        Ok(set)
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }
}

/// A problem in a rule, located so a UI can point at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleError {
    pub profile: String,
    pub rule: String,
    pub message: String,
    /// 1-based column in the rule text, for syntax errors.
    pub column: Option<usize>,
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "profile '{}', rule '{}': {}",
            self.profile, self.rule, self.message
        )?;
        if let Some(c) = self.column {
            write!(f, " (column {c})")?;
        }
        Ok(())
    }
}

impl Row {
    /// A hash from stored bytes (zeros if the length is wrong, which a database written by this crate never has).
    pub fn hash_from(b: &[u8]) -> B256 {
        if b.len() == 32 {
            B256::from_slice(b)
        } else {
            B256::ZERO
        }
    }
}

/// One stored record: a call that matched a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub t_ms: u64,
    pub seq: u64,
    pub hash: B256,
    pub from: Option<String>,
    pub tx_to: Option<String>,
    pub to: Option<String>,
    pub selector: Option<String>,
    pub via: Vec<String>,
    pub protocol: Option<String>,
    pub known: Option<String>,
    pub kind: String,
    /// Always `pending`: calldata is an intent until a chain log confirms it.
    pub status: String,
    pub fields: BTreeMap<String, String>,
    pub profile: String,
    pub rule: String,
    pub tag: Option<String>,
    pub alert: bool,
    pub len: usize,
    /// Present for `raw` rules only.
    pub input: Option<String>,
}

struct CRule {
    name: String,
    node: Node,
    action: ActionKind,
    tag: Option<String>,
    sample: u32,
    matches: AtomicU64,
}

struct CProfile {
    name: String,
    mode: Mode,
    rules: Vec<CRule>,
}

/// Counters per rule, for the stats screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleStats {
    pub profile: String,
    pub rule: String,
    pub matches: u64,
}

/// A compiled rule set.
pub struct Engine {
    profiles: Vec<CProfile>,
    calls: AtomicU64,
}

impl Engine {
    /// Compile every enabled rule; all problems are reported, not only the first.
    pub fn compile(set: &RuleSet) -> Result<Engine, Vec<RuleError>> {
        let mut errors = Vec::new();
        let mut profiles = Vec::new();
        for p in set.profile.iter().filter(|p| p.enabled) {
            let mut rules = Vec::new();
            for r in p.rule.iter().filter(|r| r.enabled) {
                let fail = |message: String, column| RuleError {
                    profile: p.name.clone(),
                    rule: r.name.clone(),
                    message,
                    column,
                };
                if r.sample == 0 {
                    errors.push(fail(
                        "sample must be at least 1 (1 keeps every match)".into(),
                        None,
                    ));
                    continue;
                }
                match parse(&r.when) {
                    Err(e) => errors.push(fail(e.message, Some(e.pos))),
                    Ok(expr) => match compile(&expr) {
                        Err(e) => errors.push(fail(e.0, None)),
                        Ok(node) => rules.push(CRule {
                            name: r.name.clone(),
                            node,
                            action: r.action,
                            tag: r.tag.clone(),
                            sample: r.sample,
                            matches: AtomicU64::new(0),
                        }),
                    },
                }
            }
            profiles.push(CProfile {
                name: p.name.clone(),
                mode: p.mode,
                rules,
            });
        }
        if errors.is_empty() {
            Ok(Engine {
                profiles,
                calls: AtomicU64::new(0),
            })
        } else {
            Err(errors)
        }
    }

    /// Calls examined so far.
    pub fn calls_seen(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> Vec<RuleStats> {
        self.profiles
            .iter()
            .flat_map(|p| {
                p.rules.iter().map(|r| RuleStats {
                    profile: p.name.clone(),
                    rule: r.name.clone(),
                    matches: r.matches.load(Ordering::Relaxed),
                })
            })
            .collect()
    }

    /// Every row the transaction produces.
    pub fn process(&self, tx: &Tx) -> Vec<Row> {
        let mut rows = Vec::new();
        let leaves: Vec<(Option<Leaf>, Option<mev_decode::Decoded>)> = match tx.to {
            Some(to) => actions_in(to, &tx.input)
                .into_iter()
                .map(|a| (Some(a.leaf), a.decoded))
                .collect(),
            None => vec![(None, None)],
        };
        for (leaf, decoded) in &leaves {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let known = leaf.as_ref().and_then(|l| known_contract(l.to));
            let protocol = match decoded {
                Some(mev_decode::Decoded::Liquidity(l)) => Some(l.protocol),
                Some(mev_decode::Decoded::Liquidation(l)) => Some(l.protocol),
                _ => leaf.as_ref().and_then(|l| protocol_of(l.to)),
            };
            let ctx = Ctx {
                tx,
                leaf: leaf.as_ref(),
                decoded: decoded.as_ref(),
                protocol,
                known,
            };
            for p in &self.profiles {
                for r in &p.rules {
                    if !r.node.eval(&ctx) {
                        continue;
                    }
                    if r.sample > 1
                        && u64::from_be_bytes(tx.hash[..8].try_into().unwrap())
                            % u64::from(r.sample)
                            != 0
                    {
                        continue;
                    }
                    r.matches.fetch_add(1, Ordering::Relaxed);
                    match r.action {
                        ActionKind::Count => {}
                        ActionKind::Drop => break,
                        ActionKind::Store | ActionKind::Raw | ActionKind::Alert => {
                            rows.push(make_row(&ctx, &p.name, r));
                        }
                    }
                    if p.mode == Mode::First {
                        break;
                    }
                }
            }
        }
        rows
    }
}

fn make_row(c: &Ctx<'_>, profile: &str, r: &CRule) -> Row {
    let fields: BTreeMap<String, String> = FieldId::ALL
        .iter()
        .filter(|f| matches!(f.group(), "swap" | "liquidity" | "liquidation"))
        .filter_map(|f| f.read(c).map(|v| (f.name().to_string(), v.render())))
        .collect();
    Row {
        t_ms: c.tx.t_ms,
        seq: c.tx.seq,
        hash: c.tx.hash,
        from: c.tx.from.map(|a| format!("{a:#x}")),
        tx_to: c.tx.to.map(|a| format!("{a:#x}")),
        to: c.leaf.map(|l| format!("{:#x}", l.to)),
        selector: c
            .leaf
            .and_then(|l| l.selector())
            .map(|s| format!("0x{}", alloy::hex::encode(s))),
        via: c
            .leaf
            .map(|l| l.via.iter().map(|v| (*v).to_string()).collect())
            .unwrap_or_default(),
        protocol: c.protocol.map(str::to_string),
        known: c.known.map(|k| k.name.to_string()),
        kind: c.kind().to_string(),
        status: "pending".into(),
        fields,
        profile: profile.to_string(),
        rule: r.name.clone(),
        tag: r.tag.clone(),
        alert: r.action == ActionKind::Alert,
        len: c.leaf.map_or(0, |l| l.input.len()),
        input: (r.action == ActionKind::Raw)
            .then(|| {
                c.leaf
                    .map(|l| format!("0x{}", alloy::hex::encode(&l.input)))
            })
            .flatten(),
    }
}

/// What a rule text would match over some transactions: counts and a few examples, without saving anything.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub transactions: u64,
    pub calls: u64,
    pub matched: u64,
    pub samples: Vec<Row>,
}

pub fn preview(when: &str, txs: &[Tx], max_samples: usize) -> Result<Preview, RuleError> {
    let set = RuleSet {
        version: FORMAT_VERSION,
        profile: vec![Profile {
            name: "preview".into(),
            enabled: true,
            mode: Mode::All,
            rule: vec![Rule {
                name: "preview".into(),
                enabled: true,
                when: when.into(),
                action: ActionKind::Store,
                tag: None,
                sample: 1,
            }],
        }],
    };
    let engine = Engine::compile(&set).map_err(|mut e| e.remove(0))?;
    let mut samples = Vec::new();
    let mut matched = 0;
    for tx in txs {
        let rows = engine.process(tx);
        matched += rows.len() as u64;
        for r in rows {
            if samples.len() < max_samples {
                samples.push(r);
            }
        }
    }
    Ok(Preview {
        transactions: txs.len() as u64,
        calls: engine.calls_seen(),
        matched,
        samples,
    })
}
