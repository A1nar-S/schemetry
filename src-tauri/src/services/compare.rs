use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};

use crate::models::{Discrepancy, ServersData};

/// Display label for a connection id — falls back to the id itself if the id isn't in
/// the name map (e.g. the connection was deleted since the data was fetched).
fn name_of(server_names: &HashMap<i64, String>, id: i64) -> String {
    server_names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"))
}

fn equal_ci(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => l.eq_ignore_ascii_case(r),
        _ => false,
    }
}

fn skip_index_diff(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (Some(l), Some(r)) => {
            !l.eq_ignore_ascii_case("none")
                && !r.eq_ignore_ascii_case("none")
                && !l.is_empty()
                && !r.is_empty()
        }
        _ => false,
    }
}

pub fn compare_tables_across_servers(
    servers: &ServersData,
    server_names: &HashMap<i64, String>,
    reference_server_id: i64,
    check_comments: bool,
    check_indexes: bool,
) -> Result<Vec<Discrepancy>> {
    if !servers.contains_key(&reference_server_id) {
        return Err(anyhow!(
            "Reference server '{}' does not exist in the server list.",
            name_of(server_names, reference_server_id)
        ));
    }

    let reference_name = name_of(server_names, reference_server_id);
    let mut discrepancies = Vec::new();
    let all_servers: Vec<i64> = servers.keys().copied().collect();
    let mut all_tables: HashSet<String> = HashSet::new();

    for server in &all_servers {
        if let Some(tables) = servers.get(server) {
            for table in tables.keys() {
                all_tables.insert(table.clone());
            }
        }
    }

    for table in all_tables {
        let reference_columns = servers
            .get(&reference_server_id)
            .and_then(|tables| tables.get(&table));

        if reference_columns.is_none() {
            for server in &all_servers {
                if *server == reference_server_id {
                    continue;
                }
                let has_table = servers
                    .get(server)
                    .and_then(|tables| tables.get(&table))
                    .is_some();

                if has_table {
                    let server_label = name_of(server_names, *server);
                    discrepancies.push(Discrepancy {
                        difference: "MISSING".to_string(),
                        element: "TABLE".to_string(),
                        table_name: table.clone(),
                        column_name: String::new(),
                        server_id: *server,
                        server_name: server_label.clone(),
                        details: format!(
                            "Table {} found in server {} but not in reference server {}",
                            table,
                            server_label.to_uppercase(),
                            reference_name.to_uppercase()
                        ),
                    });
                }
            }
            continue;
        }

        let reference_columns = reference_columns.expect("checked above");

        for server in &all_servers {
            if *server == reference_server_id {
                continue;
            }
            let server_label = name_of(server_names, *server);

            let current_columns = servers.get(server).and_then(|tables| tables.get(&table));

            if current_columns.is_none() {
                discrepancies.push(Discrepancy {
                    difference: "MISSING".to_string(),
                    element: "TABLE".to_string(),
                    table_name: table.clone(),
                    column_name: String::new(),
                    server_id: *server,
                    server_name: server_label.clone(),
                    details: format!(
                        "Table found in reference server {} but not in server {}",
                        reference_name.to_uppercase(),
                        server_label.to_uppercase()
                    ),
                });
                continue;
            }

            let current_columns = current_columns.expect("checked above");

            for (column_name, ref_col_info) in reference_columns {
                if !current_columns.contains_key(column_name) {
                    discrepancies.push(Discrepancy {
                        difference: "MISSING".to_string(),
                        element: "COLUMN".to_string(),
                        table_name: table.clone(),
                        column_name: column_name.clone(),
                        server_id: *server,
                        server_name: server_label.clone(),
                        details: format!(
                            "Column found in reference server {} but not in server {}",
                            reference_name.to_uppercase(),
                            server_label.to_uppercase()
                        ),
                    });
                    continue;
                }

                let curr_col_info = current_columns
                    .get(column_name)
                    .expect("contains_key checked above");

                let fields = [
                    ("COLUMN_NAME", &ref_col_info.column_name, &curr_col_info.column_name),
                    ("DATA_TYPE", &ref_col_info.data_type, &curr_col_info.data_type),
                    ("DATA_LENGTH", &ref_col_info.data_length, &curr_col_info.data_length),
                    ("DATA_DEFAULT", &ref_col_info.data_default, &curr_col_info.data_default),
                    ("COMMENTS", &ref_col_info.comments, &curr_col_info.comments),
                    ("INDEX_NAME", &ref_col_info.index_name, &curr_col_info.index_name),
                ];

                for (key, ref_value, curr_value) in fields {
                    if key == "COMMENTS" && !check_comments {
                        continue;
                    }
                    if key == "INDEX_NAME" && !check_indexes {
                        continue;
                    }
                    if key == "INDEX_NAME" && skip_index_diff(ref_value, curr_value) {
                        continue;
                    }

                    if !equal_ci(ref_value, curr_value) {
                        let details = if key == "DATA_LENGTH" {
                            format!(
                                "{}({}) != {}({})",
                                ref_col_info
                                    .data_type
                                    .clone()
                                    .unwrap_or_else(|| "?".to_string()),
                                ref_value.clone().unwrap_or_else(|| "NULL".to_string()),
                                curr_col_info
                                    .data_type
                                    .clone()
                                    .unwrap_or_else(|| "?".to_string()),
                                curr_value.clone().unwrap_or_else(|| "NULL".to_string())
                            )
                        } else {
                            format!(
                                "{}: {} != {}",
                                key,
                                ref_value.clone().unwrap_or_else(|| "NULL".to_string()),
                                curr_value.clone().unwrap_or_else(|| "NULL".to_string())
                            )
                        };

                        discrepancies.push(Discrepancy {
                            difference: "DIFFERENT".to_string(),
                            element: key.to_string(),
                            table_name: table.clone(),
                            column_name: column_name.clone(),
                            server_id: *server,
                            server_name: server_label.clone(),
                            details,
                        });
                    }
                }
            }

            for column_name in current_columns.keys() {
                if !reference_columns.contains_key(column_name) {
                    discrepancies.push(Discrepancy {
                        difference: "MISSING".to_string(),
                        element: "COLUMN".to_string(),
                        table_name: table.clone(),
                        column_name: column_name.clone(),
                        server_id: *server,
                        server_name: server_label.clone(),
                        details: format!(
                            "Column {} found in server {} but not in reference server {}",
                            column_name,
                            server_label.to_uppercase(),
                            reference_name.to_uppercase()
                        ),
                    });
                }
            }
        }
    }

    Ok(discrepancies)
}

#[cfg(test)]
#[path = "tests/compare.rs"]
mod tests;
