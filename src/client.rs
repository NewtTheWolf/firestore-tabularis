//! Driver connection layer — builds a `firestore::FirestoreDb` from `Settings`.

use firestore::{FirestoreDb, FirestoreDbOptions};

use crate::error::PluginError;
use crate::models::Settings;

pub async fn build(settings: &Settings) -> Result<FirestoreDb, PluginError> {
    if settings.project_id.is_empty() {
        return Err(PluginError::invalid_params(
            "project_id is empty — set it in plugin settings before connecting",
        ));
    }

    // Honour the standard emulator env var. Setting it here means firestore-rs picks
    // up the emulator endpoint without further config.
    if let Some(host) = &settings.emulator_host {
        std::env::set_var("FIRESTORE_EMULATOR_HOST", host);
    }

    // If the user supplied an explicit service-account file, point ADC at it via the
    // same env var firestore-rs reads. Empty path is interpreted as "fall back to ADC".
    if let Some(path) = &settings.service_account_path {
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", path);
    }

    let options = FirestoreDbOptions::new(settings.project_id.clone())
        .with_database_id(settings.database_id.clone());

    FirestoreDb::with_options(options)
        .await
        .map_err(|e| PluginError::internal(format!("Firestore connect: {e}")))
}
