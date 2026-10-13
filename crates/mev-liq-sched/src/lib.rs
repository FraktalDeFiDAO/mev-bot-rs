//! Deterministic mandatory scheduler for liquidation rechecks (JEV-LIQ-005).
//!
//! Layering: a leaf crate (alloy primitives, `mev-liq-state` types, serde). `schedule` is pure:
//! no I/O, no clock reads, no randomness, and it never reads advisory decisions. Callers supply
//! `now_ns` and the inputs (see `load` for the repository reader and `shadow` for the shadow pass).

mod load;
mod policy;
mod shadow;
mod types;

pub use load::{LoadedInputs, cohort_of, load_inputs};
pub use policy::{BaselinePolicy, RankingPolicy, schedule};
pub use shadow::{ShadowCounters, ShadowError, ShadowParams, ShadowReport, run_shadow_pass};
pub use types::*;
