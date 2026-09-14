//! Error codes surfaced by the MCP tools.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    NotConfigured,
    ApprovalRequired,
    PolicyDenied,
    WorkerUnavailable,
    WorkerFailed,
    Conflict,
    InternalError,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::NotFound => "not_found",
            ErrorCode::NotConfigured => "not_configured",
            ErrorCode::ApprovalRequired => "approval_required",
            ErrorCode::PolicyDenied => "policy_denied",
            ErrorCode::WorkerUnavailable => "worker_unavailable",
            ErrorCode::WorkerFailed => "worker_failed",
            ErrorCode::Conflict => "conflict",
            ErrorCode::InternalError => "internal_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ToolError {
        ToolError { code, message: message.into() }
    }

    pub fn invalid_request(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::InvalidRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::NotFound, message)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error {}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ToolError {}

/// A database failure inside a tool is an internal error: nothing the caller
/// passed can fix it.
impl From<rusqlite::Error> for ToolError {
    fn from(err: rusqlite::Error) -> ToolError {
        ToolError::new(ErrorCode::InternalError, err.to_string())
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(err: anyhow::Error) -> ToolError {
        ToolError::new(ErrorCode::InternalError, err.to_string())
    }
}

impl From<std::io::Error> for ToolError {
    fn from(err: std::io::Error) -> ToolError {
        ToolError::new(ErrorCode::InternalError, err.to_string())
    }
}
