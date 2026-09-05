use std::collections::HashSet;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::models::{ColumnInfo, Discrepancy, ServerTableDdls, ServersData, TableColumns};

const NULL_TOKEN: &str = "#NULL#";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixScriptResult {
    pub script: String,
    pub generated_count: usize,
    pub skipped_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FixKind {
    MissingTable,
    MissingColumn,
    DataType,
    DataLength,
    DataDefault,
    Comments,
    IndexName,
    ColumnName,
    Unsupported,
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn quoted_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn to_upper_key(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn parse_target_server_from_details(details: &str) -> Option<String> {
    let marker = "found in server ";
    let end_marker = " but not in reference server ";
    let start = details.find(marker)? + marker.len();
    let tail = &details[start..];
    let end = tail.find(end_marker)?;
    let server = tail[..end].trim();
    if server.is_empty() {
        None
    } else {
        Some(server.to_string())
    }
}

fn detect_fix_kind(row: &Discrepancy) -> FixKind {
    if row.difference.eq_ignore_ascii_case("MISSING") {
        if row.column_name.trim().is_empty() {
            FixKind::MissingTable
        } else {
            FixKind::MissingColumn
        }
    } else if row.difference.eq_ignore_ascii_case("DIFFERENT") {
        if row.details.starts_with("DATA_TYPE:") {
            FixKind::DataType
        } else if row.details.starts_with("DATA_DEFAULT:") {
            FixKind::DataDefault
        } else if row.details.starts_with("COMMENTS:") {
            FixKind::Comments
        } else if row.details.starts_with("INDEX_NAME:") {
            FixKind::IndexName
        } else if row.details.starts_with("COLUMN_NAME:") {
            FixKind::ColumnName
        } else if row.details.contains(" != ") && row.details.contains('(') && row.details.contains(')') {
            FixKind::DataLength
        } else {
            FixKind::Unsupported
        }
    } else {
        FixKind::Unsupported
    }
}

fn resolve_target_server(row: &Discrepancy, reference_server: &str) -> Option<String> {
    if !row.server_name.eq_ignore_ascii_case(reference_server) {
        return Some(row.server_name.clone());
    }

    if row.difference.eq_ignore_ascii_case("MISSING") {
        return parse_target_server_from_details(&row.details);
    }

    None
}

pub fn discrepancy_target_server(row: &Discrepancy, reference_server: &str) -> Option<String> {
    resolve_target_server(row, reference_server)
}

fn resolve_loaded_server_name(servers: &ServersData, target_server: &str) -> Option<String> {
    if servers.contains_key(target_server) {
        return Some(target_server.to_string());
    }

    servers
        .keys()
        .find(|name| name.eq_ignore_ascii_case(target_server))
        .cloned()
}

fn desired_type_sql(reference_col: &ColumnInfo) -> Option<String> {
    let data_type = reference_col.data_type.as_deref()?.trim();
    if data_type.is_empty() {
        return None;
    }

    let mut out = data_type.to_string();
    if let Some(length) = reference_col.data_length.as_deref() {
        let trimmed = length.trim();
        if !trimmed.is_empty() {
            out.push('(');
            out.push_str(trimmed);
            out.push(')');
        }
    }

    Some(out)
}

fn default_clause(reference_col: &ColumnInfo) -> String {
    match reference_col.data_default.as_deref().map(str::trim) {
        Some(default_expr) if !default_expr.is_empty() => format!(" DEFAULT {default_expr}"),
        _ => String::new(),
    }
}

fn desired_default_expr(reference_col: &ColumnInfo) -> Option<String> {
    reference_col
        .data_default
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn find_reference_column<'a>(
    servers: &'a ServersData,
    reference_server: &str,
    table: &str,
    column: &str,
) -> Option<&'a ColumnInfo> {
    servers
        .get(reference_server)?
        .get(table)?
        .get(column)
}

fn find_reference_table_columns<'a>(
    servers: &'a ServersData,
    reference_server: &str,
    table: &str,
) -> Option<&'a TableColumns> {
    let tables = servers.get(reference_server)?;
    tables
        .get(table)
        .or_else(|| tables.iter().find_map(|(name, cols)| {
            if name.eq_ignore_ascii_case(table) {
                Some(cols)
            } else {
                None
            }
        }))
}

