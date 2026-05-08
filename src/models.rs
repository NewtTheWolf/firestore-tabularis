//! Shared request/response shapes.
//!
//! These mirror the `ConnectionParams` struct the host sends. Keep fields
//! optional — different database types leave different fields blank.

use serde_json::Value;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConnectionParams {
    pub driver: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssl_mode: Option<String>,
}

#[allow(dead_code)]
impl ConnectionParams {
    pub fn from_value(value: &Value) -> Self {
        let obj = value.as_object();
        let get_str = |k: &str| {
            obj.and_then(|o| o.get(k))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let port = obj
            .and_then(|o| o.get("port"))
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok());

        Self {
            driver: get_str("driver"),
            host: get_str("host"),
            port,
            database: get_str("database"),
            username: get_str("username"),
            password: get_str("password"),
            ssl_mode: get_str("ssl_mode"),
        }
    }
}

/// Extract the nested `params` object every RPC method receives.
/// Tabularis wraps connection params in `params.params`.
#[allow(dead_code)]
pub fn inner_params(value: &Value) -> &Value {
    value.get("params").unwrap_or(&Value::Null)
}

/// Plugin-wide settings as delivered by the host's `initialize` call.
#[derive(Debug, Clone)]
pub struct Settings {
    pub project_id: String,
    pub database_id: String,
    pub service_account_path: Option<String>,
    pub emulator_host: Option<String>,
    pub sample_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            database_id: "(default)".into(),
            service_account_path: None,
            emulator_host: None,
            sample_size: 50,
        }
    }
}

impl Settings {
    pub fn from_value(v: &Value) -> Self {
        let mut s = Self::default();
        let Some(obj) = v.as_object() else {
            return s;
        };

        if let Some(p) = obj.get("project_id").and_then(Value::as_str) {
            s.project_id = p.to_string();
        }
        if let Some(d) = obj
            .get("database_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            s.database_id = d.to_string();
        }
        s.service_account_path = obj
            .get("service_account_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        s.emulator_host = obj
            .get("emulator_host")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(n) = obj.get("sample_size").and_then(Value::as_u64) {
            s.sample_size = n.try_into().unwrap_or(50).max(1);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_uses_defaults_for_missing_keys() {
        let s = Settings::from_value(&Value::Null);
        assert_eq!(s.project_id, "");
        assert_eq!(s.database_id, "(default)");
        assert!(s.service_account_path.is_none());
        assert!(s.emulator_host.is_none());
        assert_eq!(s.sample_size, 50);
    }

    #[test]
    fn settings_reads_provided_values() {
        let v = json!({
            "project_id": "p1",
            "database_id": "named-db",
            "service_account_path": "/etc/sa.json",
            "emulator_host": "localhost:8080",
            "sample_size": 25
        });
        let s = Settings::from_value(&v);
        assert_eq!(s.project_id, "p1");
        assert_eq!(s.database_id, "named-db");
        assert_eq!(s.service_account_path.as_deref(), Some("/etc/sa.json"));
        assert_eq!(s.emulator_host.as_deref(), Some("localhost:8080"));
        assert_eq!(s.sample_size, 25);
    }

    #[test]
    fn settings_treats_empty_strings_as_unset() {
        let v = json!({ "service_account_path": "", "emulator_host": "" });
        let s = Settings::from_value(&v);
        assert!(s.service_account_path.is_none());
        assert!(s.emulator_host.is_none());
    }

    #[test]
    fn settings_clamps_zero_sample_size_to_one() {
        let v = json!({ "sample_size": 0 });
        let s = Settings::from_value(&v);
        assert_eq!(s.sample_size, 1);
    }
}
