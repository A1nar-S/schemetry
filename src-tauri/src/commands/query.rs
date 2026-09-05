use base64::Engine;
use serde::Serialize;
use tauri::State;
use tokio;

use crate::models::{QueryHistoryEntry, QueryServerResult};
use crate::repositories::oracle_repository::LobCell;
use crate::services;
use crate::state::AppState;

#[tauri::command]
pub async fn run_query(
    state: State<'_, AppState>,
    sql: String,
    server_names: Vec<String>,
    materialize_lobs: bool,
) -> Result<Vec<QueryServerResult>, String> {
    let selected = super::selected_connections(&state.catalog, &server_names)?;
    let trimmed = sql.trim().to_string();
    if trimmed.is_empty() {
        return Err("SQL cannot be empty.".to_string());
    }
    let query_svc = std::sync::Arc::clone(&state.query_svc);
    let history_svc = std::sync::Arc::clone(&state.query_history_svc);

    tokio::task::spawn_blocking(move || {
        let results = query_svc.run_query_on_servers(&selected, &trimmed, materialize_lobs);
        let _ = history_svc.append_query(&trimmed);
        Ok(results)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// On-demand content of a single LOB cell, returned to the viewer.
#[derive(Serialize)]
pub struct LobContent {
    /// `"text"` for CLOB-like columns, `"binary"` otherwise.
    pub kind: String,
    /// Text content (for `kind == "text"`), capped at a character limit.
    pub text: Option<String>,
    /// Detected MIME type (for `kind == "binary"`).
    pub mime: Option<String>,
    /// Base64-encoded bytes (for `kind == "binary"`), capped at the preview limit.
    pub base64: Option<String>,
    /// Number of bytes/chars returned.
    pub size: u64,
    /// True if the content was capped (more data exists than was returned).
    pub truncated: bool,
}

/// Fetch a LOB cell's content for the viewer: full text for CLOB-like columns, or
/// type-sniffed binary (base64) for BLOBs. Re-runs the query on the given server.
#[tauri::command]
pub async fn fetch_lob_content(
    state: State<'_, AppState>,
    server_name: String,
    sql: String,
    row_index: usize,
    col_index: usize,
) -> Result<LobContent, String> {
    // Cap inline content so the base64 round-trip stays reasonable; larger BLOBs fall
    // back to "Save to file". Text LOBs are capped by character count.
    const MAX_BINARY: usize = 16 * 1024 * 1024;
    const MAX_TEXT_CHARS: usize = 1024 * 1024;

    let selected = super::selected_connections(&state.catalog, &[server_name])?;
    let conn = selected
        .into_iter()
        .next()
        .ok_or_else(|| "Server connection not found.".to_string())?;
    let query_svc = std::sync::Arc::clone(&state.query_svc);

    tokio::task::spawn_blocking(move || {
        // Fetch one extra byte so we can tell whether the binary content was truncated.
        let cell = query_svc
            .fetch_lob_cell(&conn, &sql, row_index, col_index, MAX_BINARY + 1)
            .map_err(|e| e.to_string())?;

        let content = match cell {
            LobCell::Text(text) => {
                let raw = text.unwrap_or_default();
                let truncated = raw.chars().count() > MAX_TEXT_CHARS;
                let text: String = if truncated {
                    raw.chars().take(MAX_TEXT_CHARS).collect()
                } else {
                    raw
                };
                LobContent {
                    kind: "text".to_string(),
                    size: text.chars().count() as u64,
                    text: Some(text),
                    mime: None,
                    base64: None,
                    truncated,
                }
            }
            LobCell::Binary(mut bytes) => {
                let truncated = bytes.len() > MAX_BINARY;
                if truncated {
                    bytes.truncate(MAX_BINARY);
                }
                LobContent {
                    kind: "binary".to_string(),
                    mime: Some(sniff_mime(&bytes).to_string()),
                    size: bytes.len() as u64,
                    base64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
                    text: None,
                    truncated,
                }
            }
        };
        Ok(content)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Write the full (untruncated) bytes of a binary LOB cell to `path`.
#[tauri::command]
pub async fn save_blob_to_file(
    state: State<'_, AppState>,
    server_name: String,
    sql: String,
    row_index: usize,
    col_index: usize,
    path: String,
) -> Result<u64, String> {
    let selected = super::selected_connections(&state.catalog, &[server_name])?;
    let conn = selected
        .into_iter()
        .next()
        .ok_or_else(|| "Server connection not found.".to_string())?;
    let query_svc = std::sync::Arc::clone(&state.query_svc);

    tokio::task::spawn_blocking(move || {
        let bytes = query_svc
            .fetch_blob_cell(&conn, &sql, row_index, col_index, usize::MAX)
            .map_err(|e| e.to_string())?;
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        Ok(bytes.len() as u64)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Best-effort content-type sniffing from leading magic bytes, falling back to a
/// printable-text check and finally `application/octet-stream`.
fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"%PDF") {
        "application/pdf"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if looks_like_text(bytes) {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

/// Heuristic: valid UTF-8 with no NULs and few control characters → treat as text.
fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    match std::str::from_utf8(sample) {
        Ok(s) => !s.chars().any(|c| c == '\0'),
        Err(_) => false,
    }
}

#[tauri::command]
pub fn get_query_history(state: State<AppState>) -> Result<Vec<QueryHistoryEntry>, String> {
    state
        .query_history_svc
        .load_history()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_query_history_item(state: State<AppState>, id: i64) -> Result<(), String> {
    state
        .query_history_svc
        .delete_query(id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pin_query_history_item(state: State<AppState>, id: i64, pinned: bool) -> Result<(), String> {
    state
        .query_history_svc
        .pin_query(id, pinned)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_query_favorite(state: State<AppState>, id: i64, favorite: bool, description: String) -> Result<(), String> {
    state
        .query_history_svc
        .set_favorite(id, favorite, &description)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reorder_favorites(state: State<AppState>, ordered_ids: Vec<i64>) -> Result<(), String> {
    state
        .query_history_svc
        .reorder_favorites(&ordered_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_query_history(state: State<AppState>) -> Result<(), String> {
    state
        .query_history_svc
        .clear_history()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_query_results(
    results: Vec<QueryServerResult>,
    output_path: String,
    single_sheet: bool,
) -> Result<(), String> {
    services::query_export::export_to_excel(&results, &output_path, single_sheet)
        .map_err(|e| e.to_string())
}
