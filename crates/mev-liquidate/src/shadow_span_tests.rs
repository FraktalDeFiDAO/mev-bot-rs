//! The shadow pass span carries chain, pass id, block and revision, and never an address (spec section 11).

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use alloy::primitives::Address;
use mev_liq_sched::{PolicyConfig, ShadowParams, run_shadow_pass};
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::SubscriberExt};

use super::{pass_span, record_pass_span};

type Spans = Arc<Mutex<Vec<(String, BTreeMap<String, String>)>>>;

struct Capture(Spans);

struct Fields<'a>(&'a mut BTreeMap<String, String>);
impl Visit for Fields<'_> {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().to_string(), format!("{v:?}"));
    }
}

impl<S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S>
    for Capture
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut m = BTreeMap::new();
        attrs.record(&mut Fields(&mut m));
        self.0
            .lock()
            .unwrap()
            .push((attrs.metadata().name().to_string(), m));
    }
    fn on_record(
        &self,
        _id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut m = BTreeMap::new();
        values.record(&mut Fields(&mut m));
        if let Some(last) = self.0.lock().unwrap().last_mut() {
            last.1.extend(m);
        }
    }
}

#[test]
fn pass_span_carries_chain_pass_block_and_revision_and_no_address() {
    let dir = tempfile::tempdir().unwrap();
    let repo = mev_liq_state::SnapshotRepository::open(&dir.path().join("s.db")).unwrap();
    let pool = Address::repeat_byte(0xaa);
    let borrower = Address::with_last_byte(9);
    repo.register_borrowers(42161, pool, &[borrower], None, 1)
        .unwrap();
    let params = ShadowParams {
        chain_id: 42161,
        pool,
        scan_name: "live".into(),
        policy: PolicyConfig::default(),
        max_records: 50,
        bounds: None,
    };
    let report = run_shadow_pass(&repo, &params, 1_000_000_000).unwrap();
    assert!(!report.jobs.is_empty(), "the fixture must schedule a job");

    let spans: Spans = Arc::default();
    let sub = tracing_subscriber::registry().with(Capture(spans.clone()));
    tracing::subscriber::with_default(sub, || {
        let span = pass_span(params.chain_id, 1_000_000_000);
        record_pass_span(&span, &report);
    });
    let spans = spans.lock().unwrap();
    let (name, f) = &spans[0];
    assert_eq!(name, "liq_shadow_pass");
    assert_eq!(f["chain_id"], "42161");
    assert_eq!(f["pass_id"], "1000000000");
    assert!(f.contains_key("revision"), "{f:?}");
    assert_eq!(f["jobs"], report.counters.dispatched.to_string());
    let all = format!("{f:?}").to_ascii_lowercase();
    for a in [format!("{pool:x}"), format!("{borrower:x}")] {
        assert!(!all.contains(&a), "span must not carry addresses: {all}");
    }
}
