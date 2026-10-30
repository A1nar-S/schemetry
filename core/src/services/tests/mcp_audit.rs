use std::sync::Arc;

use rusqlite::Connection;

use super::*;
use crate::repositories::mcp_audit_repository::SqliteMcpAuditRepository;
use crate::test_support::MemoryAudit;

fn draft(tool: &str, outcome: McpOutcome) -> McpAuditDraft {
    McpAuditDraft {
        tool: tool.to_string(),
        connection_id: Some(1),
        connection_name: "Prod".to_string(),
        arguments: "{\"a\":1}".to_string(),
        outcome,
        reason: String::new(),
        row_count: Some(3),
        bytes: 120,
        duration_ms: 7,
        client_name: "claude-code 1.0".to_string(),
    }
}

fn memory_service() -> (McpAuditService, Arc<MemoryAudit>) {
    let repo = Arc::new(MemoryAudit::default());
    (
        McpAuditService::new(repo.clone()).without_forwarding(),
        repo,
    )
}

/// A real SQLite-backed service in its own file, plus the path for tampering.
fn sqlite_service() -> (McpAuditService, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    let svc = McpAuditService::new(Arc::new(SqliteMcpAuditRepository::with_path(path.clone())))
        .without_forwarding();
    svc.init_db().unwrap();
    (svc, path, dir)
}

#[test]
fn entries_form_a_hash_chain_starting_from_the_genesis_hash() {
    let (svc, repo) = memory_service();
    let first = svc
        .record(draft("list_connections", McpOutcome::Allowed))
        .unwrap();
    let second = svc
        .record(draft("query_table", McpOutcome::Denied))
        .unwrap();

    assert_eq!(first.id, 1);
    assert_eq!(first.prev_hash, GENESIS_HASH);
    assert_eq!(second.id, 2);
    assert_eq!(second.prev_hash, first.hash);
    assert_ne!(first.hash, second.hash);
    assert_eq!(first.hash, entry_hash(&first.prev_hash, &first));
    assert_eq!(repo.entries.lock().unwrap().len(), 2);
    assert!(svc.verify().unwrap().ok);
}

#[test]
fn every_field_feeds_the_hash() {
    let (svc, _) = memory_service();
    let base = svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    let hash = |mutate: &dyn Fn(&mut McpAuditEntry)| {
        let mut copy = base.clone();
        mutate(&mut copy);
        entry_hash(&copy.prev_hash, &copy)
    };
    assert_eq!(hash(&|_| {}), base.hash);
    assert_ne!(hash(&|e| e.id += 1), base.hash);
    assert_ne!(hash(&|e| e.timestamp.push('x')), base.hash);
    assert_ne!(hash(&|e| e.tool.push('x')), base.hash);
    assert_ne!(hash(&|e| e.connection_id = Some(99)), base.hash);
    assert_ne!(hash(&|e| e.connection_name.push('x')), base.hash);
    assert_ne!(hash(&|e| e.arguments.push('x')), base.hash);
    assert_ne!(hash(&|e| e.outcome = McpOutcome::Error), base.hash);
    assert_ne!(hash(&|e| e.reason.push('x')), base.hash);
    assert_ne!(hash(&|e| e.row_count = None), base.hash);
    assert_ne!(hash(&|e| e.bytes += 1), base.hash);
    assert_ne!(hash(&|e| e.duration_ms += 1), base.hash);
    assert_ne!(hash(&|e| e.client_name.push('x')), base.hash);
}

#[test]
fn fields_cannot_be_shifted_across_each_other() {
    let (svc, _) = memory_service();
    let mut a = draft("ab", McpOutcome::Allowed);
    a.reason = "c".into();
    let mut b = draft("a", McpOutcome::Allowed);
    b.reason = "bc".into();
    let (ea, eb) = (svc.record(a).unwrap(), svc.record(b).unwrap());
    // Same concatenated text, different split: the hashes must differ.
    let rehash =
        |e: &McpAuditEntry| entry_hash(GENESIS_HASH, &McpAuditEntry { id: 1, ..e.clone() });
    assert_ne!(rehash(&ea), rehash(&eb));
}

#[test]
fn verification_detects_a_modified_entry() {
    let (svc, repo) = memory_service();
    for _ in 0..3 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }
    repo.entries.lock().unwrap()[1].outcome = McpOutcome::Denied;

    let verdict = svc.verify().unwrap();
    assert!(!verdict.ok);
    assert_eq!(verdict.first_bad_id, Some(2));
    assert!(verdict.problem.unwrap().contains("modified"));
}

