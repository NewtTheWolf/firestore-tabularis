//! Global plugin state. All accessors are safe to call from any async context.

use std::collections::HashMap;
use std::sync::RwLock;

use once_cell::sync::{Lazy, OnceCell};

use crate::models::Settings;
use crate::schema_infer::ColumnInfo;

pub static SETTINGS: OnceCell<Settings> = OnceCell::new();
pub static CLIENT: tokio::sync::OnceCell<firestore::FirestoreDb> =
    tokio::sync::OnceCell::const_new();
pub static SCHEMA_CACHE: Lazy<RwLock<HashMap<String, Vec<ColumnInfo>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub fn settings() -> Option<&'static Settings> {
    SETTINGS.get()
}
