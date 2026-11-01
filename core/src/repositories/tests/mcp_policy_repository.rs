use super::*;

/// The policy table as shipped before `allow_invalid_objects` existed.
const LEGACY_TABLE: &str = "CREATE TABLE connection_mcp_policy (
    connection_id       INTEGER PRIMARY KEY,
    mcp_enabled         INTEGER NOT NULL DEFAULT 0,
    allow_schema_list   INTEGER NOT NULL DEFAULT 1,
    allow_ddl_read      INTEGER NOT NULL DEFAULT 1,
    allow_save_scripts  INTEGER NOT NULL DEFAULT 0,
    allow_compare_fix   INTEGER NOT NULL DEFAULT 0,
    data_access         TEXT    NOT NULL DEFAULT 'none',
    allow_raw_sql       INTEGER NOT NULL DEFAULT 0,
    table_rules         TEXT    NOT NULL DEFAULT '[]',
    object_deny         TEXT    NOT NULL DEFAULT '[]',
    max_rows            INTEGER NOT NULL DEFAULT 200,
    query_timeout_secs  INTEGER NOT NULL DEFAULT 30,
    mcp_username        TEXT    NOT NULL DEFAULT ''
)";

#[test]
fn existing_policies_stay_denied_for_the_invalid_objects_flag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute(LEGACY_TABLE, []).unwrap();
        conn.execute(
            "INSERT INTO connection_mcp_policy (connection_id, mcp_enabled) VALUES (7, 1)",
            [],
        )
        .unwrap();
    }
    let repo = SqliteMcpPolicyRepository::with_path(path);

    repo.init_db().unwrap();
    repo.init_db().unwrap();

    let migrated = repo.get(7).unwrap().unwrap();
    assert!(migrated.mcp_enabled && !migrated.allow_invalid_objects);
}

#[test]
fn the_invalid_objects_flag_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let repo = SqliteMcpPolicyRepository::with_path(dir.path().join("policy.db"));
    repo.init_db().unwrap();

    let policy = McpPolicy {
        connection_id: 3,
        mcp_enabled: true,
        allow_invalid_objects: false,
        ..McpPolicy::default()
    };
    repo.upsert(&policy).unwrap();
    assert!(!repo.get(3).unwrap().unwrap().allow_invalid_objects);

    repo.upsert(&McpPolicy {
        allow_invalid_objects: true,
        ..policy
    })
    .unwrap();
    assert!(repo.get(3).unwrap().unwrap().allow_invalid_objects);
}