#[test]
fn verification_detects_a_removed_middle_entry() {
    let (svc, repo) = memory_service();
    for _ in 0..3 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }
    repo.entries.lock().unwrap().remove(1);

    let verdict = svc.verify().unwrap();
    assert!(!verdict.ok);
    assert_eq!(verdict.first_bad_id, Some(3));
    assert!(verdict.problem.unwrap().contains("missing"));
}

#[test]
fn verification_detects_a_removed_first_entry() {
    let (svc, repo) = memory_service();
    for _ in 0..2 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }
    repo.entries.lock().unwrap().remove(0);
    assert!(!svc.verify().unwrap().ok);
}

#[test]
fn verification_detects_a_rewritten_chain_link() {
    let (svc, repo) = memory_service();
    for _ in 0..3 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }
    {
        let mut entries = repo.entries.lock().unwrap();
        // Rewrite entry 2 consistently (new content, valid own hash) — entry 3 no longer links.
        entries[1].reason = "rewritten".into();
        let prev = entries[1].prev_hash.clone();
        entries[1].hash = entry_hash(&prev, &entries[1]);
    }
    let verdict = svc.verify().unwrap();
    assert!(!verdict.ok);
    assert_eq!(verdict.first_bad_id, Some(3));
}

#[test]
fn an_empty_log_verifies() {
    let (svc, _) = memory_service();
    let verdict = svc.verify().unwrap();
    assert!(verdict.ok);
    assert_eq!(verdict.entries, 0);
}

#[test]
fn a_failed_append_is_reported_not_swallowed() {
    let (svc, repo) = memory_service();
    repo.fail_appends
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(svc.record(draft("t", McpOutcome::Allowed)).is_err());
}

// ── the real SQLite repository ──────────────────────────────────────────────────

#[test]
fn sqlite_log_chains_and_verifies() {
    let (svc, _, _dir) = sqlite_service();
    for i in 0..5 {
        let outcome = if i % 2 == 0 {
            McpOutcome::Allowed
        } else {
            McpOutcome::Denied
        };
        svc.record(draft("list_schema_objects", outcome)).unwrap();
    }
    let verdict = svc.verify().unwrap();
    assert!(verdict.ok, "{:?}", verdict.problem);
    assert_eq!(verdict.entries, 5);
}

#[test]
fn sqlite_triggers_reject_update_and_delete() {
    let (svc, path, _dir) = sqlite_service();
    svc.record(draft("t", McpOutcome::Allowed)).unwrap();

    let conn = Connection::open(&path).unwrap();
    let update = conn.execute("UPDATE mcp_audit_log SET reason = 'x'", []);
    assert!(update.unwrap_err().to_string().contains("append-only"));
    let delete = conn.execute("DELETE FROM mcp_audit_log", []);
    assert!(delete.unwrap_err().to_string().contains("append-only"));
    assert!(svc.verify().unwrap().ok);
}

#[test]
fn sqlite_manual_edit_is_detected_once_the_trigger_is_bypassed() {
    let (svc, path, _dir) = sqlite_service();
    for _ in 0..3 {
        svc.record(draft("query_table", McpOutcome::Denied))
            .unwrap();
    }

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER mcp_audit_log_no_update")
        .unwrap();
    conn.execute(
        "UPDATE mcp_audit_log SET outcome = 'allowed' WHERE id = 2",
        [],
    )
    .unwrap();

    let verdict = svc.verify().unwrap();
    assert!(!verdict.ok);
    assert_eq!(verdict.first_bad_id, Some(2));
}

#[test]
fn sqlite_truncation_of_the_tail_is_detected() {
    let (svc, path, _dir) = sqlite_service();
    for _ in 0..4 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER mcp_audit_log_no_delete")
        .unwrap();
    conn.execute("DELETE FROM mcp_audit_log WHERE id > 2", [])
        .unwrap();

    let verdict = svc.verify().unwrap();
    assert!(!verdict.ok);
    assert!(verdict.problem.unwrap().contains("truncated"));
}

