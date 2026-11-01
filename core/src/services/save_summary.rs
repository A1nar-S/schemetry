//! Content-free description of the files a DDL save would write, for MCP responses.

use std::fs;

use serde_json::{Value, json};
use similar::{Algorithm, DiffOp, capture_diff_slices};

use crate::services::ddl_save::{PlannedFile, decode_content};
use crate::services::mcp_gateway::sha256_hex;

/// Rendered content is only returned on request and only below this size.
pub const MAX_INCLUDED_CONTENT_BYTES: usize = 64 * 1024;
/// Existing files above this size are not diffed.
const MAX_DIFFED_EXISTING_BYTES: u64 = 5 * 1024 * 1024;

/// `CRLF`, `LF` or `mixed`; `none` when there is no line break at all. A lone `\r` is ignored.
pub fn line_ending_style(bytes: &[u8]) -> &'static str {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if index > 0 && bytes[index - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    match (crlf, lf) {
        (0, 0) => "none",
        (_, 0) => "CRLF",
        (0, _) => "LF",
        _ => "mixed",
    }
}

/// Number of lines; an unterminated last line counts.
pub fn line_count(bytes: &[u8]) -> usize {
    let breaks = bytes.iter().filter(|b| **b == b'\n').count();
    match bytes.last() {
        None => 0,
        Some(b'\n') => breaks,
        Some(_) => breaks + 1,
    }
}

/// Added/removed line counts between two texts; line endings do not count as changes.
pub fn line_diff_counts(old: &str, new: &str) -> (usize, usize) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (mut added, mut removed) = (0, 0);
    for op in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { old_len, .. } => removed += old_len,
            DiffOp::Insert { new_len, .. } => added += new_len,
            DiffOp::Replace {
                old_len, new_len, ..
            } => {
                removed += old_len;
                added += new_len;
            }
        }
    }
    (added, removed)
}

/// Compares the planned bytes with whatever is on disk at the same path.
fn diff_against_disk(file: &PlannedFile) -> Value {
    let unavailable = |reason: String| json!({ "unavailable": reason });
    match fs::metadata(&file.path) {
        Ok(meta) if meta.len() > MAX_DIFFED_EXISTING_BYTES => {
            return unavailable(format!(
                "existing file is larger than {MAX_DIFFED_EXISTING_BYTES} bytes"
            ));
        }
        Ok(_) => {}
        Err(error) => return unavailable(format!("cannot read existing file: {error}")),
    }
    let existing = match fs::read(&file.path) {
        Ok(bytes) => bytes,
        Err(error) => return unavailable(format!("cannot read existing file: {error}")),
    };

    let (old_text, old_lossy) = decode_content(&existing, file.encoding);
    let (new_text, _) = decode_content(&file.content, file.encoding);
    let (added, removed) = line_diff_counts(&old_text, &new_text);
    json!({
        "added": added,
        "removed": removed,
        "identical": existing == file.content,
        "existing_size": existing.len(),
        "existing_sha256": sha256_hex(&existing),
        "existing_line_ending": line_ending_style(&existing),
        // The on-disk bytes are not valid in the target encoding, so the counts are approximate.
        "existing_not_valid_in_encoding": old_lossy,
    })
}

/// Describes one planned file without echoing its content (unless `include_content` and small).
pub fn describe_planned_file(file: &PlannedFile, include_content: bool) -> Value {
    let mut entry = json!({
        "kind": file.kind,
        "path": file.path.to_string_lossy(),
        "overwrites_existing": file.exists,
        "size": file.content.len(),
        "sha256": sha256_hex(&file.content),
        "lines": line_count(&file.content),
        "encoding": file.encoding,
        "line_ending": line_ending_style(&file.content),
    });
    if file.exists {
        entry["diff"] = diff_against_disk(file);
    }
    if include_content {
        if file.content.len() <= MAX_INCLUDED_CONTENT_BYTES {
            entry["content"] = Value::String(decode_content(&file.content, file.encoding).0);
        } else {
            entry["content_omitted"] = Value::String(format!(
                "content exceeds {MAX_INCLUDED_CONTENT_BYTES} bytes"
            ));
        }
    }
    entry
}

#[cfg(test)]
#[path = "tests/save_summary.rs"]
mod tests;
