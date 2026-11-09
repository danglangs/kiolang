//! `HashMap<K, V>` (L10, C13): one source for the interpreter and the compiled runtime, which
//! includes this file by path. `core` and `alloc` only, no `unsafe`.
//!
//! Entries sit in a `Vec` in insertion order, which is the iteration order; a removed entry
//! leaves a hole until holes are over half the entries, then the `Vec` is compacted and the
//! index rebuilt for what is left (so a map that was large once costs what it holds now). The
//! index is a power-of-two table of buckets, never fewer than the entries (holes included). A
//! bucket is a chain through the entries' `next` links, or, once a chain would pass `TREEIFY`
//! entries, a `BTreeMap` from key to entry. The hash is fixed and unkeyed (kio has no
//! randomness), so keys can be built to share a bucket; a sorted bucket keeps each operation
//! O(log n) then.
//!
//! The map remembers the entry its last `get` found (`last`), so `m.insert(k, m.get(k)..)`
//! hashes and searches once: `insert` compares its key with that entry's first. `last` is
//! cleared by every operation that moves or removes entries (`remove`, compaction, which growth
//! does too) and a copy starts without it, so it always names a live entry; an insert still
//! checks the key there, so a wrong `last` could cost a search but never a wrong answer.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::Cell;

/// A key: ordered, for sorted buckets, and hashed.
pub trait HashKey: Ord + Clone {
    fn hash(&self) -> u64;
}

/// The longest chain: a bucket that would hold one more entry becomes sorted.
pub const TREEIFY: usize = 8;
/// A bucket with no entries; also ends a chain.
const EMPTY: u32 = u32::MAX;
/// A bucket's head with this bit (and not `EMPTY`) is `TREE | i`, sorted bucket `trees[i]`.
const TREE: u32 = 1 << 31;

/// The hash of an `Int` key: murmur3's 64-bit finalizer, a bijection with every bit mixed.
pub fn hash_int(n: i64) -> u64 {
    fmix(n as u64)
}

/// The hash of a `Str` key's bytes: the length, then each 8 bytes (little-endian, the last
/// zero-padded) mixed in as FxHash does, then `fmix`.
pub fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = b.len() as u64;
    let (words, rest) = b.as_chunks::<8>();
    for &w in words {
        h = mix(h, u64::from_le_bytes(w));
    }
    if !rest.is_empty() {
        let mut w = [0; 8];
        w[..rest.len()].copy_from_slice(rest);
        h = mix(h, u64::from_le_bytes(w));
    }
    fmix(h)
}

/// FxHash's step.
pub fn mix(h: u64, w: u64) -> u64 {
    (h.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95)
}

/// For tests: `stages` lists of `n` distinct 16-byte printable ASCII strings such that every
/// string made of one from each list, in order, has the same `hash_bytes`, so `n^stages` keys
/// share one bucket in any table. Each string is a free word, then the word that brings the
/// state to a fixed value (`mix` is invertible in its second argument), kept if printable.
pub fn colliding_parts(stages: usize, n: usize) -> Vec<Vec<[u8; 16]>> {
    let printable = |w: u64| w.to_le_bytes().iter().all(|b| (0x20..0x7f).contains(b));
    let mut h = 16 * stages as u64;
    let mut free = 0u64;
    let mut out = Vec::new();
    for stage in 0..stages {
        let target = 0x6b69_6f20_6861_7368 ^ stage as u64;
        let mut parts = Vec::new();
        while parts.len() < n {
            free += 1;
            // `free` in base 95, as printable bytes.
            let mut w = [b' '; 8];
            let mut x = free;
            for b in w.iter_mut() {
                *b += (x % 95) as u8;
                x /= 95;
            }
            let w1 = u64::from_le_bytes(w);
            let w2 = mix(h, w1).rotate_left(5) ^ target;
            if printable(w2) {
                let mut part = [0; 16];
                part[..8].copy_from_slice(&w);
                part[8..].copy_from_slice(&w2.to_le_bytes());
                parts.push(part);
            }
        }
        h = mix(
            mix(h, u64::from_le_bytes(parts[0][..8].try_into().unwrap())),
            u64::from_le_bytes(parts[0][8..].try_into().unwrap()),
        );
        out.push(parts);
    }
    out
}

