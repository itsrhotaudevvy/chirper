//! The error every command returns to the webview: `{ "code": string, "message": string }`.

use serde::Serialize;

/// Error codes of CONTRACT.md section C.
pub mod code {
    pub const INVALID_INPUT: &str = "invalid_input";
    pub const CORE: &str = "core";
    pub const NOT_READY: &str = "not_ready";
    pub const LOCKED: &str = "locked";
    pub const UNSUPPORTED: &str = "unsupported";
    pub const IO: &str = "io";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
}

pub type CmdResult<T> = Result<T, CmdError>;

impl CmdError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(field: &str) -> Self {
        Self::new(code::INVALID_INPUT, format!("invalid {field}"))
    }

    pub fn locked() -> Self {
        Self::new(code::LOCKED, "the app is locked")
    }

    pub fn not_ready() -> Self {
        Self::new(code::NOT_READY, "the core is not ready")
    }

    /// The feature exists in the contract but this platform does not implement it yet.
    pub fn unsupported(feature: &str) -> Self {
        Self::new(
            code::UNSUPPORTED,
            format!("{feature} is not supported on this platform yet"),
        )
    }
}

impl From<chirper_core::CoreError> for CmdError {
    fn from(err: chirper_core::CoreError) -> Self {
        use chirper_core::CoreError;
        match err {
            CoreError::InvalidInput(field) => Self::invalid(&field),
            CoreError::Closed => Self::not_ready(),
            CoreError::Io(e) => Self::new(code::IO, e.to_string()),
            CoreError::Go(message) => Self::new(code::CORE, message),
            CoreError::Ffi(message) | CoreError::KeyStore(message) => {
                Self::new(code::CORE, message)
            }
            CoreError::Json(e) => Self::new(code::CORE, e.to_string()),
        }
    }
}

impl From<std::io::Error> for CmdError {
    fn from(err: std::io::Error) -> Self {
        Self::new(code::IO, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_code_and_message() {
        let json = serde_json::to_value(CmdError::locked()).unwrap();
        assert_eq!(json["code"], "locked");
        assert_eq!(json["message"], "the app is locked");
    }

    #[test]
    fn core_errors_map_to_contract_codes() {
        use chirper_core::CoreError;
        assert_eq!(CmdError::from(CoreError::Closed).code, code::NOT_READY,);
        assert_eq!(
            CmdError::from(CoreError::InvalidInput("room id".into())).code,
            code::INVALID_INPUT,
        );
        assert_eq!(
            CmdError::from(CoreError::Go("boom".into())).code,
            code::CORE,
        );
    }
}
