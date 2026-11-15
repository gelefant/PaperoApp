use crate::config::AppConfig;
use crate::error::{CoreError, Result};
use std::fmt;
use std::path::Path;

pub const KEYRING_SERVICE: &str = "org.papero.app";
pub const KEY_POLITE_EMAIL: &str = "polite_email";

/// Wrapper for sensitive API keys that NEVER leaks the raw secret in Debug/Display logs.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretApiKey(String);

impl SecretApiKey {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for SecretApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretApiKey([REDACTED])")
    }
}

impl fmt::Display for SecretApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Saves the optional polite pool email (`mailto`) in the OS keyring (`org.papero.app`),
/// falling back ONLY for `polite_email` (never for API keys!) to `config.json` on headless setups.
pub fn save_polite_email(email: Option<&str>, config_path: Option<&Path>) -> Result<()> {
    let cleaned = email.map(str::trim).filter(|s| !s.is_empty());
    let keyring_ok = match keyring::Entry::new(KEYRING_SERVICE, KEY_POLITE_EMAIL) {
        Ok(entry) => match cleaned {
            Some(addr) => entry.set_password(addr).is_ok(),
            None => matches!(
                entry.delete_credential(),
                Ok(()) | Err(keyring::Error::NoEntry)
            ),
        },
        Err(_) => false,
    };

    if let Some(cfg_path) = config_path {
        let mut cfg = AppConfig::load_or_default(cfg_path)?;
        // Store in config.json only if keyring failed or clearing existing fallback.
        cfg.polite_email_fallback = if keyring_ok {
            None
        } else {
            cleaned.map(ToOwned::to_owned)
        };
        cfg.save(cfg_path)?;
    } else if !keyring_ok && cleaned.is_some() {
        return Err(CoreError::Config(
            "OS keyring unavailable and no config fallback path provided".to_string(),
        ));
    }

    Ok(())
}

/// Loads the optional polite pool email from OS keyring first, then `config.json` fallback.
pub fn load_polite_email(config: &AppConfig) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEY_POLITE_EMAIL) {
        if let Ok(pw) = entry.get_password() {
            let trimmed = pw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    config
        .polite_email_fallback
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Stores an API secret STRICTLY in the OS keyring (`org.papero.app`). Never falls back to disk.
pub fn save_secret_key(account_key: &str, secret: &SecretApiKey) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account_key)
        .map_err(|e| CoreError::Config(format!("keyring init failed for {account_key}: {e}")))?;
    entry
        .set_password(secret.expose())
        .map_err(|e| CoreError::Config(format!("keyring store failed for {account_key}: {e}")))
}

pub fn load_secret_key(account_key: &str) -> Result<Option<SecretApiKey>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account_key)
        .map_err(|e| CoreError::Config(format!("keyring init failed for {account_key}: {e}")))?;
    match entry.get_password() {
        Ok(pw) if !pw.trim().is_empty() => Ok(Some(SecretApiKey::new(pw))),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CoreError::Config(format!(
            "keyring load failed for {account_key}: {e}"
        ))),
    }
}

pub fn delete_secret_key(account_key: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account_key)
        .map_err(|e| CoreError::Config(format!("keyring init failed for {account_key}: {e}")))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CoreError::Config(format!(
            "keyring delete failed for {account_key}: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_api_key_never_prints_raw_value() {
        let key = SecretApiKey::new("sk-super-secret-value");
        assert_eq!(format!("{key}"), "[REDACTED]");
        assert_eq!(format!("{key:?}"), "SecretApiKey([REDACTED])");
        assert!(!format!("{key:?}").contains("sk-super-secret"));
    }
}
