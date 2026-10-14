//! Deterministic mandatory scheduler for liquidation rechecks (JEV-LIQ-005).
//!
//! Layering: a leaf crate (alloy primitives, `mev-liq-state` types, serde). `schedule` is pure:
//! no I/O, no clock reads, no randomness, and it never reads advisory decisions. Callers supply
//! `now_ns` and the inputs (see `load` for the repository reader and `shadow` for the shadow pass).

mod advisory;
mod load;
mod policy;
mod shadow;
mod types;

pub use advisory::*;
pub use load::{LoadBounds, LoadedInputs, cohort_of, load_inputs, load_inputs_bounded};
pub use policy::{
    BaselinePolicy, BonusFn, MAX_ADVISORY_BONUS, RankingPolicy, schedule, schedule_with_advisory,
    sweep_capacity,
};
pub use shadow::{
    ShadowCounters, ShadowError, ShadowParams, ShadowReport, run_shadow_pass,
    run_shadow_pass_advisory,
};
pub use types::*;
