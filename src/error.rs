//! Plugin-local error type. Keep deps light — no `anyhow`/`thiserror` by default.

use std::fmt;

#[derive(Debug)]
pub struct PluginError {
    pub code: i64,
    pub message: String,
}

impl PluginError {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_uses_jsonrpc_internal_code() {
        let e = PluginError::internal("boom");
        assert_eq!(e.code, -32603);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn invalid_params_uses_jsonrpc_invalid_params_code() {
        let e = PluginError::invalid_params("missing x");
        assert_eq!(e.code, -32602);
    }

    #[test]
    fn display_format_includes_code() {
        let e = PluginError::internal("bad");
        assert_eq!(format!("{e}"), "[-32603] bad");
    }

    #[test]
    fn implements_std_error_trait() {
        fn assert_err<E: std::error::Error>(_: &E) {}
        let e = PluginError::internal("x");
        assert_err(&e);
    }
}
