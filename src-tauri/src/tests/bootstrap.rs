use std::collections::HashSet;

use rusqlite::Connection;

use crate::init;

#[test]
fn init_creates_sqlite_tables_and_returns_usable_services() {
    let temp_dir = tempfile::tempdir().unwrap();
    let database_path = temp_dir.path().join("isolated.db");
    let services = init(Some(database_path.clone().into_os_string())).unwrap();

    assert!(services.catalog.get_all_connections().unwrap().is_empty());
    assert!(!services.filter_rules_svc.list_rules().unwrap().is_empty());
    assert!(services.history_naming_rules_svc.list_rules().is_ok());
    assert!(
        services
            .query_history_svc
            .load_history()
            .unwrap()
            .is_empty()
    );

    let connection = Connection::open(database_path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    let tables: HashSet<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    for table in [
        "settings",
        "connections",
        "query_history",
        "table_filter_rules",
        "history_naming_rules",
        "folder_schema_overrides",
    ] {
        assert!(tables.contains(table), "missing table {table}");
    }
}