fn fmix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

#[derive(Clone)]
struct Entry<K, V> {
    hash: u64,
    /// The next entry of this entry's chain, or `EMPTY`; unused in a sorted bucket.
    next: u32,
    /// `None` once removed: a hole.
    kv: Option<(K, V)>,
}

pub struct HashMap<K, V> {
    entries: Vec<Entry<K, V>>,
    /// Per bucket: `EMPTY`, the first entry of its chain, or `TREE | i`.
    heads: Vec<u32>,
    trees: Vec<BTreeMap<K, u32>>,
    len: usize,
    /// The entry the last `get` found, or `EMPTY`.
    last: Cell<u32>,
}

/// A copy remembers no entry.
impl<K: Clone, V: Clone> Clone for HashMap<K, V> {
    fn clone(&self) -> Self {
        HashMap {
            entries: self.entries.clone(),
            heads: self.heads.clone(),
            trees: self.trees.clone(),
            len: self.len,
            last: Cell::new(EMPTY),
        }
    }
}

/// The sorted bucket a head names, if it names one.
fn tree(head: u32) -> Option<usize> {
    (head != EMPTY && head & TREE != 0).then_some((head & !TREE) as usize)
}

impl<K: HashKey, V> Default for HashMap<K, V> {
    fn default() -> Self {
        HashMap::new()
    }
}

impl<K: HashKey, V> HashMap<K, V> {
    pub fn new() -> Self {
        HashMap {
            entries: Vec::new(),
            heads: Vec::new(),
            trees: Vec::new(),
            len: 0,
            last: Cell::new(EMPTY),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entries and values in insertion order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> {
        self.entries.iter().filter_map(|e| e.kv.as_ref().map(|(k, v)| (k, v)))
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    /// The value of `k`; the entry is remembered for an `insert` of the same key.
    pub fn get(&self, k: &K) -> Option<&V> {
        let i = self.find(k, k.hash());
        self.last.set(i.map_or(EMPTY, |i| i as u32));
        self.entries[i?].kv.as_ref().map(|(_, v)| v)
    }

    /// Sets `k` to `v`. A key already present keeps its place and its value is returned; a new
    /// one goes last.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        match self.replace(&k, v) {
            Ok(old) => Some(old),
            Err((hash, v)) => {
                self.push(hash, k, v);
                None
            }
        }
    }

    /// `insert` for a key borrowed: `own` makes the key to keep (equal to `k`), called only if
    /// `k` is new.
    pub fn insert_with(&mut self, k: &K, own: impl FnOnce() -> K, v: V) -> Option<V> {
        match self.replace(k, v) {
            Ok(old) => Some(old),
            Err((hash, v)) => {
                let k = own();
                debug_assert!(k.hash() == hash);
                self.push(hash, k, v);
                None
            }
        }
    }

    /// Replaces `k`'s value with `v`, returning the old one; if `k` is absent, gives back its
    /// hash and `v`. Tries the remembered entry first, then hashes and searches once.
    fn replace(&mut self, k: &K, v: V) -> Result<V, (u64, V)> {
        if let Some(e) = self.entries.get_mut(self.last.get() as usize) {
            if let Some((x, old)) = &mut e.kv {
                if x == k {
                    return Ok(core::mem::replace(old, v));
                }
            }
        }
        let hash = k.hash();
        match self.find(k, hash) {
            Some(i) => match &mut self.entries[i].kv {
                Some((_, old)) => Ok(core::mem::replace(old, v)),
                None => unreachable!("find returns live entries"),
            },
            None => Err((hash, v)),
        }
    }

    /// Adds `k`, absent, with hash `hash`, last.
    fn push(&mut self, hash: u64, k: K, v: V) {
        if self.entries.len() == self.heads.len() {
            self.compact();
        }
        assert!(self.entries.len() < (TREE - 1) as usize, "HashMap too large");
        self.entries.push(Entry {
            hash,
            next: EMPTY,
            kv: Some((k, v)),
        });
        self.len += 1;
        self.link(self.entries.len() - 1);
    }

    /// Removes `k`, returning its value if it was present.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.last.set(EMPTY);
        let hash = k.hash();
        let i = self.find(k, hash)?;
        let b = self.bucket(hash);
        match tree(self.heads[b]) {
            Some(t) => {
                self.trees[t].remove(k);
            }
            None => self.unlink(b, i),
        }
        let (_, v) = self.entries[i].kv.take()?;
        self.len -= 1;
        if 2 * (self.entries.len() - self.len) > self.entries.len() {
            self.compact();
        }
        Some(v)
    }