fn find_reference_table_ddl<'a>(
    server_table_ddls: &'a ServerTableDdls,
    reference_server: &str,
    table: &str,
) -> Option<&'a str> {
    let table_ddls = server_table_ddls.get(reference_server)?;
    table_ddls
        .get(table)
        .or_else(|| table_ddls.get(&table.to_ascii_uppercase()))
        .or_else(|| table_ddls.get(&table.to_ascii_lowercase()))
        .or_else(|| table_ddls.iter().find_map(|(name, ddl)| {
            if name.eq_ignore_ascii_case(table) {
                Some(ddl)
            } else {
                None
            }
        }))
        .map(String::as_str)
}

fn normalize_ddl_for_execute_immediate(ddl: &str) -> Option<String> {
    let mut out = ddl.replace("\r\n", "\n").replace('\r', "\n");
    out = out.trim().to_string();
    if out.is_empty() {
        return None;
    }

    loop {
        let trimmed = out.trim_end();
        if let Some(stripped) = trimmed.strip_suffix(';') {
            out = stripped.trim_end().to_string();
            continue;
        }
        if let Some(stripped) = trimmed.strip_suffix('/') {
            out = stripped.trim_end().to_string();
            continue;
        }
        break;
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn build_create_table_block(table: &str, reference_table_ddl: &str) -> Option<String> {
    let create_sql = normalize_ddl_for_execute_immediate(reference_table_ddl)?;
    let table_key = to_upper_key(table);

    Some(format!(
        "DECLARE\n    v_table_count NUMBER := 0;\nBEGIN\n    SELECT COUNT(*)\n      INTO v_table_count\n      FROM USER_TABLES\n     WHERE TABLE_NAME = UPPER({table_lit});\n\n    IF v_table_count = 0 THEN\n        EXECUTE IMMEDIATE {create_lit};\n    END IF;\nEND;\n/",
        table_lit = sql_literal(&table_key),
        create_lit = sql_literal(&create_sql),
    ))
}

fn build_column_definition_sql(column: &str, reference_col: &ColumnInfo) -> Option<String> {
    let type_sql = desired_type_sql(reference_col)?;
    let mut out = format!("{} {}", quoted_ident(column), type_sql);
    out.push_str(&default_clause(reference_col));
    Some(out)
}

fn build_comments_suffix(table: &str, reference_columns: &TableColumns) -> String {
    let table_key = to_upper_key(table);
    let mut extra = String::new();

    let mut column_names: Vec<&String> = reference_columns.keys().collect();
    column_names.sort_unstable();

    for column_name in &column_names {
        let col = match reference_columns.get(*column_name) {
            Some(c) => c,
            None => continue,
        };
        let comment = match col.comments.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(c) => c,
            None => continue,
        };
        let column_key = to_upper_key(column_name);
        let comment_sql = format!(
            "COMMENT ON COLUMN {}.{} IS {}",
            quoted_ident(table),
            quoted_ident(column_name),
            sql_literal(comment)
        );
        extra.push_str(&format!(
            "\n\nDECLARE\n    v_comment USER_COL_COMMENTS.COMMENTS%TYPE;\nBEGIN\n    SELECT COMMENTS\n      INTO v_comment\n      FROM USER_COL_COMMENTS\n     WHERE TABLE_NAME = UPPER({table_lit})\n       AND COLUMN_NAME = UPPER({column_lit});\n\n    IF NVL(v_comment, CHR(0)) <> NVL({comment_lit}, CHR(0)) THEN\n        EXECUTE IMMEDIATE {comment_sql_lit};\n    END IF;\nEXCEPTION\n    WHEN NO_DATA_FOUND THEN\n        NULL;\nEND;\n/",
            table_lit = sql_literal(&table_key),
            column_lit = sql_literal(&column_key),
            comment_lit = sql_literal(comment),
            comment_sql_lit = sql_literal(&comment_sql),
        ));
    }

    extra
}

fn build_indexes_suffix(table: &str, reference_columns: &TableColumns) -> String {
    let mut extra = String::new();

    let mut column_names: Vec<&String> = reference_columns.keys().collect();
    column_names.sort_unstable();

    let mut index_columns: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for column_name in &column_names {
        let col = match reference_columns.get(*column_name) {
            Some(c) => c,
            None => continue,
        };
        if let Some(idx) = col.index_name.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            index_columns
                .entry(idx.to_string())
                .or_default()
                .push(column_name.to_string());
        }
    }
    for (index_name, cols) in &index_columns {
        let index_key = to_upper_key(index_name);
        let col_list = cols.iter().map(|c| quoted_ident(c)).collect::<Vec<_>>().join(", ");
        let create_index_sql = format!(
            "CREATE INDEX {} ON {} ({})",
            quoted_ident(index_name),
            quoted_ident(table),
            col_list
        );
        extra.push_str(&format!(
            "\n\nDECLARE\n    v_index_count NUMBER := 0;\nBEGIN\n    SELECT COUNT(*)\n      INTO v_index_count\n      FROM USER_INDEXES\n     WHERE INDEX_NAME = UPPER({index_lit});\n\n    IF v_index_count = 0 THEN\n        EXECUTE IMMEDIATE {create_index_lit};\n    END IF;\nEND;\n/",
            index_lit = sql_literal(&index_key),
            create_index_lit = sql_literal(&create_index_sql),
        ));
    }

    extra
}

