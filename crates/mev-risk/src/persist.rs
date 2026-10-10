//! Tiny JSON snapshot file: atomic write, tolerant read. Used so spend caps survive a restart.

use std::path::PathBuf;

use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `None` when the file is missing or unreadable (a corrupt file must not stop the bot).
    pub fn load<T: DeserializeOwned>(&self) -> Option<T> {
        let bytes = std::fs::read(&self.path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(path = %self.path.display(), error = %e, "risk state unreadable, starting fresh");
                None
            }
        }
    }

    /// Write via a temp file and rename, so a crash mid-write leaves the old snapshot.
    pub fn save<T: Serialize>(&self, value: &T) {
        let tmp = self.path.with_extension("tmp");
        let result = serde_json::to_vec(value)
            .map_err(std::io::Error::other)
            .and_then(|b| std::fs::write(&tmp, b))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = result {
            warn!(path = %self.path.display(), error = %e, "risk state not saved");
        }
    }
}
