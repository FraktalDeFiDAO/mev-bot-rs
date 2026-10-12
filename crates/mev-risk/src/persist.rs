//! Tiny JSON snapshot file: atomic write, tolerant read. Used so spend caps survive a restart.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    healthy: Arc<AtomicBool>,
    _lock: Option<Arc<std::fs::File>>,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        use std::os::unix::fs::OpenOptionsExt;
        let lock = (|| -> std::io::Result<std::fs::File> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(path.with_extension("lock"))?;
            file.try_lock().map_err(std::io::Error::other)?;
            Ok(file)
        })();
        if let Err(e) = &lock {
            warn!(path = %path.display(), error = %e, "accounting ownership unavailable; submissions disabled");
        }
        Self {
            path,
            healthy: Arc::new(AtomicBool::new(lock.is_ok())),
            _lock: lock.ok().map(Arc::new),
        }
    }

    /// Missing files return `None`; read failure latches the store closed for submissions.
    pub fn load<T: DeserializeOwned>(&self) -> Option<T> {
        self.load_checked().unwrap_or_else(|e| {
            warn!(path = %self.path.display(), error = %e, "risk state unreadable; submissions disabled");
            None
        })
    }

    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub fn load_checked<T: DeserializeOwned>(&self) -> anyhow::Result<Option<T>> {
        anyhow::ensure!(self.healthy(), "accounting ownership unavailable");
        let result = match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(anyhow::Error::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        };
        if result.is_err() {
            self.healthy.store(false, Ordering::Release);
        }
        result
    }

    /// Write via a temp file and rename, so a crash mid-write leaves the old snapshot.
    pub fn save<T: Serialize>(&self, value: &T) {
        if let Err(e) = self.save_checked(value) {
            warn!(path = %self.path.display(), error = %e, "risk state not saved");
        }
    }

    pub fn save_checked<T: Serialize>(&self, value: &T) -> anyhow::Result<()> {
        anyhow::ensure!(self.healthy(), "accounting storage requires reconciliation");
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let result = (|| -> anyhow::Result<()> {
            let bytes = serde_json::to_vec(value)?;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)?;
            std::fs::File::open(
                self.path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new(".")),
            )?
            .sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.healthy.store(false, Ordering::Release);
        }
        result
    }
}
