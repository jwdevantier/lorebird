// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2025 Jesper Devantier <jwd@defmacro.it>

//! Send email via SMTP using the `lettre` crate.
//!
//! This crate provides a synchronous `send()` function callable from the
//! Lua thread.  It mirrors what `msmtp` does — TLS connection, SMTP AUTH,
//! envelope-based delivery — but without requiring an external binary.
//!
//! Three TLS modes are supported:
//!   - STARTTLS (port 587, most common)
//!   - SMTPS / wrapper TLS (port 465)
//!   - Unencrypted localhost (for local MTA)

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};

// ── Error type ────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("SMTP error: {0}")]
    Smtp(String),

    #[error("Invalid address: {0}")]
    Address(String),

    #[error("Missing SMTP configuration")]
    NoConfig,

    #[error("Password evaluation failed: {0}")]
    CommandEval(String),
}

// ── Configuration ─────────────────────────────────────────────────────

/// SMTP configuration, deserialized from the Lua `smtp` config block.
///
/// Can be global (shared by all profiles) or per-profile (overrides global).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SmtpConfig {
    /// SMTP server hostname (e.g. "smtp.gmail.com")
    pub host: String,

    /// Port number.  Defaults to 587 (STARTTLS).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Username for SMTP AUTH.
    pub username: String,

    /// Password string.  May be a literal or an `eval:` / `sh:` prefix
    /// that was resolved at config-load time.
    #[serde(default)]
    pub password: String,

    /// Use STARTTLS (port 587).  If false, use SMTPS wrapper TLS (port 465).
    #[serde(default = "default_starttls")]
    pub starttls: bool,
}

fn default_port() -> u16 {
    587
}
fn default_starttls() -> bool {
    true
}

impl SmtpConfig {
    /// Resolve the password at call time (not at config-load time).
    ///
    /// If `password` starts with `eval:` or `sh:`, the remainder is
    /// executed via `sh -c` and the stdout (trimmed) is returned.
    /// Otherwise the literal password string is returned as-is.
    ///
    /// Evaluating at send time (not load time) means rotating tokens
    /// from `pass` or similar are always fresh.
    pub fn resolved_password(&self) -> Result<String, SendError> {
        let evaluated = if let Some(cmd) = self.password.strip_prefix("eval:") {
            Some(cmd.to_string())
        } else if let Some(cmd) = self.password.strip_prefix("sh:") {
            Some(cmd.to_string())
        } else {
            None
        };

        match evaluated {
            Some(cmd) => {
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .output()
                    .map_err(|e| SendError::CommandEval(e.to_string()))?;

                if !output.status.success() {
                    return Err(SendError::CommandEval(format!(
                        "password command exited with status {}: {}",
                        output.status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&output.stderr).trim()
                    )));
                }

                Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
            }
            None => Ok(self.password.clone()),
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────

/// Send an RFC 2822 message via SMTP.
///
/// `rfc2822` is the complete message text (headers + blank line + body).
/// `envelope_from` and `envelope_to` are used for the SMTP envelope
/// (they may differ from the message headers, e.g. Bcc recipients).
///
/// The call blocks until the message is sent or an error occurs.
/// Safe to call from the Lua thread (which is already blocking).
pub fn send(
    config: &SmtpConfig,
    envelope_from: &str,
    envelope_to: &[&str],
    rfc2822: &[u8],
) -> Result<(), SendError> {
    // Build the SMTP envelope
    let from = envelope_from
        .parse::<lettre::Address>()
        .map_err(|e| SendError::Address(format!("invalid from address '{}': {}", envelope_from, e)))?;

    let to: Vec<lettre::Address> = envelope_to
        .iter()
        .map(|addr| {
            addr.parse::<lettre::Address>()
                .map_err(|e| SendError::Address(format!("invalid to address '{}': {}", addr, e)))
        })
        .collect::<Result<Vec<_>, SendError>>()?;

    let envelope = Envelope::new(Some(from), to)
        .map_err(|e| SendError::Address(format!("envelope error: {}", e)))?;

    // Build the transport
    let builder = if config.starttls {
        SmtpTransport::starttls_relay(&config.host)
            .map_err(|e| SendError::Smtp(format!("TLS setup failed: {}", e)))?
    } else {
        SmtpTransport::relay(&config.host)
            .map_err(|e| SendError::Smtp(format!("TLS setup failed: {}", e)))?
    };

    let transport = builder
        .port(config.port)
        .credentials(Credentials::new(
            config.username.clone(),
            config.resolved_password()?,
        ))
        .timeout(Some(Duration::from_secs(60)))
        .build();

    // Send
    transport
        .send_raw(&envelope, rfc2822)
        .map_err(|e| SendError::Smtp(e.to_string()))?;

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"user@example.com","password":"secret"}"#
        ).unwrap();
        assert_eq!(config.port, 587);
        assert!(config.starttls);
    }

    #[test]
    fn explicit_port_and_starttls() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","port":465,"username":"u","password":"p","starttls":false}"#
        ).unwrap();
        assert_eq!(config.port, 465);
        assert!(!config.starttls);
    }

    #[test]
    fn password_literal() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"u","password":"literal_secret"}"#
        ).unwrap();
        assert_eq!(config.resolved_password().unwrap(), "literal_secret");
    }

    #[test]
    fn password_eval_prefix() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"u","password":"eval:echo hello_secret"}"#
        ).unwrap();
        assert_eq!(config.resolved_password().unwrap(), "hello_secret");
    }

    #[test]
    fn password_sh_prefix() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"u","password":"sh:echo sh_secret"}"#
        ).unwrap();
        assert_eq!(config.resolved_password().unwrap(), "sh_secret");
    }

    #[test]
    fn password_eval_trims_trailing_newline() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"u","password":"eval:echo trimmed"}"#
        ).unwrap();
        assert_eq!(config.resolved_password().unwrap(), "trimmed");
    }

    #[test]
    fn password_eval_failure() {
        let config: SmtpConfig = serde_json::from_str(
            r#"{"host":"smtp.example.com","username":"u","password":"eval:false"}"#
        ).unwrap();
        assert!(config.resolved_password().is_err());
    }

    #[test]
    fn address_parsing() {
        let addr: Result<lettre::Address, _> = "user@example.com".parse();
        assert!(addr.is_ok());
    }

    #[test]
    fn address_parsing_invalid() {
        let addr: Result<lettre::Address, _> = "not an email".parse();
        assert!(addr.is_err());
    }
}