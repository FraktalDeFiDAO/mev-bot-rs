//! seqscope core: filter and decode Arbitrum sequencer transactions with user-written rules.
//!
//! - [`tx::Tx`]: a transaction as plain data (from the live feed, a file or a test).
//! - [`expr`]: the rule language, as text and as data.
//! - [`compile`]: type-checks a rule against the field model and evaluates it.
//! - [`field`]: every field a rule can read (transaction, call, decoded swap / liquidity / liquidation).
//! - [`engine`]: the rule file, the compiled [`engine::Engine`], and the [`engine::Row`]s it produces.
//! - [`catalogue`]: fields, protocols, contracts and events as data, for building UIs.
//!
//! Calldata decoding is an intent: the transaction may revert, and the free feed has no receipts. Rows say `pending`.

pub mod catalogue;
pub mod compile;
pub mod engine;
pub mod explain;
pub mod expr;
pub mod field;
pub mod names;
pub mod templates;
pub mod tx;
pub mod value;

pub use engine::{
    ActionKind, Engine, Mode, Preview, Profile, Row, Rule, RuleError, RulePatch, RuleSet, preview,
};
pub use tx::Tx;
