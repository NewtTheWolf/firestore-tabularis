//! Global plugin state. All accessors are safe to call from any async context.
//!
//! Locking choice: `Mutex` (not `RwLock`) on the TTL/LRU caches because their
//! `get` mutates (TTL eviction touches the entry list), so a read lock would be
//! a lie. `SCHEMA_CACHE` is read-mostly so it stays an `RwLock`.
//! All `lock()` / `read()` / `write()` calls are wrapped via the `lock_*` helpers
//! to recover from lock poisoning instead of panicking — a panic in one handler
//! must not poison the cache and take down every subsequent request.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use once_cell::sync::{Lazy, OnceCell};

use crate::cache::TtlLruCache;
use crate::models::Settings;
use crate::schema_infer::ColumnInfo;

pub static SETTINGS: OnceCell<Settings> = OnceCell::new();
pub static CLIENT: tokio::sync::OnceCell<firestore::FirestoreDb> =
    tokio::sync::OnceCell::const_new();
pub static SCHEMA_CACHE: Lazy<RwLock<HashMap<String, Vec<ColumnInfo>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Per-(table, where) cached row counts. TTL 30 s — short enough that external
/// writes (Firebase Console, Phase 3 mutations) don't stay invisible long, long
/// enough to absorb a full Tabularis grid-paint that hits the same query
/// repeatedly. Capacity 200 keys is generous for typical use and bounded enough
/// that an attacker can't OOM us via cache-key churn.
pub static COUNT_CACHE: Lazy<Mutex<TtlLruCache<CountKey, u64>>> =
    Lazy::new(|| Mutex::new(TtlLruCache::new(200, Duration::from_secs(30))));

/// Per-(table, where, order_by) cached cursors for sequential pagination.
/// Each entry maps page-end offset to the FirestoreDocument that closes that
/// page. TTL 5 min covers a typical user-pagination session; capacity 100 keys
/// covers a few open tabs each browsing different filters.
pub static CURSOR_CACHE: Lazy<Mutex<TtlLruCache<QueryKey, CursorEntry>>> =
    Lazy::new(|| Mutex::new(TtlLruCache::new(100, Duration::from_secs(300))));

/// Recover from poison: a panic mid-mutation should not take the whole plugin
/// down. The cache state may be inconsistent for one entry, but TTL eviction
/// will clear it within seconds.
pub fn lock_count_cache() -> MutexGuard<'static, TtlLruCache<CountKey, u64>> {
    COUNT_CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn lock_cursor_cache() -> MutexGuard<'static, TtlLruCache<QueryKey, CursorEntry>> {
    CURSOR_CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn schema_cache_read() -> RwLockReadGuard<'static, HashMap<String, Vec<ColumnInfo>>> {
    SCHEMA_CACHE.read().unwrap_or_else(|p| p.into_inner())
}

pub fn schema_cache_write() -> RwLockWriteGuard<'static, HashMap<String, Vec<ColumnInfo>>> {
    SCHEMA_CACHE.write().unwrap_or_else(|p| p.into_inner())
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct CountKey {
    pub table: String,
    pub where_canonical: String,
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct QueryKey {
    pub table: String,
    pub where_canonical: String,
    pub order_by_canonical: String,
}

#[derive(Clone, Debug)]
pub struct CursorEntry {
    pub cursors: std::collections::BTreeMap<u64, firestore::FirestoreDocument>,
}

pub fn settings() -> Option<&'static Settings> {
    SETTINGS.get()
}
