use std::collections::BTreeMap;

use crate::models::ConnectionRecord;

pub fn group_by_schema(connections: &[ConnectionRecord]) -> Vec<(String, Vec<ConnectionRecord>)> {
    let mut groups: BTreeMap<String, Vec<ConnectionRecord>> = BTreeMap::new();

    for conn in connections.iter().cloned() {
        let group = if conn.group_name.trim().is_empty() {
            "(no group)".to_string()
        } else {
            conn.group_name.clone()
        };
        groups.entry(group).or_default().push(conn);
    }

    for items in groups.values_mut() {
        items.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    }

    groups.into_iter().collect()
}