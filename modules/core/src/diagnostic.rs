//! Stable domain errors allow callers to translate without parsing English text.
use std::{error::Error, fmt, io};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    InvalidPath,
    ProtectedPath,
    RecursiveRequired,
    IdentityChanged,
    DirectoryChanged,
    InvalidOptions,
    System,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPath => "invalid_path",
            Self::ProtectedPath => "protected_path",
            Self::RecursiveRequired => "recursive_required",
            Self::IdentityChanged => "identity_changed",
            Self::DirectoryChanged => "directory_changed",
            Self::InvalidOptions => "invalid_options",
            Self::System => "system",
        }
    }
}

#[derive(Debug)]
struct DomainError {
    code: DiagnosticCode,
    message: String,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}

impl Error for DomainError {}

pub(crate) fn error(code: DiagnosticCode, message: impl Into<String>) -> io::Error {
    let kind = match code {
        DiagnosticCode::InvalidPath
        | DiagnosticCode::ProtectedPath
        | DiagnosticCode::RecursiveRequired
        | DiagnosticCode::InvalidOptions => io::ErrorKind::InvalidInput,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(
        kind,
        DomainError {
            code,
            message: message.into(),
        },
    )
}

/// Unknown errors retain their original OS code and system description.
pub fn diagnostic_code(error: &io::Error) -> DiagnosticCode {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<DomainError>())
        .map_or(DiagnosticCode::System, |inner| inner.code)
}

#[cfg(test)]
mod tests;
