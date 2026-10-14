//! A manually advanced clock. `sleep` completes only when the test advances time past its
//! deadline, so no real timer ever races a real network exchange.
#![allow(dead_code)]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jev_advisor::{Clock, SleepPurpose};
use tokio::sync::{Notify, watch};

struct Inner {
    now: Mutex<Duration>,
    tx: watch::Sender<Duration>,
    log: Mutex<Vec<(SleepPurpose, Duration)>>,
    pending: Mutex<Vec<(u64, SleepPurpose, Duration)>>,
    next_id: Mutex<u64>,
    registered: Notify,
}

#[derive(Clone)]
pub struct ManualClock(Arc<Inner>);

impl ManualClock {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(Duration::ZERO);
        Self(Arc::new(Inner {
            now: Mutex::new(Duration::ZERO),
            tx,
            log: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
            registered: Notify::new(),
        }))
    }

    pub fn advance(&self, d: Duration) {
        let n = {
            let mut g = self.0.now.lock().unwrap();
            *g += d;
            *g
        };
        self.0.tx.send_replace(n);
    }

    /// Requested durations of every sleep of `purpose`, in request order.
    pub fn requested(&self, purpose: SleepPurpose) -> Vec<Duration> {
        self.0
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| *p == purpose)
            .map(|(_, d)| *d)
            .collect()
    }

    fn earliest(&self, purpose: SleepPurpose) -> Option<Duration> {
        self.0
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p, _)| *p == purpose)
            .map(|(_, _, d)| *d)
            .min()
    }

    /// Resolves once a sleep of `purpose` is pending; returns its absolute deadline.
    pub async fn pending(&self, purpose: SleepPurpose) -> Duration {
        loop {
            if let Some(d) = self.earliest(purpose) {
                return d;
            }
            self.0.registered.notified().await;
        }
    }

    /// Run `fut`, advancing the clock to each backoff sleep as the call reaches it.
    pub async fn drive<F: Future>(&self, fut: F) -> F::Output {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                biased;
                r = &mut fut => return r,
                dl = self.pending(SleepPurpose::Backoff) => {
                    let now = self.now();
                    if dl > now {
                        self.advance(dl - now);
                    }
                    // Let the sleeper observe the new time and deregister.
                    tokio::task::yield_now().await;
                }
            }
        }
    }
}

struct Deregister(Arc<Inner>, u64);
impl Drop for Deregister {
    fn drop(&mut self) {
        self.0
            .pending
            .lock()
            .unwrap()
            .retain(|(id, _, _)| *id != self.1);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self.0.now.lock().unwrap()
    }

    fn sleep(&self, d: Duration, purpose: SleepPurpose) -> impl Future<Output = ()> + Send {
        let inner = self.0.clone();
        async move {
            let deadline = *inner.now.lock().unwrap() + d;
            inner.log.lock().unwrap().push((purpose, d));
            let id = {
                let mut n = inner.next_id.lock().unwrap();
                *n += 1;
                *n
            };
            inner.pending.lock().unwrap().push((id, purpose, deadline));
            inner.registered.notify_waiters();
            inner.registered.notify_one();
            let _guard = Deregister(inner.clone(), id);
            let mut rx = inner.tx.subscribe();
            loop {
                if *inner.now.lock().unwrap() >= deadline {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        }
    }
}
