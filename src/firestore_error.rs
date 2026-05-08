//! FirestoreError → JSON-RPC error mapping.
//!
//! Phase 1 surfaces missing-index URLs verbatim. Phase 2 will expand this with
//! IAM hints, quota backoff, and retry guidance.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

/// Coarse classification of a Firestore error, used to pick a response shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    FailedPrecondition,
    Unauthenticated,
    NotFound,
    PermissionDenied,
    ResourceExhausted,
    DeadlineExceeded,
    Unavailable,
    Other,
}

static INDEX_URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https://console\.(?:firebase|cloud)\.google\.com/[^\s'"]+"#).unwrap()
});

pub fn map_message(raw: &str, kind: ErrorKind) -> (i64, String, Option<Value>) {
    if kind == ErrorKind::FailedPrecondition {
        if let Some(m) = INDEX_URL_RE.find(raw) {
            let url = m.as_str().to_string();
            return (
                -32603,
                format!("Missing Firestore index. Create it: {url}"),
                Some(json!({ "create_index_url": url })),
            );
        }
    }
    if kind == ErrorKind::Unauthenticated {
        return (
            -32602,
            format!(
                "Auth failed: {raw}. Set service_account_path in plugin settings or run \
                 'gcloud auth application-default login'."
            ),
            None,
        );
    }
    if kind == ErrorKind::NotFound {
        return (-32602, format!("Not found: {raw}"), None);
    }
    if kind == ErrorKind::PermissionDenied {
        let project = crate::state::settings()
            .map(|s| s.project_id.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("the configured project");
        return (
            -32602,
            format!(
                "Access denied: {raw}. Check the service account's IAM roles for project '{project}' \
                 (needs at minimum 'roles/datastore.viewer' for reads)."
            ),
            None,
        );
    }
    if kind == ErrorKind::ResourceExhausted {
        return (
            -32603,
            format!(
                "Firestore quota exceeded: {raw}. Wait a minute and retry. \
                 If this persists, check the GCP Quotas page for your project."
            ),
            None,
        );
    }
    if kind == ErrorKind::DeadlineExceeded {
        return (
            -32603,
            format!(
                "Request timed out: {raw}. The query may be missing an index or scanning a very \
                 large collection — try LIMIT to narrow the result set."
            ),
            None,
        );
    }
    if kind == ErrorKind::Unavailable {
        return (
            -32603,
            format!(
                "Firestore temporarily unavailable: {raw}. \
                 This is usually transient — retry in a few seconds."
            ),
            None,
        );
    }
    (-32603, format!("Firestore: {raw}"), None)
}

/// Adapter for real `firestore::errors::FirestoreError`. Keeps the public-facing API
/// the handlers call into thin; the `map_message` helper carries the actual logic and
/// is the only thing the unit tests exercise.
pub fn map_error(err: &firestore::errors::FirestoreError) -> (i64, String, Option<Value>) {
    let raw = err.to_string();
    let kind = classify(&raw);
    map_message(&raw, kind)
}

fn classify(raw: &str) -> ErrorKind {
    // Phase 1 uses substring matching on the gRPC status name. firestore-rs surfaces
    // these tokens in the Display output of `FirestoreError::DatabaseError` variants.
    if raw.contains("FAILED_PRECONDITION") {
        ErrorKind::FailedPrecondition
    } else if raw.contains("UNAUTHENTICATED") {
        ErrorKind::Unauthenticated
    } else if raw.contains("PERMISSION_DENIED") {
        ErrorKind::PermissionDenied
    } else if raw.contains("NOT_FOUND") {
        ErrorKind::NotFound
    } else if raw.contains("RESOURCE_EXHAUSTED") {
        ErrorKind::ResourceExhausted
    } else if raw.contains("DEADLINE_EXCEEDED") {
        ErrorKind::DeadlineExceeded
    } else if raw.contains("UNAVAILABLE") {
        ErrorKind::Unavailable
    } else {
        ErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_firebase_console_url() {
        let raw = "FAILED_PRECONDITION: The query requires an index. \
                   You can create it here: https://console.firebase.google.com/v1/r/project/p1/firestore/indexes?create_composite=abc";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.starts_with(
            "Missing Firestore index. Create it: https://console.firebase.google.com/"
        ));
        let url = data.unwrap()["create_index_url"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(url.starts_with("https://console.firebase.google.com/"));
    }

    #[test]
    fn extracts_cloud_console_url() {
        let raw = "FAILED_PRECONDITION: missing index, see https://console.cloud.google.com/firestore/indexes?project=p1";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.contains("https://console.cloud.google.com/"));
        assert!(data.is_some());
    }

    #[test]
    fn failed_precondition_without_url_falls_through() {
        // No index URL in the message → not classified as missing-index; default branch.
        let (code, msg, data) = map_message(
            "FAILED_PRECONDITION: something else",
            ErrorKind::FailedPrecondition,
        );
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: "));
        assert!(data.is_none());
    }

    #[test]
    fn unauthenticated_message_includes_setup_hint() {
        let (code, msg, data) =
            map_message("UNAUTHENTICATED: bad creds", ErrorKind::Unauthenticated);
        assert_eq!(code, -32602);
        assert!(msg.contains("service_account_path"));
        assert!(msg.contains("gcloud auth application-default login"));
        assert!(data.is_none());
    }

    #[test]
    fn not_found_uses_invalid_params_code() {
        let (code, msg, _) =
            map_message("NOT_FOUND: collection 'gone' missing", ErrorKind::NotFound);
        assert_eq!(code, -32602);
        assert!(msg.starts_with("Not found: "));
    }

    #[test]
    fn other_kind_falls_through_to_internal_error() {
        let (code, msg, data) = map_message("DEADLINE_EXCEEDED", ErrorKind::Other);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: "));
        assert!(data.is_none());
    }

    #[test]
    fn url_regex_does_not_match_non_console_links() {
        // Make sure the regex isn't matching unrelated URLs that happen to mention google.com.
        let raw =
            "FAILED_PRECONDITION: see https://example.com/help and https://google.com/policies";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: ")); // fell through to default
        assert!(data.is_none());
    }

    #[test]
    fn classifier_recognises_each_status_token() {
        assert_eq!(
            classify("rpc error: code = FAILED_PRECONDITION"),
            ErrorKind::FailedPrecondition
        );
        assert_eq!(
            classify("rpc error: code = UNAUTHENTICATED"),
            ErrorKind::Unauthenticated
        );
        assert_eq!(classify("rpc error: code = NOT_FOUND"), ErrorKind::NotFound);
        assert_eq!(classify("rpc error: code = INTERNAL"), ErrorKind::Other);
    }

    #[test]
    fn permission_denied_message_includes_project_id_and_role_hint() {
        let (code, msg, _) = map_message(
            "PERMISSION_DENIED: missing scope",
            ErrorKind::PermissionDenied,
        );
        assert_eq!(code, -32602);
        assert!(msg.contains("Access denied"));
        assert!(msg.contains("roles/datastore.viewer"));
    }

    #[test]
    fn resource_exhausted_message_mentions_quota() {
        let (code, msg, _) = map_message("RESOURCE_EXHAUSTED: quota", ErrorKind::ResourceExhausted);
        assert_eq!(code, -32603);
        assert!(msg.contains("quota exceeded"));
    }

    #[test]
    fn deadline_exceeded_message_suggests_limit() {
        let (code, msg, _) = map_message("DEADLINE_EXCEEDED", ErrorKind::DeadlineExceeded);
        assert_eq!(code, -32603);
        assert!(msg.contains("LIMIT"));
    }

    #[test]
    fn unavailable_message_says_transient() {
        let (code, msg, _) = map_message("UNAVAILABLE: temp", ErrorKind::Unavailable);
        assert_eq!(code, -32603);
        assert!(msg.contains("transient"));
    }

    #[test]
    fn classifier_recognises_new_status_tokens() {
        assert_eq!(
            classify("rpc error: code = PERMISSION_DENIED"),
            ErrorKind::PermissionDenied
        );
        assert_eq!(
            classify("rpc error: code = RESOURCE_EXHAUSTED"),
            ErrorKind::ResourceExhausted
        );
        assert_eq!(
            classify("rpc error: code = DEADLINE_EXCEEDED"),
            ErrorKind::DeadlineExceeded
        );
        assert_eq!(
            classify("rpc error: code = UNAVAILABLE"),
            ErrorKind::Unavailable
        );
    }
}
