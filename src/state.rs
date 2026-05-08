//! Global plugin state. All accessors are safe to call from any async context.

use std::collections::HashMap;
use std::sync::RwLock;
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

/// Per-(table, where) cached row counts, populated by execute_query, evicted after 30 s
/// or when capacity exceeds 200 keys.
#[allow(dead_code)]
pub static COUNT_CACHE: Lazy<RwLock<TtlLruCache<CountKey, u64>>> =
    Lazy::new(|| RwLock::new(TtlLruCache::new(200, Duration::from_secs(30))));

/// Per-(table, where, order_by) cached cursors for sequential pagination.
/// Each entry maps page-end offset to the FirestoreDocument that closes that page.
#[allow(dead_code)]
pub static CURSOR_CACHE: Lazy<RwLock<TtlLruCache<QueryKey, CursorEntry>>> =
    Lazy::new(|| RwLock::new(TtlLruCache::new(100, Duration::from_secs(300))));

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
    #[allow(dead_code)]
    pub cursors: std::collections::BTreeMap<u64, firestore::FirestoreDocument>,
}

pub fn settings() -> Option<&'static Settings> {
    SETTINGS.get()
}
