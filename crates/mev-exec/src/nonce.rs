//! Nonce allocation for concurrent sends. A nonce is `take`n before signing and either stays in
//! use (the tx was accepted somewhere) or is `release`d (no endpoint took it). A released nonce is
//! handed out again before any new one, so a failed send never leaves a gap that would stall the
//! transactions queued behind it, and never makes the next send collide with a pending one.

use std::{collections::BTreeSet, sync::Mutex};

#[derive(Debug)]
struct State {
    /// Next never-used nonce.
    next: u64,
    /// Released nonces below `next`, lowest handed out first.
    free: BTreeSet<u64>,
}

#[derive(Debug)]
pub struct NonceManager(Mutex<State>);

impl NonceManager {
    pub fn new(start: u64) -> Self {
        Self(Mutex::new(State {
            next: start,
            free: BTreeSet::new(),
        }))
    }

    /// A nonce nobody else holds: the lowest released one, else a fresh one.
    pub fn take(&self) -> u64 {
        let mut s = self.0.lock().unwrap();
        if let Some(n) = s.free.pop_first() {
            return n;
        }
        let n = s.next;
        s.next += 1;
        n
    }

    /// `n` was never accepted by any endpoint: make it available again.
    pub fn release(&self, n: u64) {
        let mut s = self.0.lock().unwrap();
        if n >= s.next {
            return;
        }
        s.free.insert(n);
        // Hand the top of the range back instead of leaving it as a "free" hole.
        while s.next > 0 {
            let top = s.next - 1;
            if !s.free.remove(&top) {
                break;
            }
            s.next = top;
        }
    }

    /// The chain disagrees with us (stale nonce, or a tx was dropped): continue from its pending
    /// count and forget every released nonce.
    pub fn reset(&self, chain_pending: u64) {
        let mut s = self.0.lock().unwrap();
        s.next = chain_pending;
        s.free.clear();
    }

    #[cfg(test)]
    fn snapshot(&self) -> (u64, Vec<u64>) {
        let s = self.0.lock().unwrap();
        (s.next, s.free.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, sync::Arc};

    #[test]
    fn hands_out_consecutive_nonces() {
        let m = NonceManager::new(7);
        assert_eq!((m.take(), m.take(), m.take()), (7, 8, 9));
    }

    #[test]
    fn releasing_the_latest_nonce_just_rewinds() {
        let m = NonceManager::new(0);
        let (_a, b) = (m.take(), m.take());
        m.release(b);
        assert_eq!(m.snapshot(), (1, vec![]));
        assert_eq!(m.take(), 1);
    }

    #[test]
    fn releasing_an_earlier_nonce_is_reused_before_a_new_one_so_no_collision_no_gap() {
        // The old fetch_sub bug: A=7 fails after B=8 was taken. The counter fell to 8 and the
        // next send reused 8 (colliding with B). Now the next send takes 7 and B stays 8.
        let m = NonceManager::new(7);
        let (a, b) = (m.take(), m.take());
        assert_eq!((a, b), (7, 8));
        m.release(a);
        assert_eq!(m.take(), 7, "the hole is refilled first");
        assert_eq!(m.take(), 9, "and nothing reuses B's nonce");
    }

    #[test]
    fn releasing_out_of_order_collapses_back_to_a_clean_range() {
        let m = NonceManager::new(0);
        let n: Vec<u64> = (0..4).map(|_| m.take()).collect();
        m.release(n[1]);
        m.release(n[3]);
        m.release(n[2]);
        assert_eq!(m.snapshot(), (1, vec![]));
    }

    #[test]
    fn reset_follows_the_chain_and_forgets_released_nonces() {
        let m = NonceManager::new(0);
        let _ = (m.take(), m.take(), m.take());
        m.release(0);
        m.reset(10);
        assert_eq!(m.snapshot(), (10, vec![]));
        assert_eq!(m.take(), 10);
    }

    #[test]
    fn releasing_a_nonce_that_was_never_handed_out_is_ignored() {
        let m = NonceManager::new(3);
        m.release(50);
        assert_eq!(m.snapshot(), (3, vec![]));
    }

    #[test]
    fn concurrent_sends_with_failures_never_share_a_live_nonce_or_leave_a_gap() {
        let m = Arc::new(NonceManager::new(100));
        let live = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let threads: Vec<_> = (0..8u64)
            .map(|t| {
                let (m, live) = (m.clone(), live.clone());
                std::thread::spawn(move || {
                    for i in 0..400u64 {
                        let n = m.take();
                        assert!(live.lock().unwrap().insert(n), "nonce {n} handed out twice");
                        // Roughly a third of the sends fail on every endpoint.
                        if (t * 7 + i * 13) % 3 == 0 {
                            assert!(live.lock().unwrap().remove(&n));
                            m.release(n);
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // What is still in use plus what is free must be exactly the contiguous range [100, next).
        let (next, free) = m.snapshot();
        let mut all: Vec<u64> = live.lock().unwrap().iter().copied().chain(free).collect();
        all.sort_unstable();
        assert_eq!(all, (100..next).collect::<Vec<_>>());
    }
}
