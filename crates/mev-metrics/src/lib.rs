//! Event log (JSONL, one file per day) and running counters for PnL and funnel stats, plus the
//! Prometheus text exposition (`prom`), the scrape-time `registry` and the optional loopback
//! `/metrics` listener (`server`). The exposition part has no dependency on the event log so a
//! signer-free process (the Jev advisor) can use it with `default-features = false`.

pub mod prom;
pub mod registry;
pub mod server;
pub mod textcheck;

#[cfg(feature = "jsonl")]
mod jsonl;
#[cfg(feature = "jsonl")]
pub use jsonl::{Counters, Metrics};

pub use prom::{Exposition, Kind};
pub use registry::Registry;
pub use server::{ListenAddrError, ServerHandle, addr_from_env, parse_listen_addr};
