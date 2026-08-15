/// Destination for log output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogDestination {
    /// Write logs to a file.
    File,
    /// Write logs to stderr.
    Stderr,
}

/// Configuration for initializing the logger.
#[derive(Debug, Clone, Copy)]
pub struct LogConfig {
    /// Whether the caller is the CLI. When true, logs are written to a separate subdirectory
    /// with a higher rotation limit so that CLI invocations don't evict GUI application logs.
    pub is_cli: bool,
    /// The destination for log output. If `None`, the destination is inferred from the environment.
    pub log_destination: Option<LogDestination>,
}

#[cfg_attr(not(target_family = "wasm"), path = "native.rs")]
#[cfg_attr(target_family = "wasm", path = "wasm.rs")]
mod imp;

#[cfg(not(target_family = "wasm"))]
pub use imp::{
    ExtraFile, InlineFile, LogBundleExtras, create_log_bundle_zip, default_log_bundle_filename,
    log_directory, log_file_path, rotate_log_files, write_log_bundle_zip_to,
};
pub use imp::{diagnostic_logging_enabled, init, set_diagnostic_logging_enabled};

#[cfg(not(target_family = "wasm"))]
pub use imp::{
    init_for_crash_recovery_process, init_logging_for_unit_tests, on_crash_recovery_process_killed,
    on_parent_process_crash,
};

const REDACTED: &str = "[REDACTED]";
const SENSITIVE_FIELD_NAMES: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "client_secret",
    "credential",
    "forked_from_server_conversation_token",
    "database_url",
    "passphrase",
    "password",
    "private_key",
    "refresh_token",
    "server_conversation_token",
    "secret",
    "token",
];

pub fn redact_sensitive_text(text: &str) -> String {
    let mut redacted = redact_bearer_tokens(text);
    for field in SENSITIVE_FIELD_NAMES {
        redacted = redact_field_values(&redacted, field);
    }
    redacted
}

fn redact_field_values(text: &str, field: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative_index) = lower[cursor..].find(field) {
        let field_start = cursor + relative_index;
        let field_end = field_start + field.len();
        let before = field_start
            .checked_sub(1)
            .and_then(|index| lower.as_bytes().get(index));
        let after = lower.as_bytes().get(field_end);
        if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
            || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
        {
            output.push_str(&text[cursor..field_end]);
            cursor = field_end;
            continue;
        }

        let bytes = text.as_bytes();
        let mut separator = field_end;
        while matches!(bytes.get(separator), Some(b' ' | b'\t' | b'"' | b'\'')) {
            separator += 1;
        }
        if !matches!(bytes.get(separator), Some(b'=' | b':')) {
            output.push_str(&text[cursor..field_end]);
            cursor = field_end;
            continue;
        }

        let mut value_start = separator + 1;
        while matches!(bytes.get(value_start), Some(b' ' | b'\t')) {
            value_start += 1;
        }
        let quote = match bytes.get(value_start) {
            Some(b'"') => {
                value_start += 1;
                Some(b'"')
            }
            Some(b'\'') => {
                value_start += 1;
                Some(b'\'')
            }
            _ => None,
        };

        let mut value_end = value_start;
        while let Some(ch) = bytes.get(value_end) {
            if quote.is_some_and(|quote| *ch == quote)
                || (quote.is_none()
                    && matches!(ch, b',' | b';' | b'&' | b' ' | b'\t' | b'\r' | b'\n'))
            {
                break;
            }
            value_end += 1;
        }

        output.push_str(&text[cursor..value_start]);
        output.push_str(REDACTED);
        cursor = value_end;
    }
    output.push_str(&text[cursor..]);
    output
}

fn redact_bearer_tokens(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative_index) = lower[cursor..].find("bearer ") {
        let token_start = cursor + relative_index + "bearer ".len();
        let mut token_end = token_start;
        while let Some(ch) = text.as_bytes().get(token_end) {
            if matches!(
                ch,
                b',' | b';' | b'&' | b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'\''
            ) {
                break;
            }
            token_end += 1;
        }
        output.push_str(&text[cursor..token_start]);
        output.push_str(REDACTED);
        cursor = token_end;
    }
    output.push_str(&text[cursor..]);
    output
}

pub fn diagnostic_text_preview(text: &str, max_chars: usize) -> String {
    let text = redact_sensitive_text(text);
    let mut preview = String::new();
    let mut truncated = false;
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            truncated = true;
            break;
        }
        match ch {
            '\n' => preview.push_str("\\n"),
            '\r' => preview.push_str("\\r"),
            '\t' => preview.push_str("\\t"),
            ch if ch.is_control() => {
                preview.push_str(&format!("\\u{{{:x}}}", ch as u32));
            }
            ch => preview.push(ch),
        }
    }
    if truncated {
        preview.push_str("...");
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_preview_redacts_secret_values() {
        let preview = diagnostic_text_preview("API_KEY=abc123 password: \"hunter2\"", 200);
        assert!(preview.contains("API_KEY=[REDACTED]"));
        assert!(preview.contains("password: \"[REDACTED]\""));
        assert!(!preview.contains("abc123"));
        assert!(!preview.contains("hunter2"));
    }

    #[test]
    fn redacts_bearer_tokens() {
        let redacted = redact_sensitive_text("Authorization: Bearer token-value");
        assert!(redacted.contains("Authorization: [REDACTED]"));
        assert!(!redacted.contains("token-value"));
    }
}
