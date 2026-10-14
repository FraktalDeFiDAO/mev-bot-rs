//! Bounded TTL + LRU map (pure: time is an argument). Used for the answer cache and the short
//! negative cache.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::Duration;

#[derive(Debug)]
struct Entry<V> {
    value: V,
    expires: Duration,
    last_used: u64,
}

#[derive(Debug)]
pub struct TtlLru<K, V> {
    cap: usize,
    ttl: Duration,
    tick: u64,
    map: HashMap<K, Entry<V>>,
}

impl<K: Hash + Eq + Clone, V: Clone> TtlLru<K, V> {
    /// `cap == 0` or `ttl == 0` disables the cache (every `get` misses, `insert` is a no-op).
    pub fn new(cap: usize, ttl: Duration) -> Self {
        Self {
            cap,
            ttl,
            tick: 0,
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, now: Duration, key: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        match self.map.get_mut(key) {
            Some(e) if now < e.expires => {
                e.last_used = tick;
                Some(e.value.clone())
            }
            Some(_) => {
                self.map.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn insert(&mut self, now: Duration, key: K, value: V) {
        if self.cap == 0 || self.ttl.is_zero() {
            return;
        }
        self.tick += 1;
        if !self.map.contains_key(&key) && self.map.len() >= self.cap {
            self.map.retain(|_, e| now < e.expires);
            if self.map.len() >= self.cap
                && let Some(victim) = self
                    .map
                    .iter()
                    .min_by_key(|(_, e)| e.last_used)
                    .map(|(k, _)| k.clone())
            {
                self.map.remove(&victim);
            }
        }
        self.map.insert(
            key,
            Entry {
                value,
                expires: now.saturating_add(self.ttl),
                last_used: self.tick,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn hit_within_ttl_miss_at_ttl() {
        let mut c = TtlLru::new(4, s(60));
        c.insert(s(0), 1u8, "a");
        assert_eq!(c.get(s(59), &1), Some("a"));
        assert_eq!(c.get(s(60), &1), None);
        assert!(c.is_empty());
    }

    #[test]
    fn evicts_least_recently_used_when_full() {
        let mut c = TtlLru::new(2, s(60));
        c.insert(s(0), 1u8, "a");
        c.insert(s(0), 2, "b");
        assert_eq!(c.get(s(1), &1), Some("a")); // 2 is now the LRU
        c.insert(s(1), 3, "c");
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(s(2), &2), None);
        assert_eq!(c.get(s(2), &1), Some("a"));
        assert_eq!(c.get(s(2), &3), Some("c"));
    }

    #[test]
    fn expired_entries_are_dropped_before_live_ones() {
        let mut c = TtlLru::new(2, s(10));
        c.insert(s(0), 1u8, "old");
        c.insert(s(8), 2, "live");
        c.insert(s(12), 3, "new");
        assert_eq!(c.get(s(12), &2), Some("live"));
        assert_eq!(c.get(s(12), &3), Some("new"));
    }

    #[test]
    fn zero_ttl_or_cap_disables() {
        let mut c = TtlLru::new(4, Duration::ZERO);
        c.insert(s(0), 1u8, "a");
        assert_eq!(c.get(s(0), &1), None);
        let mut c = TtlLru::new(0, s(5));
        c.insert(s(0), 1u8, "a");
        assert_eq!(c.get(s(0), &1), None);
    }
}