fn build_comments_and_indexes_suffix(table: &str, reference_columns: &TableColumns) -> String {
    let mut out = build_comments_suffix(table, reference_columns);
    out.push_str(&build_indexes_suffix(table, reference_columns));
    out
}

fn build_create_table_from_columns_block(
    table: &str,
    reference_columns: &TableColumns,
) -> Option<String> {
    if reference_columns.is_empty() {
        return None;
    }

    let mut column_names: Vec<&String> = reference_columns.keys().collect();
    column_names.sort_unstable();

    let mut definitions = Vec::with_capacity(column_names.len());
    for column_name in &column_names {
        let reference_col = reference_columns.get(*column_name)?;
        definitions.push(build_column_definition_sql(column_name, reference_col)?);
    }

    let ddl = format!(
        "CREATE TABLE {} ({})",
        quoted_ident(table),
        definitions.join(", ")
    );

    let create_block = build_create_table_block(table, &ddl)?;
    let suffix = build_comments_and_indexes_suffix(table, reference_columns);
    Some(format!("{}{}", create_block, suffix))
}

fn build_add_column_block(table: &str, column: &str, reference_col: &ColumnInfo) -> Option<String> {
    let type_sql = desired_type_sql(reference_col)?;
    let alter_sql = format!(
        "ALTER TABLE {} ADD ({} {}{})",
        quoted_ident(table),
        quoted_ident(column),
        type_sql,
        default_clause(reference_col)
    );

    let table_key = to_upper_key(table);
    let column_key = to_upper_key(column);

    let mut block = format!(
        "DECLARE\n    v_table_count NUMBER := 0;\n    v_column_count NUMBER := 0;\nBEGIN\n    SELECT COUNT(*)\n      INTO v_table_count\n      FROM USER_TABLES\n     WHERE TABLE_NAME = UPPER({table_lit});\n\n    IF v_table_count = 1 THEN\n        SELECT COUNT(*)\n          INTO v_column_count\n          FROM USER_TAB_COLUMNS\n         WHERE TABLE_NAME = UPPER({table_lit})\n           AND COLUMN_NAME = UPPER({column_lit});\n\n        IF v_column_count = 0 THEN\n            EXECUTE IMMEDIATE {alter_lit};\n        END IF;\n    END IF;\nEND;\n/",
        table_lit = sql_literal(&table_key),
        column_lit = sql_literal(&column_key),
        alter_lit = sql_literal(&alter_sql),
    );

    if let Some(comment) = reference_col.comments.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let comment_sql = format!(
            "COMMENT ON COLUMN {}.{} IS {}",
            quoted_ident(table),
            quoted_ident(column),
            sql_literal(comment)
        );

        let comment_block = format!(
            "\n\nDECLARE\n    v_comment USER_COL_COMMENTS.COMMENTS%TYPE;\nBEGIN\n    SELECT COMMENTS\n      INTO v_comment\n      FROM USER_COL_COMMENTS\n     WHERE TABLE_NAME = UPPER({table_lit})\n       AND COLUMN_NAME = UPPER({column_lit});\n\n    IF NVL(v_comment, CHR(0)) <> NVL({comment_lit}, CHR(0)) THEN\n        EXECUTE IMMEDIATE {comment_sql_lit};\n    END IF;\nEXCEPTION\n    WHEN NO_DATA_FOUND THEN\n        NULL;\nEND;\n/",
            table_lit = sql_literal(&table_key),
            column_lit = sql_literal(&column_key),
            comment_lit = sql_literal(comment),
            comment_sql_lit = sql_literal(&comment_sql),
        );

        block.push_str(&comment_block);
    }

    Some(block)
}