#[test]
fn sqlite_deleting_the_whole_log_is_detected() {
    let (svc, path, _dir) = sqlite_service();
    svc.record(draft("t", McpOutcome::Allowed)).unwrap();

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER mcp_audit_log_no_delete")
        .unwrap();
    conn.execute("DELETE FROM mcp_audit_log", []).unwrap();

    assert!(!svc.verify().unwrap().ok);
}

#[test]
fn sqlite_appends_after_truncation_leave_a_visible_gap() {
    let (svc, path, _dir) = sqlite_service();
    for _ in 0..3 {
        svc.record(draft("t", McpOutcome::Allowed)).unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER mcp_audit_log_no_delete")
        .unwrap();
    conn.execute("DELETE FROM mcp_audit_log WHERE id = 3", [])
        .unwrap();
    svc.record(draft("t", McpOutcome::Allowed)).unwrap();

    assert!(!svc.verify().unwrap().ok);
}

#[test]
fn sqlite_filters_by_connection_tool_outcome_and_date() {
    let (svc, _, _dir) = sqlite_service();
    let mut other = draft("get_object_ddl", McpOutcome::Denied);
    other.connection_id = Some(2);
    svc.record(draft("query_table", McpOutcome::Allowed))
        .unwrap();
    svc.record(other).unwrap();
    svc.record(draft("query_table", McpOutcome::Denied))
        .unwrap();

    let list = |filter: McpAuditFilter| svc.list(&filter).unwrap();
    assert_eq!(list(McpAuditFilter::default()).len(), 3);
    assert_eq!(
        list(McpAuditFilter {
            connection_id: Some(2),
            ..Default::default()
        })
        .len(),
        1
    );
    assert_eq!(
        list(McpAuditFilter {
            tool: Some("query_table".into()),
            ..Default::default()
        })
        .len(),
        2
    );
    assert_eq!(
        list(McpAuditFilter {
            outcome: Some(McpOutcome::Denied),
            ..Default::default()
        })
        .len(),
        2
    );

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let filter = McpAuditFilter {
        from: Some(today.clone()),
        to: Some(today),
        ..Default::default()
    };
    assert_eq!(list(filter.clone()).len(), 3);
    assert_eq!(svc.count(&filter).unwrap(), 3);
    assert_eq!(
        list(McpAuditFilter {
            to: Some("2000-01-01".into()),
            ..Default::default()
        })
        .len(),
        0
    );

    let page = list(McpAuditFilter {
        limit: Some(2),
        offset: Some(1),
        ..Default::default()
    });
    assert_eq!(page.len(), 2);
    // Newest first.
    assert_eq!(page[0].id, 2);
}

#[test]
fn concurrent_writers_still_produce_a_single_valid_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.db");
    McpAuditService::new(Arc::new(SqliteMcpAuditRepository::with_path(path.clone())))
        .init_db()
        .unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let svc = McpAuditService::new(Arc::new(SqliteMcpAuditRepository::with_path(path)))
                    .without_forwarding();
                for _ in 0..10 {
                    svc.record(draft("t", McpOutcome::Allowed)).unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let svc = McpAuditService::new(Arc::new(SqliteMcpAuditRepository::with_path(path)));
    let verdict = svc.verify().unwrap();
    assert!(verdict.ok, "{:?}", verdict.problem);
    assert_eq!(verdict.entries, 40);
}

// ── CSV export ──────────────────────────────────────────────────────────────────

#[test]
fn csv_export_quotes_fields_and_neutralises_formulas() {
    let (svc, _) = memory_service();
    let mut entry = draft("query_table", McpOutcome::Allowed);
    entry.arguments = "{\"a\":\"x,\\\"y\\\"\"}".to_string();
    entry.reason = "=cmd|' /C calc'!A0".to_string();
    svc.record(entry).unwrap();

    let csv = svc.export_csv(&McpAuditFilter::default()).unwrap();
    let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("id,timestamp,tool,"));
    assert!(lines[1].contains("\"'=cmd|' /C calc'!A0\""));
    assert!(lines[1].contains("\"{\"\"a\"\":"));
}

#[test]
fn csv_export_lists_oldest_first() {
    let (svc, _) = memory_service();
    for tool in ["first", "second"] {
        svc.record(draft(tool, McpOutcome::Allowed)).unwrap();
    }
    let csv = svc.export_csv(&McpAuditFilter::default()).unwrap();
    let first = csv.find("first").unwrap();
    let second = csv.find("second").unwrap();
    assert!(first < second);
}
