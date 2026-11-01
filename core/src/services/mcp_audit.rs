//! Tamper-evident audit trail of MCP tool calls.
//!
//! Every entry stores `hash = sha256(prev_hash || canonical(entry))`, chaining it to the
//! one before. Together with SQLite triggers that reject UPDATE/DELETE and the absence of
//! any purge path in the app, editing or removing entries is detectable by
//! [`McpAuditService::verify`]. A machine's owner can still rewrite the whole file, so a
//! copy forwarded to an external sink (see `mcp_audit_sink`) is what gives true immutability.

use std::sync::Arc;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::models::{McpAuditEntry, McpAuditFilter, McpAuditVerification, McpOutcome};
use crate::repositories::mcp_audit_repository::{GENESIS_HASH, McpAuditRepository};
use crate::services::mcp_audit_sink;
use crate::services::mcp_audit_target;

/// Everything about a call except the fields the chain assigns (id, hash, prev_hash).
#[derive(Clone, Debug)]
pub struct McpAuditDraft {
    pub tool: String,
    pub connection_id: Option<i64>,
    pub connection_name: String,
    pub arguments: String,
    pub outcome: McpOutcome,
    pub reason: String,
    pub row_count: Option<i64>,
    pub bytes: i64,
    pub duration_ms: i64,
    pub client_name: String,
}

/// The hash an entry must carry given its predecessor's hash. Fields go through a JSON
/// array so no field value can masquerade as a separator.
pub fn entry_hash(prev_hash: &str, entry: &McpAuditEntry) -> String {
    let canonical = serde_json::json!([
        entry.id,
        entry.timestamp,
        entry.tool,
        entry.connection_id,
        entry.connection_name,
        entry.arguments,
        entry.outcome.as_str(),
        entry.reason,
        entry.row_count,
        entry.bytes,
        entry.duration_ms,
        entry.client_name,
    ]);
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct McpAuditService {
    repo: Arc<dyn McpAuditRepository>,
    forward_to_sink: bool,
}

impl McpAuditService {
    pub fn new(repo: Arc<dyn McpAuditRepository>) -> Self {
        Self {
            repo,
            forward_to_sink: true,
        }
    }

    /// For tests and the GUI, which never forward anything.
    pub fn without_forwarding(mut self) -> Self {
        self.forward_to_sink = false;
        self
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    /// Appends an entry; the external sink copy is best effort and never fails the call.
    pub fn record(&self, draft: McpAuditDraft) -> Result<McpAuditEntry> {
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let entry = self.repo.append_chained(&mut |id, prev_hash| {
            let mut entry = McpAuditEntry {
                id,
                timestamp: timestamp.clone(),
                tool: draft.tool.clone(),
                connection_id: draft.connection_id,
                connection_name: draft.connection_name.clone(),
                arguments: draft.arguments.clone(),
                outcome: draft.outcome,
                reason: draft.reason.clone(),
                row_count: draft.row_count,
                bytes: draft.bytes,
                duration_ms: draft.duration_ms,
                client_name: draft.client_name.clone(),
                prev_hash: prev_hash.to_string(),
                hash: String::new(),
            };
            entry.hash = entry_hash(prev_hash, &entry);
            entry
        })?;

        if self.forward_to_sink
            && let Err(error) = mcp_audit_sink::forward(&entry)
        {
            eprintln!("MCP audit sink: {error:#}");
        }
        Ok(entry)
    }

    pub fn list(&self, filter: &McpAuditFilter) -> Result<Vec<McpAuditEntry>> {
        self.repo.list(filter)
    }

    pub fn count(&self, filter: &McpAuditFilter) -> Result<i64> {
        self.repo.count(filter)
    }

    /// Walks the whole chain and reports the first inconsistency, if any.
    pub fn verify(&self) -> Result<McpAuditVerification> {
        let mut expected_id = 1i64;
        let mut prev_hash = GENESIS_HASH.to_string();
        let mut entries = 0i64;
        let mut failure: Option<(i64, String)> = None;

        self.repo.for_each_in_order(&mut |entry| {
            if entry.id != expected_id {
                failure = Some((
                    entry.id,
                    format!(
                        "Entry {expected_id} is missing (found {} next) — entries were removed.",
                        entry.id
                    ),
                ));
                return false;
            }
            if entry.prev_hash != prev_hash {
                failure = Some((
                    entry.id,
                    format!(
                        "Entry {} does not follow the previous entry's hash.",
                        entry.id
                    ),
                ));
                return false;
            }
            if entry_hash(&entry.prev_hash, &entry) != entry.hash {
                failure = Some((
                    entry.id,
                    format!("Entry {} was modified (hash mismatch).", entry.id),
                ));
                return false;
            }
            prev_hash = entry.hash;
            expected_id += 1;
            entries += 1;
            true
        })?;

        if let Some((id, problem)) = failure {
            return Ok(McpAuditVerification {
                ok: false,
                entries,
                problem: Some(problem),
                first_bad_id: Some(id),
            });
        }

        let issued = self.repo.issued_sequence()?;
        if issued > entries {
            return Ok(McpAuditVerification {
                ok: false,
                entries,
                problem: Some(format!(
                    "The log was truncated: {issued} entries were written but only {entries} remain."
                )),
                first_bad_id: Some(entries + 1),
            });
        }

        Ok(McpAuditVerification {
            ok: true,
            entries,
            problem: None,
            first_bad_id: None,
        })
    }

    /// Every entry matching `filter` (no paging), oldest first, as RFC 4180 CSV.
    pub fn export_csv(&self, filter: &McpAuditFilter) -> Result<String> {
        let mut all = McpAuditFilter {
            limit: Some(10_000),
            offset: Some(0),
            ..filter.clone()
        };
        let mut entries = Vec::new();
        loop {
            let page = self.repo.list(&all)?;
            let fetched = page.len();
            entries.extend(page);
            if fetched < 10_000 {
                break;
            }
            all.offset = Some(all.offset.unwrap_or(0) + 10_000);
        }
        entries.reverse();

        let mut csv = String::from(
            "id,timestamp,tool,connection_id,connection_name,client,outcome,reason,\
             row_count,bytes,duration_ms,target,arguments,prev_hash,hash\r\n",
        );
        for entry in &entries {
            let fields = [
                entry.id.to_string(),
                entry.timestamp.clone(),
                entry.tool.clone(),
                entry
                    .connection_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                entry.connection_name.clone(),
                entry.client_name.clone(),
                entry.outcome.as_str().to_string(),
                entry.reason.clone(),
                entry.row_count.map(|n| n.to_string()).unwrap_or_default(),
                entry.bytes.to_string(),
                entry.duration_ms.to_string(),
                mcp_audit_target::summarize(&entry.arguments),
                entry.arguments.clone(),
                entry.prev_hash.clone(),
                entry.hash.clone(),
            ];
            let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
            csv.push_str(&line.join(","));
            csv.push_str("\r\n");
        }
        Ok(csv)
    }
}

/// Quotes a field and neutralises spreadsheet formula injection (`=`, `+`, `-`, `@`
/// leading characters) since arguments contain agent-controlled text.
fn csv_field(value: &str) -> String {
    let guarded = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) || guarded.starts_with('\'') {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

#[cfg(test)]
#[path = "tests/mcp_audit.rs"]
mod tests;
