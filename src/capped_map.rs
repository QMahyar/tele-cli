use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

#[derive(Debug)]
pub struct CappedMap<K, V> {
    entries: HashMap<K, Slot<V>>,
    order: VecDeque<OrderEntry<K>>,
    cap: usize,
    next_seq: u64,
}

#[derive(Debug)]
struct Slot<V> {
    value: V,
    seq: u64,
}

#[derive(Debug)]
struct OrderEntry<K> {
    seq: u64,
    key: K,
}

impl<K: Eq + Hash + Clone, V> CappedMap<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(cap),
            order: VecDeque::new(),
            cap,
            next_seq: 0,
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|slot| &slot.value)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.entries.get_mut(key).map(|slot| &mut slot.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        let seq = self.claim_seq();
        if let Some(slot) = self.entries.get_mut(&key) {
            slot.value = value;
            slot.seq = seq;
        } else {
            if self.entries.len() >= self.cap {
                self.evict_oldest();
            }
            self.entries.insert(key.clone(), Slot { value, seq });
        }
        self.order.push_back(OrderEntry { seq, key });
        self.compact_if_needed();
    }

    fn claim_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    fn evict_oldest(&mut self) {
        while let Some(entry) = self.order.pop_front() {
            let live = self
                .entries
                .get(&entry.key)
                .is_some_and(|slot| slot.seq == entry.seq);
            if live {
                self.entries.remove(&entry.key);
                break;
            }
        }
    }

    fn compact_if_needed(&mut self) {
        let bound = self.cap.saturating_mul(2).max(64);
        if self.order.len() <= bound {
            return;
        }
        let mut live: Vec<(u64, K)> = self
            .entries
            .iter()
            .map(|(key, slot)| (slot.seq, key.clone()))
            .collect();
        live.sort_by_key(|(seq, _)| *seq);
        self.order = live
            .into_iter()
            .map(|(seq, key)| OrderEntry { seq, key })
            .collect();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.cap
    }
}

impl<K: Eq + Hash + Clone> CappedMap<K, ()> {
    pub fn check(&mut self, key: K) -> bool {
        // Re-observation refreshes the eviction position, mirroring
        // insert's refresh semantics.
        let seq = self.claim_seq();
        if let Some(slot) = self.entries.get_mut(&key) {
            slot.seq = seq;
            self.order.push_back(OrderEntry { seq, key });
            self.compact_if_needed();
            return true;
        }
        if self.entries.len() >= self.cap {
            self.evict_oldest();
        }
        self.entries.insert(key.clone(), Slot { value: (), seq });
        self.order.push_back(OrderEntry { seq, key });
        self.compact_if_needed();
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

    #[test]
    #[ignore]
    fn timing_redelivery_storm() {
        let cap = 10_000usize;
        let mut m: CappedMap<i32, ()> = CappedMap::new(cap);
        for i in 0..cap as i32 {
            m.check(i);
        }
        let start = std::time::Instant::now();
        let mut dups = 0u32;
        for round in 0..5 {
            for i in 0..cap as i32 {
                if m.check((i + round) % cap as i32) {
                    dups += 1;
                }
            }
        }
        let elapsed = start.elapsed();
        println!(
            "capped_map redelivery storm: 50k checks at cap 10k took {elapsed:?} ({dups} dups)"
        );
    }
}
