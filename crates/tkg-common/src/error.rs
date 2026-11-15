//! Error types for tkg-common and `TorqKage`.

use std::path::Path;

use thiserror::Error;

/// Core error enumeration across `TorqKage`.
#[derive(Debug, Error)]
pub enum TkgError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Configuration parse error in '{path}'{}: {message}", format_line_col(*.line_col))]
    ConfigParse {
        path: String,
        line_col: Option<(usize, usize)>,
        message: String,
    },

    #[error("Workspace error: {0}")]
    Workspace(String),

    #[error("Process execution error: {0}")]
    Process(String),

    #[error("Admission gate blocked: {0}")]
    GateBlocked(String),
}

fn format_line_col(line_col: Option<(usize, usize)>) -> String {
    line_col.map_or_default(|(line, col)| format!(" at line {line}, column {col}"))
}

fn compute_line_col(content: &str, byte_offset: usize) -> (usize, usize, &str) {
    let clamped = byte_offset.min(content.len());
    let mut byte_pos = 0_usize;
    let mut last_match = (1_usize, 1_usize, "");

    for (idx, raw_line) in content.split('\n').enumerate() {
        let clean_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line_end = byte_pos.saturating_add(raw_line.len());
        let line_no = idx.saturating_add(1);
        let col_no = content
            .get(byte_pos..clamped.min(line_end))
            .map_or(1, |slice| slice.chars().count().saturating_add(1));
        last_match = (line_no, col_no, clean_line);
        if clamped <= line_end {
            return last_match;
        }
        // Account for the '\n' delimiter
        byte_pos = line_end.saturating_add(1);
    }

    last_match
}

impl TkgError {
    /// Converts a [`toml::de::Error`] into a [`TkgError::ConfigParse`] with
    /// 1-based line/column numbers and a caret-annotated snippet pointing at
    /// the syntax error.
    #[must_use]
    pub fn from_toml(path: Option<&Path>, content: &str, err: &toml::de::Error) -> Self {
        let path_display = path.map_or_else(|| "<inline>".to_owned(), |p| p.display().to_string());
        let err_msg = err.message();
        let (line_col, message) = err.span().map_or_else(
            || (None, err_msg.to_owned()),
            |span| {
                let (line_no, col_no, line_text) = compute_line_col(content, span.start);
                let line_label = line_no.to_string();
                let pad = " ".repeat(line_label.len());
                let caret_pad = " ".repeat(col_no.saturating_sub(1));
                (
                    Some((line_no, col_no)),
                    format!(
                        "{err_msg}\n{pad} --> {path_display}:{line_no}:{col_no}\n{pad} |\n{line_label} | {line_text}\n{pad} | {caret_pad}^"
                    ),
                )
            },
        );

        Self::ConfigParse {
            path: path_display,
            line_col,
            message,
        }
    }
}

/// Convenience alias for `Result<T, TkgError>`.
pub type Result<T> = std::result::Result<T, TkgError>;