fn build_modify_type_block(table: &str, column: &str, reference_col: &ColumnInfo) -> Option<String> {
    let type_sql = desired_type_sql(reference_col)?;
    let expected_type = reference_col.data_type.as_deref()?.trim();
    let expected_length = reference_col
        .data_length
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(NULL_TOKEN);

    let alter_sql = format!(
        "ALTER TABLE {} MODIFY ({} {})",
        quoted_ident(table),
        quoted_ident(column),
        type_sql
    );

    let table_key = to_upper_key(table);
    let column_key = to_upper_key(column);

    Some(format!(
        "DECLARE\n    v_column_count NUMBER := 0;\n    v_match_count NUMBER := 0;\nBEGIN\n    SELECT COUNT(*)\n      INTO v_column_count\n      FROM USER_TAB_COLUMNS\n     WHERE TABLE_NAME = UPPER({table_lit})\n       AND COLUMN_NAME = UPPER({column_lit});\n\n    IF v_column_count = 1 THEN\n        SELECT COUNT(*)\n          INTO v_match_count\n          FROM USER_TAB_COLUMNS\n         WHERE TABLE_NAME = UPPER({table_lit})\n           AND COLUMN_NAME = UPPER({column_lit})\n           AND UPPER(DATA_TYPE) = UPPER({dtype_lit})\n           AND NVL(\n                 TO_CHAR(\n                     CASE\n                         WHEN DATA_TYPE = 'NUMBER' THEN DATA_PRECISION\n                         WHEN DATA_TYPE = 'DATE' THEN NULL\n                         WHEN DATA_TYPE = 'NVARCHAR2' THEN CHAR_LENGTH\n                         WHEN DATA_TYPE IN ('CLOB', 'BLOB', 'LONG RAW') THEN NULL\n                         ELSE DATA_LENGTH\n                     END\n                 ),\n                 {null_token_lit}\n               ) = NVL({dlen_lit}, {null_token_lit});\n\n        IF v_match_count = 0 THEN\n            EXECUTE IMMEDIATE {alter_lit};\n        END IF;\n    END IF;\nEND;\n/",
        table_lit = sql_literal(&table_key),
        column_lit = sql_literal(&column_key),
        dtype_lit = sql_literal(expected_type),
        dlen_lit = sql_literal(expected_length),
        null_token_lit = sql_literal(NULL_TOKEN),
        alter_lit = sql_literal(&alter_sql),
    ))
}

