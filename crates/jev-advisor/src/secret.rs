//! API key holder. Never printed: `Debug` redacts, there is no `Display`/`Serialize`.

use std::path::Path;

#[derive(Clone)]
pub struct Secret(Box<str>);

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("environment variable `{0}` is not set")]
    EnvMissing(String),
    #[error("secret file could not be read")]
    FileUnreadable,
    #[error("secret is empty or contains whitespace/control characters")]
    Malformed,
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretError> {
        let v: String = value.into();
        let v = v.trim().to_string();
        if v.is_empty()
            || v.chars()
                .any(|c| c.is_whitespace() || c.is_control() || !c.is_ascii())
        {
            return Err(SecretError::Malformed);
        }
        Ok(Self(v.into_boxed_str()))
    }

    /// Read from the named environment variable (the variable name is not secret).
    pub fn from_env(var: &str) -> Result<Self, SecretError> {
        let v = std::env::var(var).map_err(|_| SecretError::EnvMissing(var.to_string()))?;
        Self::new(v)
    }

    /// Read from a file (trailing newline allowed). The path and contents never enter errors.
    pub fn from_file(path: &Path) -> Result<Self, SecretError> {
        let v = std::fs::read_to_string(path).map_err(|_| SecretError::FileUnreadable)?;
        Self::new(v)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
