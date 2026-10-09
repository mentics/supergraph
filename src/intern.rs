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
pub struct Sym(std::num::NonZeroU32);

impl Sym {
    /// The empty string.
    pub const EMPTY: Sym = Sym(std::num::NonZeroU32::MIN);

    pub fn new(text: &str) -> Sym {
        if text.is_empty() {
            return Sym::EMPTY;
        }
        let interner = interner();
        let shard = &interner.shards[shard_of(text)];
        if let Some(index) = shard.read().expect("interner shard").lookup.get(text) {
            return Sym::from_index(*index);
        }
        let mut guard = shard.write().expect("interner shard");
        if let Some(index) = guard.lookup.get(text) {
            return Sym::from_index(*index);
        }
        let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
        let index = {
            let mut strings = interner.strings.write().expect("interner table");
            strings.push(leaked);
            u32::try_from(strings.len() - 1).expect("interner exceeded u32::MAX strings")
        };
        guard.lookup.insert(leaked, index);
        Sym::from_index(index)
    }

    fn from_index(index: u32) -> Sym {
        Sym(std::num::NonZeroU32::new(index + 1).expect("interner index overflow"))
    }

    pub fn as_str(self) -> &'static str {
        interner().strings.read().expect("interner table")[self.index() as usize]
    }

    /// Dense index of this string in the interner table (the empty string is 0).
    pub fn index(self) -> u32 {
        self.0.get() - 1
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
        if self == other {
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

impl std::ops::Deref for Sym {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<String> for Sym {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<Sym> for str {
    fn eq(&self, other: &Sym) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Sym> for &str {
    fn eq(&self, other: &Sym) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Sym> for String {
    fn eq(&self, other: &Sym) -> bool {
        self.as_str() == other.as_str()
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

thread_local! {
    /// Global interner index -> position in the written string table; set while writing the
    /// compact (table) form so `Sym` serializes as a number instead of text.
    static WRITE_RANKS: std::cell::RefCell<Option<std::sync::Arc<Vec<u32>>>> =
        const { std::cell::RefCell::new(None) };
    /// String table position -> `Sym`; set while loading a document that carries a table.
    static READ_TABLE: std::cell::RefCell<Option<Vec<Sym>>> =
        const { std::cell::RefCell::new(None) };
}

/// Snapshot of the interner for the compact on-disk form: every interned string in
/// ascending text order (so the table is deterministic) plus the rank of each `Sym`.
pub struct StringTable {
    pub strings: Vec<&'static str>,
    ranks: std::sync::Arc<Vec<u32>>,
}

impl StringTable {
    pub fn snapshot() -> StringTable {
        let strings = interner().strings.read().expect("interner table").clone();
        let mut order: Vec<u32> = (0..strings.len() as u32).collect();
        order.sort_unstable_by(|left, right| strings[*left as usize].cmp(strings[*right as usize]));
        let mut ranks = vec![0u32; strings.len()];
        for (rank, index) in order.iter().enumerate() {
            ranks[*index as usize] = rank as u32;
        }
        StringTable {
            strings: order.iter().map(|index| strings[*index as usize]).collect(),
            ranks: std::sync::Arc::new(ranks),
        }
    }

    /// Runs `f` with `Sym` serializing as table positions on the current thread.
    pub fn serialize_with<R>(&self, f: impl FnOnce() -> R) -> R {
        let previous = WRITE_RANKS.with(|slot| slot.replace(Some(self.ranks.clone())));
        let result = f();
        WRITE_RANKS.with(|slot| *slot.borrow_mut() = previous);
        result
    }
}

/// Installs the table read from a document so numeric `Sym`s on this thread resolve.
pub fn install_read_table(strings: &[String]) {
    let table = strings.iter().map(|text| Sym::new(text)).collect();
    READ_TABLE.with(|slot| *slot.borrow_mut() = Some(table));
}

/// Forgets the table installed by `install_read_table`.
pub fn clear_read_table() {
    READ_TABLE.with(|slot| *slot.borrow_mut() = None);
}

impl Serialize for Sym {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let rank = WRITE_RANKS.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|ranks| ranks[self.index() as usize])
        });
        match rank {
            Some(rank) => serializer.serialize_u32(rank),
            None => serializer.serialize_str(self.as_str()),
        }
    }
}

struct SymVisitor;

impl serde::de::Visitor<'_> for SymVisitor {
    type Value = Sym;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a string or a string-table index")
    }

    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Sym, E> {
        Ok(Sym::new(text))
    }

    fn visit_u64<E: serde::de::Error>(self, position: u64) -> Result<Sym, E> {
        READ_TABLE.with(|slot| match slot.borrow().as_ref() {
            None => Err(E::custom(
                "numeric string reference without a preceding `strings` table",
            )),
            Some(table) => table
                .get(position as usize)
                .copied()
                .ok_or_else(|| E::custom(format!("string table index {position} out of range"))),
        })
    }
}

impl<'de> Deserialize<'de> for Sym {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SymVisitor)
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
        assert_eq!(std::mem::size_of::<Option<Sym>>(), 4);
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
    fn table_form_round_trips() {
        let sym = Sym::new("table form text");
        let table = StringTable::snapshot();
        let json = table.serialize_with(|| serde_json::to_string(&sym).unwrap());
        assert!(json.parse::<u32>().is_ok(), "table form is numeric: {json}");
        let strings: Vec<String> = table.strings.iter().map(|text| text.to_string()).collect();
        assert!(serde_json::from_str::<Sym>(&json).is_err(), "no table installed yet");
        install_read_table(&strings);
        assert_eq!(serde_json::from_str::<Sym>(&json).unwrap(), sym);
        clear_read_table();
        assert_eq!(serde_json::to_string(&sym).unwrap(), "\"table form text\"");
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
