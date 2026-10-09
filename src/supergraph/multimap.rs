use std::collections::BTreeMap;

/// Read-only map from a key to a slice of values, stored as three flat vectors
/// (sorted keys, offsets, values) instead of one heap allocation per key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiMap<K, V> {
    keys: Vec<K>,
    /// `offsets[i]..offsets[i + 1]` is the value range of `keys[i]`.
    offsets: Vec<u32>,
    values: Vec<V>,
}

impl<K, V> Default for MultiMap<K, V> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            offsets: vec![0],
            values: Vec::new(),
        }
    }
}

impl<K: Ord, V> MultiMap<K, V> {
    /// Consumes `map`; keys stay in ascending order, each value list keeps its order.
    pub fn from_btree(map: BTreeMap<K, Vec<V>>) -> Self {
        let mut keys = Vec::with_capacity(map.len());
        let mut offsets = Vec::with_capacity(map.len() + 1);
        let mut values = Vec::new();
        offsets.push(0);
        for (key, mut list) in map {
            keys.push(key);
            values.append(&mut list);
            offsets.push(u32::try_from(values.len()).expect("index value count fits u32"));
        }
        values.shrink_to_fit();
        Self { keys, offsets, values }
    }

    pub fn get(&self, key: &K) -> Option<&[V]> {
        let position = self.keys.binary_search(key).ok()?;
        Some(self.slice(position))
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.keys.binary_search(key).is_ok()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.keys.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &[V]> {
        (0..self.keys.len()).map(|position| self.slice(position))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &[V])> {
        self.keys
            .iter()
            .enumerate()
            .map(|(position, key)| (key, self.slice(position)))
    }

    fn slice(&self, position: usize) -> &[V] {
        &self.values[self.offsets[position] as usize..self.offsets[position + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_return_slices_in_key_order() {
        let mut map = BTreeMap::new();
        map.insert(3u32, vec![9u32, 8]);
        map.insert(1, vec![5]);
        map.insert(2, vec![]);
        let compact = MultiMap::from_btree(map);
        assert_eq!(compact.get(&1), Some(&[5][..]));
        assert_eq!(compact.get(&2), Some(&[][..]));
        assert_eq!(compact.get(&3), Some(&[9, 8][..]));
        assert_eq!(compact.get(&4), None);
        assert_eq!(compact.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(compact.values().map(<[u32]>::len).sum::<usize>(), 3);
        assert!(MultiMap::<u32, u32>::default().get(&1).is_none());
    }
}
