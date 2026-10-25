//! An [`EventIndex`] stepped in its own task, so scanning (a back-fill is up to ~100k throttled
//! `getLogs` on Base) never holds up the liquidation runner's loop. Readers get the account set
//! as of the last step that changed it; edits made meanwhile (`insert`, `remove`) apply to that
//! copy at once and to the index before its next step.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use alloy::{providers::DynProvider, rpc::types::Log};
use tokio::sync::mpsc;
use tracing::warn;

use crate::{EventIndex, Key};

/// Pause between steps while history is being back-filled (each step is already throttled).
const BACKFILL_PAUSE: Duration = Duration::from_secs(1);
/// Forward scans once history is complete.
const FORWARD_EVERY: Duration = Duration::from_secs(60);
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(300);

enum Edit {
    Insert(Key),
    Remove(Key),
}

#[derive(Clone)]
struct Published {
    accounts: Arc<BTreeSet<Key>>,
    backfilled: bool,
}

pub struct BackgroundIndex {
    /// The index until `run` moves it into its task.
    idle: Option<EventIndex>,
    published: Arc<Mutex<Published>>,
    edits: Option<mpsc::UnboundedSender<Edit>>,
}

impl BackgroundIndex {
    pub fn new(index: EventIndex) -> Self {
        let published = Published {
            accounts: Arc::new(index.state.accounts.clone()),
            backfilled: index.backfilled(),
        };
        Self {
            idle: Some(index),
            published: Arc::new(Mutex::new(published)),
            edits: None,
        }
    }

    /// The accounts as of the last published step.
    pub fn accounts(&self) -> Arc<BTreeSet<Key>> {
        self.published.lock().unwrap().accounts.clone()
    }

    pub fn len(&self) -> usize {
        self.published.lock().unwrap().accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn backfilled(&self) -> bool {
        self.published.lock().unwrap().backfilled
    }

    pub fn insert(&mut self, key: Key) {
        Arc::make_mut(&mut self.published.lock().unwrap().accounts).insert(key);
        self.edit(Edit::Insert(key));
    }

    /// Drop an account; a later scan that finds it again brings it back.
    pub fn remove(&mut self, key: &Key) -> bool {
        let had = Arc::make_mut(&mut self.published.lock().unwrap().accounts).remove(key);
        self.edit(Edit::Remove(*key));
        had
    }

    fn edit(&mut self, e: Edit) {
        match (&mut self.idle, &self.edits) {
            (Some(idx), _) => match e {
                Edit::Insert(k) => idx.insert(k),
                Edit::Remove(k) => {
                    idx.remove(&k);
                }
            },
            (None, Some(tx)) => {
                let _ = tx.send(e);
            }
            (None, None) => {}
        }
    }

    /// Start scanning in the background on the first call (`extract` maps a log to accounts);
    /// every call returns at once whether history is complete. Never waits on the RPC.
    pub fn run(
        &mut self,
        p: &DynProvider,
        extract: impl Fn(&Log) -> Vec<Key> + Send + Sync + 'static,
    ) -> bool {
        if let Some(idx) = self.idle.take() {
            let (tx, rx) = mpsc::unbounded_channel();
            self.edits = Some(tx);
            tokio::spawn(work(idx, p.clone(), extract, self.published.clone(), rx));
        }
        self.backfilled()
    }
}

async fn work(
    mut idx: EventIndex,
    p: DynProvider,
    extract: impl Fn(&Log) -> Vec<Key>,
    published: Arc<Mutex<Published>>,
    mut edits: mpsc::UnboundedReceiver<Edit>,
) {
    let mut retry = RETRY_MIN;
    let mut was_done = idx.backfilled();
    loop {
        if edits.is_closed() {
            return; // the adapter is gone
        }
        let mut changed = false;
        while let Ok(e) = edits.try_recv() {
            match e {
                Edit::Insert(k) => idx.insert(k),
                Edit::Remove(k) => {
                    idx.remove(&k);
                }
            }
            changed = true;
        }
        let before = idx.len();
        match idx.step(&p, &extract).await {
            Ok(done) => {
                retry = RETRY_MIN;
                if changed || idx.len() != before || done != was_done {
                    *published.lock().unwrap() = Published {
                        accounts: Arc::new(idx.state.accounts.clone()),
                        backfilled: done,
                    };
                }
                was_done = done;
                tokio::time::sleep(if done { FORWARD_EVERY } else { BACKFILL_PAUSE }).await;
            }
            Err(e) => {
                warn!(index = idx.cfg.name, error = %format!("{e:#}"), retry_in_s = retry.as_secs(), "index failed");
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(RETRY_MAX);
            }
        }
    }
}
