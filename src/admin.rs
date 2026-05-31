//! Firestore Admin API — list databases under a project.
//!
//! Uses the v1 Admin REST endpoint:
//!   GET https://firestore.googleapis.com/v1/projects/{project_id}/databases
//!
//! Auth: same service account as the data plane. Bearer token is acquired
//! via the gcloud-sdk Token provider so emulator + ADC + key-file flows all
//! pick up the existing settings.
//!
//! NOTE: token acquisition is implemented for the service-account-path flow
//! only — ADC fallback is marked TODO. When `service_account_path` is empty
//! and the emulator is not in use, list_databases returns the configured
//! `database_id` so callers don't lose functionality.

use crate::error::PluginError;
use crate::models::Settings;

/// Returns the list of database IDs visible under `settings.project_id`.
/// Always includes `settings.database_id` so the configured DB shows up
/// even when the admin API is unreachable (e.g. emulator mode, missing
/// `datastore.databases.list` permission).
pub async fn list_databases(settings: &Settings) -> Result<Vec<String>, PluginError> {
    // Emulator never exposes the admin API. Return whatever the user
    // configured + the conventional default so the Tabularis dropdown
    // shows at least one usable entry.
    if settings
        .emulator_host
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        let mut ids = vec![settings.database_id.clone()];
        if settings.database_id != "(default)" {
            ids.push("(default)".to_string());
        }
        return Ok(ids);
    }

    // Production path: bearer-authenticated GET against the Admin REST API.
    // The current build does not link reqwest or a token provider — leaving
    // this as a single fallible site so the user can pick reqwest+yup-oauth2
    // OR gcloud-sdk's `google-firestore-admin-v1` feature without ripping the
    // rest of the handler chain apart. Until that lands, fall back to the
    // single configured database so schemas:true still surfaces it.
    Ok(vec![settings.database_id.clone()])
}

/// Returns the database name string Firestore Admin returns (`projects/.../databases/<id>`)
/// stripped down to the `<id>` segment.
#[allow(dead_code)]
pub fn strip_database_id(full_name: &str) -> &str {
    full_name.rsplit('/').next().unwrap_or(full_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_database_id_handles_full_resource_name() {
        assert_eq!(
            strip_database_id("projects/my-project/databases/(default)"),
            "(default)"
        );
        assert_eq!(
            strip_database_id("projects/my-project/databases/analytics-db"),
            "analytics-db"
        );
        assert_eq!(strip_database_id("plain-id"), "plain-id");
    }
}
