//! Private hash maps that reserve every bucket table through package funding.

use std::borrow::Borrow;
use std::hash::{Hash, RandomState};

use crate::metadata_owned::MapAllocator;

type Inner<K, V> = hashbrown::HashMap<K, V, RandomState, MapAllocator>;

pub(crate) struct OwnedHashMap<K, V> {
    inner: Inner<K, V>,
}

pub(crate) struct SealedHashMap<K, V> {
    inner: Inner<K, V>,
}

impl<K, V> OwnedHashMap<K, V> {
    pub(crate) fn new(allocator: MapAllocator) -> Self {
        Self {
            inner: Inner::with_hasher_in(RandomState::new(), allocator),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    pub(crate) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.inner.contains_key(key)
    }

    pub(crate) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.inner.get(key)
    }

    #[cfg(test)]
    pub(crate) fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.inner.get_key_value(key)
    }

    pub(crate) fn try_reserve(
        &mut self,
        additional: usize,
    ) -> Result<(), hashbrown::TryReserveError>
    where
        K: Eq + Hash,
    {
        self.inner.try_reserve(additional)
    }

    pub(crate) fn insert_reserved(&mut self, key: K, value: V) -> Option<V>
    where
        K: Eq + Hash,
    {
        assert!(
            self.inner.contains_key(&key) || self.inner.len() < self.inner.capacity(),
            "map insertion needs admitted capacity"
        );
        self.inner.insert(key, value)
    }

    pub(crate) fn seal(self) -> SealedHashMap<K, V> {
        self.inner.allocator().seal();
        SealedHashMap { inner: self.inner }
    }
}

impl<K, V> SealedHashMap<K, V> {
    pub(crate) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.inner.contains_key(key)
    }

    pub(crate) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Eq + Hash + ?Sized,
    {
        self.inner.get(key)
    }
}
