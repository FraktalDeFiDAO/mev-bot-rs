//! Constant-space ordering guard for the canonical log subscription.
use alloy::{primitives::B256, rpc::types::Log};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invalid {
    Removed,
    MissingIdentity,
    ConflictingBlock,
    Reordered,
}

#[derive(Default)]
pub(crate) struct Continuity {
    snapshot: Option<(u64, B256)>,
    last: Option<(u64, B256, u64)>,
}

impl Continuity {
    pub fn reconciled(&mut self, number: u64, hash: B256) {
        self.snapshot = Some((number, hash));
        self.last = None;
    }

    /// False means already covered by the snapshot or an exact replay of the last log.
    /// Every invalidation must trigger a fresh subscribe + pinned snapshot before trading.
    pub fn accept(&mut self, log: &Log) -> Result<bool, Invalid> {
        if log.removed {
            return Err(Invalid::Removed);
        }
        let (Some(number), Some(hash), Some(index)) =
            (log.block_number, log.block_hash, log.log_index)
        else {
            return Err(Invalid::MissingIdentity);
        };
        if let Some((floor, anchor)) = self.snapshot {
            if number == floor && hash != anchor {
                return Err(Invalid::ConflictingBlock);
            }
            if number <= floor {
                return Ok(false);
            }
        }
        if let Some((prior, prior_hash, prior_index)) = self.last {
            if number == prior && hash != prior_hash {
                return Err(Invalid::ConflictingBlock);
            }
            if number < prior || (number == prior && index < prior_index) {
                return Err(Invalid::Reordered);
            }
            if number == prior && index == prior_index {
                return Ok(false);
            }
        }
        self.last = Some((number, hash, index));
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn log(block: u64, hash: u8, index: u64) -> Log {
        Log {
            block_number: Some(block),
            block_hash: Some(B256::repeat_byte(hash)),
            log_index: Some(index),
            ..Default::default()
        }
    }
    #[test]
    fn removed_logs_are_invalid_even_when_covered_by_snapshot() {
        let mut c = Continuity::default();
        c.reconciled(100, B256::repeat_byte(1));
        let mut old = log(99, 1, 0);
        old.removed = true;
        assert_eq!(c.accept(&old), Err(Invalid::Removed));
        assert_eq!(c.accept(&log(100, 2, 0)), Err(Invalid::ConflictingBlock));
        assert_eq!(c.accept(&log(100, 1, 0)), Ok(false));
    }
    #[test]
    fn duplicates_and_reordered_or_conflicting_blocks_cannot_change_state() {
        let mut c = Continuity::default();
        assert_eq!(c.accept(&log(101, 1, 4)), Ok(true));
        assert_eq!(c.accept(&log(101, 1, 4)), Ok(false));
        assert_eq!(c.accept(&log(101, 1, 5)), Ok(true));
        assert_eq!(c.accept(&log(101, 1, 3)), Err(Invalid::Reordered));
        assert_eq!(c.accept(&log(101, 2, 6)), Err(Invalid::ConflictingBlock));
        assert_eq!(c.accept(&log(102, 2, 0)), Ok(true));
        assert_eq!(c.accept(&log(101, 1, 6)), Err(Invalid::Reordered));
        assert_eq!(c.accept(&Log::default()), Err(Invalid::MissingIdentity));
    }
}
