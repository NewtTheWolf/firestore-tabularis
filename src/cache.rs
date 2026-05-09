//! Bounded cache with TTL eviction. Used for COUNT_CACHE and CURSOR_CACHE.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

pub struct TtlLruCache<K: Hash + Eq + Clone, V> {
    capacity: usize,
    ttl: Duration,
    entries: HashMap<K, Entry<V>>,
    /// Insertion order for LRU eviction. Front = oldest.
    order: std::collections::VecDeque<K>,
}

struct Entry<V> {
    value: V,
    inserted_at: Instant,
}

impl<K: Hash + Eq + Clone, V> TtlLruCache<K, V> {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl,
            entries: HashMap::with_capacity(capacity),
            order: std::collections::VecDeque::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        // Single lookup branch: if expired, evict and report miss; else borrow
        // the value directly. The previous double-`get` was a defensive
        // workaround for a borrow-checker problem that doesn't exist here.
        let expired = matches!(self.entries.get(key), Some(e) if e.inserted_at.elapsed() > self.ttl);
        if expired {
            self.entries.remove(key);
            self.order.retain(|k| k != key);
            return None;
        }
        self.entries.get(key).map(|entry| &entry.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        // If key already present, refresh in place (don't grow order).
        if self.entries.contains_key(&key) {
            self.entries.insert(
                key.clone(),
                Entry {
                    value,
                    inserted_at: Instant::now(),
                },
            );
            return;
        }
        // Evict oldest while at capacity.
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            Entry {
                value,
                inserted_at: Instant::now(),
            },
        );
    }

    #[cfg(test)]
    pub fn remove(&mut self, key: &K) {
        self.entries.remove(key);
        self.order.retain(|k| k != key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_miss_on_empty_cache() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        assert!(c.get(&"x".to_string()).is_none());
    }

    #[test]
    fn inserted_value_is_retrievable() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert("x".to_string(), 42);
        assert_eq!(c.get(&"x".to_string()), Some(&42));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn expired_entry_returns_miss() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_millis(10));
        c.insert("x".to_string(), 1);
        std::thread::sleep(Duration::from_millis(20));
        assert!(c.get(&"x".to_string()).is_none());
    }

    #[test]
    fn lru_eviction_on_capacity() {
        let mut c: TtlLruCache<u64, &str> = TtlLruCache::new(2, Duration::from_secs(60));
        c.insert(1, "a");
        c.insert(2, "b");
        c.insert(3, "c"); // evicts key 1
        assert!(c.get(&1).is_none());
        assert_eq!(c.get(&2), Some(&"b"));
        assert_eq!(c.get(&3), Some(&"c"));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn reinsert_refreshes_in_place() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(2, Duration::from_secs(60));
        c.insert(1, 100);
        c.insert(1, 200);
        assert_eq!(c.get(&1), Some(&200));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn explicit_remove_clears_entry() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert(1, 100);
        c.remove(&1);
        assert!(c.get(&1).is_none());
    }

    #[test]
    fn clear_drops_all_entries() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert(1, 100);
        c.insert(2, 200);
        c.clear();
        assert_eq!(c.len(), 0);
    }
}
