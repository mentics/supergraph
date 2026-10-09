//! Process-wide append-only string interner.
//!
//! [`Sym`] is a `u32` index into a global table, so it is `Copy`, hashes and compares in O(1),
//! and can render itself (`as_str`, `Display`, serde) without a context argument. Interned
//! strings are leaked and never freed.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const SHARDS: usize = 16;

struct Shard {
    lookup: HashMap<&'static str, u32>,
}

struct Interner {
    shards: [RwLock<Shard>; SHARDS],
    /// Index -> text. Appended to under the write lock; reads take the read lock briefly.
    strings: RwLock<Vec<&'static str>>,
}

fn interner() -> &'static Interner {
    static INTERNER: OnceLock<Interner> = OnceLock::new();
    INTERNER.get_or_init(|| Interner {
        shards: std::array::from_fn(|_| {
            RwLock::new(Shard {
                lookup: HashMap::new(),
            })
        }),
        strings: RwLock::new(vec![""]),
    })
}

fn shard_of(text: &str) -> usize {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash >> 32) as usize % SHARDS
}

/// An interned string. Equality and hashing use the index; ordering uses the text.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sym(u32);

impl Sym {
    /// The empty string.
    pub const EMPTY: Sym = Sym(0);

    pub fn new(text: &str) -> Sym {
        if text.is_empty() {
            return Sym::EMPTY;
        }
        let interner = interner();
        let shard = &interner.shards[shard_of(text)];
        if let Some(index) = shard.read().expect("interner shard").lookup.get(text) {
            return Sym(*index);
        }
        let mut guard = shard.write().expect("interner shard");
        if let Some(index) = guard.lookup.get(text) {
            return Sym(*index);
        }
        let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
        let index = {
            let mut strings = interner.strings.write().expect("interner table");
            strings.push(leaked);
            u32::try_from(strings.len() - 1).expect("interner exceeded u32::MAX strings")
        };
        guard.lookup.insert(leaked, index);
        Sym(index)
    }

    pub fn as_str(self) -> &'static str {
        interner().strings.read().expect("interner table")[self.0 as usize]
    }

    pub fn index(self) -> u32 {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self == Sym::EMPTY
    }

    /// Number of distinct strings interned so far (including the empty string).
    pub fn count() -> usize {
        interner().strings.read().expect("interner table").len()
    }
}

impl Default for Sym {
    fn default() -> Self {
        Sym::EMPTY
    }
}

impl PartialOrd for Sym {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Sym {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.0 == other.0 {
            return std::cmp::Ordering::Equal;
        }
        self.as_str().cmp(other.as_str())
    }
}

impl fmt::Display for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl From<&str> for Sym {
    fn from(text: &str) -> Self {
        Sym::new(text)
    }
}

impl From<&String> for Sym {
    fn from(text: &String) -> Self {
        Sym::new(text)
    }
}

impl From<String> for Sym {
    fn from(text: String) -> Self {
        Sym::new(&text)
    }
}

impl From<Sym> for String {
    fn from(sym: Sym) -> Self {
        sym.as_str().to_owned()
    }
}

impl AsRef<str> for Sym {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Sym {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for Sym {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Sym {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for Sym {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Sym {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Ok(Sym::new(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_and_round_trips() {
        let a = Sym::new("normalized expression fact");
        let b = Sym::new("normalized expression fact");
        assert_eq!(a, b);
        assert_eq!(a.index(), b.index());
        assert_eq!(a.as_str(), "normalized expression fact");
        assert_eq!(Sym::new(""), Sym::EMPTY);
        assert_eq!(Sym::EMPTY.as_str(), "");
        assert_eq!(std::mem::size_of::<Sym>(), 4);
    }

    #[test]
    fn orders_by_content_not_index() {
        let late = Sym::new("zzz-order-test");
        let early = Sym::new("aaa-order-test");
        assert!(late.index() < early.index());
        assert!(early < late);
        let mut syms = vec![late, early];
        syms.sort();
        assert_eq!(syms, vec![early, late]);
    }

    #[test]
    fn serde_uses_text() {
        let sym = Sym::new("serde \"quoted\"");
        let json = serde_json::to_string(&sym).unwrap();
        assert_eq!(json, "\"serde \\\"quoted\\\"\"");
        assert_eq!(serde_json::from_str::<Sym>(&json).unwrap(), sym);
    }

    #[test]
    fn concurrent_interning_agrees() {
        let results: Vec<Vec<Sym>> = std::thread::scope(|scope| {
            (0..8)
                .map(|thread| {
                    scope.spawn(move || {
                        (0..500)
                            .map(|n| Sym::new(&format!("concurrent-{}", (n + thread) % 500)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        for syms in &results {
            for (n, sym) in syms.iter().enumerate() {
                assert!(sym.as_str().starts_with("concurrent-"));
                assert_eq!(*sym, Sym::new(sym.as_str()), "entry {n}");
            }
        }
        let first: std::collections::HashSet<_> = results[0].iter().copied().collect();
        let last: std::collections::HashSet<_> = results[7].iter().copied().collect();
        assert_eq!(first, last);
        assert_eq!(first.len(), 500);
    }
}
