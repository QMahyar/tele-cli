use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub struct CappedMap<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    cap: usize,
}

impl<K: Eq + Hash + Clone, V> CappedMap<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.map.get_mut(key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(slot) = self.map.get_mut(&key) {
            *slot = value;
            // Refresh the eviction position: a redelivered/updated key is
            // "seen again" and must not be the next evicted (a stale-FIFO
            // here would re-emit redelivered updates as new once the original
            // entry rotates out).
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
        } else {
            if self.map.len() >= self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.map.remove(&oldest);
                }
            }
            self.map.insert(key.clone(), value);
            self.order.push_back(key);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[cfg(test)]
    pub fn is_full(&self) -> bool {
        self.map.len() >= self.cap
    }
}

impl<K: Eq + Hash + Clone> CappedMap<K, ()> {
    pub fn check(&mut self, key: K) -> bool {
        if self.contains(&key) {
            // Re-observation refreshes the eviction position, mirroring
            // insert's refresh semantics.
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
                self.order.push_back(key);
            }
            return true;
        }
        self.insert(key, ());
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_tracks_size_and_capacity() {
        let mut m: CappedMap<i32, ()> = CappedMap::new(3);
        assert!(m.is_empty());
        assert!(!m.is_full());
        m.insert(1, ());
        m.insert(2, ());
        assert_eq!(m.len(), 2);
        assert!(!m.is_full());
        m.insert(3, ());
        assert_eq!(m.len(), 3);
        assert!(m.is_full());
    }

    #[test]
    fn insert_evicts_oldest_at_capacity() {
        let mut m: CappedMap<i32, i32> = CappedMap::new(2);
        m.insert(1, 10);
        m.insert(2, 20);
        m.insert(3, 30);
        assert_eq!(m.len(), 2);
        assert!(!m.contains(&1));
        assert_eq!(m.get(&2), Some(&20));
        assert_eq!(m.get(&3), Some(&30));
    }

    #[test]
    fn insert_refreshes_eviction_position() {
        let mut m: CappedMap<i32, i32> = CappedMap::new(2);
        m.insert(1, 10);
        m.insert(2, 20);
        m.insert(1, 11);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&1), Some(&11));
        m.insert(3, 30);
        assert_eq!(m.len(), 2);
        // key 1 was re-inserted (refreshed), so key 2 is the oldest and must
        // be evicted instead — a stale-FIFO here would evict the refreshed
        // entry and re-emit a redelivered update as new.
        assert!(!m.contains(&2), "unrefreshed oldest key evicted");
        assert!(m.contains(&1));
        assert!(m.contains(&3));
    }

    #[test]
    fn check_refreshes_eviction_position() {
        let mut m: CappedMap<i32, ()> = CappedMap::new(2);
        m.check(1);
        m.check(2);
        assert!(m.check(1), "redelivery is a duplicate");
        assert!(!m.check(3), "key 2 (oldest, unrefreshed) is evicted");
        assert!(m.contains(&1));
        assert!(m.contains(&3));
        assert!(!m.contains(&2));
    }

    #[test]
    fn check_reports_first_insert_false_then_true() {
        let mut m: CappedMap<&str, ()> = CappedMap::new(4);
        assert!(!m.check("a"));
        assert!(m.check("a"));
        assert!(!m.check("b"));
        assert!(m.check("b"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn check_evicts_oldest_at_capacity() {
        let mut m: CappedMap<i32, ()> = CappedMap::new(2);
        m.check(1);
        m.check(2);
        assert!(!m.check(3));
        assert_eq!(m.len(), 2);
        assert!(m.contains(&2));
        assert!(m.contains(&3));
        assert!(!m.contains(&1));
    }

    #[test]
    fn get_mut_updates_value() {
        let mut m: CappedMap<i32, i32> = CappedMap::new(2);
        m.insert(1, 10);
        if let Some(v) = m.get_mut(&1) {
            *v = 42;
        }
        assert_eq!(m.get(&1), Some(&42));
    }
}
