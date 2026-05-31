//! Driver connection layer — builds `firestore::FirestoreDb` clients keyed by
//! database_id so one Tabularis connection can browse multiple Firestore
//! databases as separate schemas.
//!
//! Configuration is passed via `FirestoreDbOptions` (emulator URL) and
//! `with_options_service_account_key_file` (auth) — never via process env
//! vars, which would race with other threads (`std::env::set_var` is
//! `unsafe` since Rust 1.80) and persist across re-`initialize` calls.

use std::path::PathBuf;

use firestore::{FirestoreDb, FirestoreDbOptions};
use serde_json::Value;

use crate::error::PluginError;
use crate::models::Settings;

pub async fn build(settings: &Settings, database_id: &str) -> Result<FirestoreDb, PluginError> {
    if settings.project_id.is_empty() {
        return Err(PluginError::invalid_params(
            "project_id is empty — set it in plugin settings before connecting",
        ));
    }

    let mut options = FirestoreDbOptions::new(settings.project_id.clone())
        .with_database_id(database_id.to_string());

    if let Some(host) = settings.emulator_host.as_deref().filter(|s| !s.is_empty()) {
        let url = if host.starts_with("http://") || host.starts_with("https://") {
            host.to_string()
        } else {
            format!("http://{host}")
        };
        options = options.with_firebase_api_url(url);
    }

    let result = match settings
        .service_account_path
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        Some(path) => {
            FirestoreDb::with_options_service_account_key_file(options, PathBuf::from(path)).await
        }
        None => FirestoreDb::with_options(options).await,
    };

    result.map_err(|e| PluginError::internal(format!("Firestore connect: {e}")))
}

/// Resolve a FirestoreDb client for `schema` (the database_id), initializing
/// on first access. `None` falls back to the configured `settings.database_id`
/// so legacy hosts keep working unchanged.
pub async fn resolve_for(id: Value, schema: Option<&str>) -> Result<FirestoreDb, Value> {
    let Some(settings) = crate::state::settings() else {
        return Err(crate::rpc::error_response(
            id,
            -32602,
            "plugin not initialised",
            None,
        ));
    };
    let target = match schema {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => settings.database_id.clone(),
    };
    crate::state::get_or_build_client(&target, || async { build(settings, &target).await })
        .await
        .map_err(|err| crate::rpc::error_response(id.clone(), err.code, &err.message, None))
}

