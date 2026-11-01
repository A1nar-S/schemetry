//! The "what was this call about" summary of an audit entry.
//!
//! It lives under the `target` key of the entry's `arguments` JSON, which is already part
//! of the hashed payload, so adding it changes neither the hash format nor how existing
//! entries verify. Everything here is bounded: the log is append-only forever.

use serde_json::{Map, Value, json};

/// Names (tables, connections, …) kept per entry.
pub const MAX_TARGET_NAMES: usize = 20;
/// Selected column names kept per entry; the full count is stored next to them.
pub const MAX_TARGET_COLUMNS: usize = 30;
const MAX_NAME_CHARS: usize = 128;
const MAX_TYPE_CHARS: usize = 32;
const MAX_SCANNED_NAMES: usize = 500;
/// Key under which the summary is stored inside `arguments`.
pub const TARGET_KEY: &str = "target";

fn clip(name: &str) -> String {
    let name = name.trim();
    if name.chars().count() <= MAX_NAME_CHARS {
        name.to_string()
    } else {
        name.chars().take(MAX_NAME_CHARS).collect::<String>() + "…"
    }
}

/// Distinct, trimmed, non-empty names in first-seen order (case-insensitive). Only the
/// first `MAX_SCANNED_NAMES` inputs are looked at, so a huge request can't stall the audit.
fn distinct<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names.into_iter().take(MAX_SCANNED_NAMES) {
        let name = name.as_ref().trim();
        if !name.is_empty() && !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// `{ "object_type": …, "names": [...] }`, plus `names_total` when `names` was cut short.
pub fn object_target<S: AsRef<str>>(
    object_type: &str,
    names: impl IntoIterator<Item = S>,
) -> Map<String, Value> {
    let all = distinct(names);
    let mut target = Map::new();
    target.insert(
        "object_type".into(),
        json!(
            object_type
                .trim()
                .chars()
                .take(MAX_TYPE_CHARS)
                .collect::<String>()
        ),
    );
    if all.len() > MAX_TARGET_NAMES {
        target.insert("names_total".into(), json!(all.len()));
    }
    let kept: Vec<String> = all.iter().take(MAX_TARGET_NAMES).map(|n| clip(n)).collect();
    target.insert("names".into(), json!(kept));
    target
}

/// Adds the selected columns (bounded list + total count) to a target.
pub fn set_columns<S: AsRef<str>>(target: &mut Map<String, Value>, columns: &[S]) {
    let kept: Vec<String> = columns
        .iter()
        .take(MAX_TARGET_COLUMNS)
        .map(|c| clip(c.as_ref()))
        .collect();
    target.insert("column_count".into(), json!(columns.len()));
    target.insert("columns".into(), json!(kept));
}

/// A short human-readable form such as `TABLE EMP, DEPT`, derived from an entry's
/// `arguments` text; empty when the entry has no target (older entries, or tools that
/// have none).
pub fn summarize(arguments: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(arguments) else {
        return String::new();
    };
    let Some(Value::Object(target)) = map.get(TARGET_KEY) else {
        return String::new();
    };
    let names: Vec<&str> = target
        .get("names")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return String::new();
    }
    let total = target
        .get("names_total")
        .and_then(Value::as_u64)
        .unwrap_or(names.len() as u64);
    let mut text = names.join(", ");
    if total > names.len() as u64 {
        text.push_str(&format!(" (+{} more)", total - names.len() as u64));
    }
    match target.get("object_type").and_then(Value::as_str) {
        Some(kind) if !kind.is_empty() => format!("{kind} {text}"),
        _ => text,
    }
}

#[cfg(test)]
#[path = "tests/mcp_audit_target.rs"]
mod tests;