    /// Drops the holes and indexes what is left, with room to double: entries move, so the
    /// remembered one is forgotten.
    fn compact(&mut self) {
        self.last.set(EMPTY);
        self.entries.retain(|e| e.kv.is_some());
        self.rebuild((2 * self.len).next_power_of_two().max(8));
    }

    fn bucket(&self, hash: u64) -> usize {
        hash as usize & (self.heads.len() - 1)
    }

    /// The index of the entry holding `k`, whose hash is `hash`.
    fn find(&self, k: &K, hash: u64) -> Option<usize> {
        if self.heads.is_empty() {
            return None;
        }
        let head = self.heads[self.bucket(hash)];
        if let Some(t) = tree(head) {
            return self.trees[t].get(k).map(|&i| i as usize);
        }
        let mut i = head;
        while i != EMPTY {
            let e = &self.entries[i as usize];
            if e.hash == hash && e.kv.as_ref().is_some_and(|(x, _)| x == k) {
                return Some(i as usize);
            }
            i = e.next;
        }
        None
    }

    /// Adds entry `i` to its bucket: first in its chain, which past `TREEIFY` entries becomes
    /// a sorted bucket.
    fn link(&mut self, i: usize) {
        let b = self.bucket(self.entries[i].hash);
        let head = self.heads[b];
        if let Some(t) = tree(head) {
            let k = self.entries[i].kv.as_ref().expect("a live entry").0.clone();
            self.trees[t].insert(k, i as u32);
            return;
        }
        let mut n = 0;
        let mut j = head;
        while j != EMPTY {
            n += 1;
            j = self.entries[j as usize].next;
        }
        self.entries[i].next = head;
        self.heads[b] = i as u32;
        if n + 1 > TREEIFY {
            let mut sorted = BTreeMap::new();
            let mut j = self.heads[b];
            while j != EMPTY {
                let e = &self.entries[j as usize];
                sorted.insert(e.kv.as_ref().expect("a live entry").0.clone(), j);
                j = e.next;
            }
            self.heads[b] = TREE | self.trees.len() as u32;
            self.trees.push(sorted);
        }
    }

    /// Takes entry `i` out of bucket `b`'s chain.
    fn unlink(&mut self, b: usize, i: usize) {
        let next = self.entries[i].next;
        if self.heads[b] == i as u32 {
            self.heads[b] = next;
            return;
        }
        let mut j = self.heads[b] as usize;
        while self.entries[j].next != i as u32 {
            j = self.entries[j].next as usize;
        }
        self.entries[j].next = next;
    }

    /// A fresh index of `n` buckets (a power of two) over the entries.
    fn rebuild(&mut self, n: usize) {
        self.heads.clear();
        self.heads.resize(n, EMPTY);
        self.trees.clear();
        for i in 0..self.entries.len() {
            if self.entries[i].kv.is_some() {
                self.link(i);
            }
        }
    }

    /// The number of sorted buckets, for tests.
    pub fn sorted_buckets(&self) -> usize {
        self.trees.len()
    }

    /// For tests: `None` if no entry is remembered, else the remembered entry's key, `None` if
    /// it names no live entry.
    pub fn remembered(&self) -> Option<Option<&K>> {
        let i = self.last.get();
        (i != EMPTY).then(|| self.entries.get(i as usize)?.kv.as_ref().map(|(k, _)| k))
    }

    /// The number of buckets, for tests.
    pub fn buckets(&self) -> usize {
        self.heads.len()
    }
}