fn build_default_block(table: &str, column: &str, reference_col: &ColumnInfo) -> Option<String> {
    let desired_default = desired_default_expr(reference_col);
    let default_token = desired_default
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(NULL_TOKEN)
        .to_string();

    let alter_sql = match desired_default {
        Some(expr) => format!(
            "ALTER TABLE {} MODIFY ({} DEFAULT {})",
            quoted_ident(table),
            quoted_ident(column),
            expr
        ),
        None => format!(
            "ALTER TABLE {} MODIFY ({} DEFAULT NULL)",
            quoted_ident(table),
            quoted_ident(column)
        ),
    };

    let table_key = to_upper_key(table);
    let column_key = to_upper_key(column);

    Some(format!(
        "DECLARE\n    v_column_count NUMBER := 0;\n    v_match_count NUMBER := 0;\nBEGIN\n    SELECT COUNT(*)\n      INTO v_column_count\n      FROM USER_TAB_COLUMNS\n     WHERE TABLE_NAME = UPPER({table_lit})\n       AND COLUMN_NAME = UPPER({column_lit});\n\n    IF v_column_count = 1 THEN\n        SELECT COUNT(*)\n          INTO v_match_count\n          FROM USER_TAB_COLUMNS\n         WHERE TABLE_NAME = UPPER({table_lit})\n           AND COLUMN_NAME = UPPER({column_lit})\n           AND NVL(TRIM(DATA_DEFAULT), {null_token_lit}) = NVL({default_lit}, {null_token_lit});\n\n        IF v_match_count = 0 THEN\n            EXECUTE IMMEDIATE {alter_lit};\n        END IF;\n    END IF;\nEND;\n/",
        table_lit = sql_literal(&table_key),
        column_lit = sql_literal(&column_key),
        default_lit = sql_literal(&default_token),
        null_token_lit = sql_literal(NULL_TOKEN),
        alter_lit = sql_literal(&alter_sql),
    ))
}

fn build_comment_block(table: &str, column: &str, reference_col: &ColumnInfo) -> Option<String> {
    let desired_comment = reference_col.comments.as_deref().map(str::trim).unwrap_or("");
    let comment_sql = format!(
        "COMMENT ON COLUMN {}.{} IS {}",
        quoted_ident(table),
        quoted_ident(column),
        sql_literal(desired_comment)
    );

    let table_key = to_upper_key(table);
    let column_key = to_upper_key(column);

    Some(format!(
        "DECLARE\n    v_comment USER_COL_COMMENTS.COMMENTS%TYPE;\nBEGIN\n    SELECT COMMENTS\n      INTO v_comment\n      FROM USER_COL_COMMENTS\n     WHERE TABLE_NAME = UPPER({table_lit})\n       AND COLUMN_NAME = UPPER({column_lit});\n\n    IF NVL(v_comment, CHR(0)) <> NVL({comment_lit}, CHR(0)) THEN\n        EXECUTE IMMEDIATE {comment_sql_lit};\n    END IF;\nEXCEPTION\n    WHEN NO_DATA_FOUND THEN\n        NULL;\nEND;\n/",
        table_lit = sql_literal(&table_key),
        column_lit = sql_literal(&column_key),
        comment_lit = sql_literal(desired_comment),
        comment_sql_lit = sql_literal(&comment_sql),
    ))
}

fn classify_skip_reason(kind: FixKind) -> &'static str {
    match kind {
        FixKind::IndexName => {
            "Index discrepancy skipped: index composition is not available, so auto-fix would be unsafe."
        }
        FixKind::ColumnName => {
            "Column name discrepancy skipped: automatic column rename is unsafe and not generated."
        }
        _ => "Discrepancy type is not recognized for automatic fixing.",
    }
}

pub fn selection_requires_reference_table_ddl(
    discrepancies: &[Discrepancy],
    selected_ids: &HashSet<usize>,
) -> bool {
    selected_ids.iter().any(|id| {
        discrepancies
            .get(*id)
            .map(|row| detect_fix_kind(row) == FixKind::MissingTable)
            .unwrap_or(false)
    })
}