/// Order-insensitive: the same keys with equal values, as Rust's `HashMap`.
impl<K: HashKey, V: PartialEq> PartialEq for HashMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl<K: HashKey + core::fmt::Debug, V: core::fmt::Debug> core::fmt::Debug for HashMap<K, V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A key whose hash is `self.0 % self.1`: few buckets, long chains, sorted buckets.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct K(u32, u32);

    impl HashKey for K {
        fn hash(&self) -> u64 {
            u64::from(self.0 % self.1)
        }
    }

    /// Random operations against a list of pairs in insertion order, the meaning `HashMap`
    /// promises: inserts, replacements, removes and re-inserts, with spread keys and with keys
    /// in few hash classes (so chains pass `TREEIFY`), checked in full after every step.
    #[test]
    fn matches_an_ordered_list() {
        let rounds = if cfg!(miri) { 300 } else { 20_000 };
        for classes in [1, 3, 1 << 20] {
            let mut seed = 0x2545_f491_4f6c_dd1d_u64 ^ classes;
            let mut rand = move |n: u64| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed % n
            };
            let mut m: HashMap<K, u64> = HashMap::new();
            let mut want: Vec<(K, u64)> = Vec::new();
            for step in 0..rounds {
                let k = K(rand(60) as u32, classes as u32);
                if rand(3) == 0 {
                    let pos = want.iter().position(|(x, _)| *x == k);
                    assert_eq!(m.remove(&k), pos.map(|p| want.remove(p).1));
                } else {
                    let old = match want.iter_mut().find(|(x, _)| *x == k) {
                        Some((_, v)) => Some(core::mem::replace(v, step)),
                        None => {
                            want.push((k, step));
                            None
                        }
                    };
                    assert_eq!(m.insert(k, step), old);
                }
                assert_eq!(m.len(), want.len());
                assert!(m.iter().map(|(k, v)| (*k, *v)).eq(want.iter().copied()));
                for x in 0..60 {
                    let k = K(x, classes as u32);
                    assert_eq!(m.get(&k), want.iter().find(|(y, _)| *y == k).map(|(_, v)| v));
                }
            }
            if classes == 1 {
                assert!(m.sorted_buckets() > 0);
            }
        }
    }

    /// The entry `get` remembers, against a model: random gets, inserts (half on the key last
    /// got), removes and copies, with keys in few hash classes (sorted buckets), and key ranges
    /// that grow and shrink so growth drops holes from before the remembered entry. What the map remembers must be the key the model's last `get`
    /// found, unless something since forgot it; each clear of `last` is needed to pass.
    #[test]
    fn remembered_entry_matches_a_model() {
        let rounds = if cfg!(miri) { 1_500 } else { 50_000 };
        for classes in [1, 3, 1 << 20] {
            let mut seed = 0x9e37_79b9_7f4a_7c15_u64 ^ classes;
            let mut rand = move |n: u64| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed % n
            };
            let key = |x: u64| K(x as u32, classes as u32);
            let mut m: HashMap<K, u64> = HashMap::new();
            let mut want: Vec<(K, u64)> = Vec::new();
            // The key the last `get` found, until an operation that may forget it.
            let mut remembered: Option<K> = None;
            let mut got = key(0);
            // Growths that dropped holes while an entry was remembered; compactions by removes.
            let (mut grew, mut compacted) = (0, 0);
            for step in 0..rounds {
                let span = [8, 60, 300][(step / 500) as usize % 3];
                let k = if rand(2) == 0 { got } else { key(rand(span)) };
                let pos = want.iter().position(|(x, _)| *x == k);
                match rand(8) {
                    0..=2 => {
                        assert_eq!(m.get(&k), pos.map(|p| &want[p].1), "step {step}");
                        remembered = pos.map(|_| k);
                        assert_eq!(m.remembered(), remembered.as_ref().map(Some), "step {step}");
                        got = k;
                    }
                    3..=5 => {
                        let entries = m.entries.len();
                        let old = match pos {
                            Some(p) => Some(core::mem::replace(&mut want[p].1, step)),
                            None => {
                                want.push((k, step));
                                None
                            }
                        };
                        assert_eq!(m.insert(k, step), old, "step {step}");
                        if remembered.is_some() && m.entries.len() < entries + usize::from(old.is_none()) {
                            grew += 1;
                        }
                    }
                    6 => {
                        let entries = m.entries.len();
                        assert_eq!(m.remove(&k), pos.map(|p| want.remove(p).1), "step {step}");
                        remembered = None;
                        if m.entries.len() < entries {
                            compacted += 1;
                        }
                    }
                    _ => {
                        m = m.clone();
                        remembered = None;
                    }
                }
                // Remembered or not, never an entry other than the model's, a hole or past the end.
                let ok = m.remembered().is_none_or(|k| k.is_some() && k == remembered.as_ref());
                assert!(ok, "step {step}");
                assert!(m.iter().map(|(k, v)| (*k, *v)).eq(want.iter().copied()), "step {step}");
            }
            assert!(grew > 0 && compacted > 0, "{grew} growths, {compacted} compactions");
            if classes == 1 {
                assert!(m.sorted_buckets() > 0);
            }
        }
    }

    #[test]
    fn order_after_removes_and_reinserts() {
        let mut m: HashMap<K, i32> = HashMap::new();
        for x in [3, 1, 2] {
            m.insert(K(x, 7), 0);
        }
        m.remove(&K(1, 7));
        m.insert(K(1, 7), 1);
        m.insert(K(3, 7), 2);
        let keys: Vec<u32> = m.keys().map(|k| k.0).collect();
        assert_eq!(keys, vec![3, 2, 1]);
        assert_eq!(m.values().copied().collect::<Vec<_>>(), vec![2, 0, 1]);
    }

    #[test]
    fn equality_ignores_order() {
        let (mut a, mut b) = (HashMap::new(), HashMap::new());
        for x in 0..20 {
            a.insert(K(x, 3), x);
            b.insert(K(19 - x, 3), 19 - x);
        }
        assert!(a == b);
        b.insert(K(0, 3), 1);
        assert!(a != b);
        b.remove(&K(0, 3));
        assert!(a != b);
        assert!(HashMap::<K, u32>::new() == HashMap::new());
    }

    #[test]
    fn colliding_parts_collide() {
        let parts = colliding_parts(2, 20);
        let mut hashes = Vec::new();
        for a in &parts[0] {
            for b in &parts[1] {
                hashes.push(hash_bytes(&[&a[..], &b[..]].concat()));
            }
        }
        assert!(hashes.iter().all(|&h| h == hashes[0]));
        assert!(parts
            .iter()
            .flatten()
            .all(|p| p.iter().all(|b| (0x20..0x7f).contains(b))));
        assert_ne!(hash_bytes(b"a"), hash_bytes(b"b"));
    }

    /// The index follows the entries down: a map that held 5,000 entries and keeps 10 is
    /// indexed for 10, so a remove's compaction (every `len / 2` removes) costs what the map
    /// holds, not what it held. Before, the index never shrank and an insert/remove pair on
    /// such a map rebuilt the old index every time: 17,000 fuel against a fresh map's 1,000.
    #[test]
    fn index_shrinks_with_the_map() {
        let k = |x: u32| K(x, 1 << 20);
        let mut m: HashMap<K, u32> = HashMap::new();
        for x in 0..5000 {
            m.insert(k(x), x);
        }
        assert!(m.buckets() >= 5000);
        for x in 0..4990 {
            m.remove(&k(x));
        }
        assert_eq!(m.len(), 10);
        assert!(m.buckets() <= 64, "{} buckets for 10 entries", m.buckets());
        for x in 10_000..15_000 {
            m.insert(k(x), x);
            m.remove(&k(x));
            assert!(m.buckets() <= 64, "{} buckets for {} entries", m.buckets(), m.len());
        }
        assert!(m.iter().map(|(k, _)| k.0).eq(4990..5000));
    }

    /// Keys built to collide: every one in one sorted bucket, each operation still found.
    #[test]
    fn colliding_keys_share_a_sorted_bucket() {
        let n = if cfg!(miri) { 200 } else { 100_000 };
        let mut m = HashMap::new();
        for x in 0..n {
            m.insert(K(x, 1), x);
        }
        for x in 0..n {
            assert_eq!(m.get(&K(x, 1)), Some(&x));
        }
        assert_eq!(m.sorted_buckets(), 1);
    }
}