pub fn generate_fix_script(
    discrepancies: &[Discrepancy],
    selected_ids: &HashSet<usize>,
    servers: &ServersData,
    server_table_ddls: &ServerTableDdls,
    reference_server: &str,
) -> Result<FixScriptResult> {
    if selected_ids.is_empty() {
        return Err(anyhow!("Select at least one discrepancy first."));
    }

    if !servers.contains_key(reference_server) {
        return Err(anyhow!(
            "Reference server '{}' is not loaded in current fetch data.",
            reference_server
        ));
    }

    let mut ids: Vec<usize> = selected_ids.iter().copied().collect();
    ids.sort_unstable();

    let mut generated_count = 0usize;
    let mut skipped_count = 0usize;
    let mut generated_targets = Vec::new();
    let mut generated_blocks: Vec<(String, String)> = Vec::new();
    let mut skipped_messages = Vec::new();
    let mut dedup = HashSet::new();

    for id in ids {
        let Some(row) = discrepancies.get(id) else {
            skipped_count += 1;
            skipped_messages.push(format!("#{id}: discrepancy no longer exists."));
            continue;
        };

        let kind = detect_fix_kind(row);
        if matches!(
            kind,
            FixKind::IndexName | FixKind::ColumnName | FixKind::Unsupported
        ) {
            skipped_count += 1;
            skipped_messages.push(format!("#{}: {}", id + 1, classify_skip_reason(kind)));
            continue;
        }

        let Some(target_server) = resolve_target_server(row, reference_server) else {
            skipped_count += 1;
            skipped_messages.push(format!(
                "#{}: could not resolve target server for this discrepancy.",
                id + 1
            ));
            continue;
        };

        let Some(target_server) = resolve_loaded_server_name(servers, &target_server) else {
            skipped_count += 1;
            skipped_messages.push(format!(
                "#{}: target server '{}' is not loaded in fetched data.",
                id + 1,
                target_server
            ));
            continue;
        };

        if kind != FixKind::MissingTable && row.column_name.trim().is_empty() {
            skipped_count += 1;
            skipped_messages.push(format!(
                "#{}: missing column name, cannot generate statement safely.",
                id + 1
            ));
            continue;
        }

        let block = match kind {
            FixKind::MissingTable => {
                if row.server_name.eq_ignore_ascii_case(reference_server) {
                    skipped_count += 1;
                    skipped_messages.push(format!(
                        "#{}: table exists on non-reference server but missing in reference; create-table fix is not applicable.",
                        id + 1
                    ));
                    continue;
                }

                if let Some(table_ddl) = find_reference_table_ddl(
                    server_table_ddls,
                    reference_server,
                    row.table_name.trim(),
                ) {
                    if let Some(block) = build_create_table_block(row.table_name.trim(), table_ddl) {
                        let suffix = find_reference_table_columns(servers, reference_server, row.table_name.trim())
                            .map(|cols| build_comments_and_indexes_suffix(row.table_name.trim(), cols))
                            .unwrap_or_default();
                        Some(format!("{}{}", block, suffix))
                    } else {
                        let Some(reference_columns) = find_reference_table_columns(
                            servers,
                            reference_server,
                            row.table_name.trim(),
                        ) else {
                            skipped_count += 1;
                            skipped_messages.push(format!(
                                "#{}: full DDL for table {} could not be normalized to executable SQL.",
                                id + 1,
                                row.table_name
                            ));
                            continue;
                        };

                        let Some(fallback_block) =
                            build_create_table_from_columns_block(row.table_name.trim(), reference_columns)
                        else {
                            skipped_count += 1;
                            skipped_messages.push(format!(
                                "#{}: failed to build fallback CREATE TABLE for {} from column metadata.",
                                id + 1,
                                row.table_name
                            ));
                            continue;
                        };

                        Some(format!(
                            "-- Fallback: DBMS_METADATA DDL could not be normalized, using column metadata definition.\n{}",
                            fallback_block
                        ))
                    }
                } else {
                    let Some(reference_columns) = find_reference_table_columns(
                        servers,
                        reference_server,
                        row.table_name.trim(),
                    ) else {
                        skipped_count += 1;
                        skipped_messages.push(format!(
                            "#{}: full DDL for table {} is not available from reference metadata.",
                            id + 1,
                            row.table_name
                        ));
                        continue;
                    };

                    let Some(fallback_block) =
                        build_create_table_from_columns_block(row.table_name.trim(), reference_columns)
                    else {
                        skipped_count += 1;
                        skipped_messages.push(format!(
                            "#{}: failed to build fallback CREATE TABLE for {} from column metadata.",
                            id + 1,
                            row.table_name
                        ));
                        continue;
                    };

                    Some(format!(
                        "-- Fallback: DBMS_METADATA DDL unavailable, using column metadata definition.\n{}",
                        fallback_block
                    ))
                }
            }
            FixKind::MissingColumn => {
                let Some(reference_col) = find_reference_column(
                    servers,
                    reference_server,
                    row.table_name.trim(),
                    row.column_name.trim(),
                ) else {
                    skipped_count += 1;
                    skipped_messages.push(format!(
                        "#{}: reference metadata for {}.{} is not available.",
                        id + 1,
                        row.table_name,
                        row.column_name
                    ));
                    continue;
                };
                build_add_column_block(row.table_name.trim(), row.column_name.trim(), reference_col)
            }
            FixKind::DataType | FixKind::DataLength => {
                let Some(reference_col) = find_reference_column(
                    servers,
                    reference_server,
                    row.table_name.trim(),
                    row.column_name.trim(),
                ) else {
                    skipped_count += 1;
                    skipped_messages.push(format!(
                        "#{}: reference metadata for {}.{} is not available.",
                        id + 1,
                        row.table_name,
                        row.column_name
                    ));
                    continue;
                };
                build_modify_type_block(row.table_name.trim(), row.column_name.trim(), reference_col)
            }
            FixKind::DataDefault => {
                let Some(reference_col) = find_reference_column(
                    servers,
                    reference_server,
                    row.table_name.trim(),
                    row.column_name.trim(),
                ) else {
                    skipped_count += 1;
                    skipped_messages.push(format!(
                        "#{}: reference metadata for {}.{} is not available.",
                        id + 1,
                        row.table_name,
                        row.column_name
                    ));
                    continue;
                };
                build_default_block(row.table_name.trim(), row.column_name.trim(), reference_col)
            }
            FixKind::Comments => {
                let Some(reference_col) = find_reference_column(
                    servers,
                    reference_server,
                    row.table_name.trim(),
                    row.column_name.trim(),
                ) else {
                    skipped_count += 1;
                    skipped_messages.push(format!(
                        "#{}: reference metadata for {}.{} is not available.",
                        id + 1,
                        row.table_name,
                        row.column_name
                    ));
                    continue;
                };
                build_comment_block(row.table_name.trim(), row.column_name.trim(), reference_col)
            }
            _ => None,
        };

        let Some(block) = block else {
            skipped_count += 1;
            skipped_messages.push(format!(
                "#{}: insufficient metadata to build a safe statement.",
                id + 1
            ));
            continue;
        };

        let dedup_key = format!(
            "{}|{}|{}|{:?}|{}",
            target_server,
            row.table_name.trim(),
            row.column_name.trim(),
            kind,
            block
        );
        if !dedup.insert(dedup_key) {
            continue;
        }

        generated_count += 1;
        if !generated_targets
            .iter()
            .any(|name: &String| name.eq_ignore_ascii_case(&target_server))
        {
            generated_targets.push(target_server.clone());
        }

        generated_blocks.push((
            target_server.clone(),
            format!(
            "-- discrepancy #{}\n-- table: {}\n-- column: {}\n{}",
            id + 1,
            row.table_name,
            if row.column_name.trim().is_empty() {
                "(table-level)".to_string()
            } else {
                row.column_name.clone()
            },
            block
        )));
    }

    let mut script = String::new();
    script.push_str(&format!("-- Reference server: {}\n", reference_server));
    script.push_str("-- Review carefully before execution.\n\n");

    if generated_blocks.is_empty() {
        script.push_str("-- No executable fix statements were generated for the current selection.\n");
    } else {
        for (index, target_server) in generated_targets.iter().enumerate() {
            if index > 0 {
                script.push('\n');
            }

            script.push_str(&format!("-- Target data source: {}\n\n", target_server));

            let blocks: Vec<&str> = generated_blocks
                .iter()
                .filter(|(block_target, _)| block_target.eq_ignore_ascii_case(target_server))
                .map(|(_, block)| block.as_str())
                .collect();

            script.push_str(&blocks.join("\n\n"));
            script.push('\n');
        }
    }

    if !skipped_messages.is_empty() {
        script.push_str("\n-- Skipped discrepancies\n");
        for message in skipped_messages {
            script.push_str("-- ");
            script.push_str(&message);
            script.push('\n');
        }
    }

    Ok(FixScriptResult {
        script,
        generated_count,
        skipped_count,
    })
}

#[cfg(test)]
#[path = "tests/fix.rs"]
mod tests;
