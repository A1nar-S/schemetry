use super::*;

fn result(id: i64, name: &str, columns: &[&str], rows: &[&[Option<&str>]]) -> QueryServerResult {
    QueryServerResult {
        server_id: id,
        server_name: name.to_string(),
        result_id: 0,
        columns: columns.iter().map(|c| c.to_string()).collect(),
        column_types: vec![String::new(); columns.len()],
        rows: rows
            .iter()
            .map(|r| r.iter().map(|v| v.map(str::to_string)).collect())
            .collect(),
        error: None,
        duration_ms: 0,
    }
}

fn csv_string(results: &[QueryServerResult]) -> String {
    let mut buf = Vec::new();
    write_csv(&mut buf, results).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn csv_escape_plain_value_unchanged() {
    assert_eq!(csv_escape("hello"), "hello");
    assert_eq!(csv_escape(""), "");
}

#[test]
fn csv_escape_quotes_special_characters() {
    assert_eq!(csv_escape("a,b"), "\"a,b\"");
    assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    assert_eq!(csv_escape("cr\rhere"), "\"cr\rhere\"");
}

#[test]
fn csv_single_server_has_no_server_column() {
    let results = [result(1, "DEV", &["ID", "NAME"], &[&[Some("1"), Some("x")], &[Some("2"), None]])];
    assert_eq!(csv_string(&results), "ID,NAME\n1,x\n2,\n");
}

#[test]
fn csv_multiple_servers_adds_server_column() {
    let results = [
        result(1, "DEV", &["ID"], &[&[Some("1")]]),
        result(2, "PROD, EU", &["ID"], &[&[Some("2")]]),
    ];
    assert_eq!(csv_string(&results), "Server,ID\nDEV,1\n\"PROD, EU\",2\n");
}

#[test]
fn csv_skips_failed_servers() {
    let mut failed = result(2, "BROKEN", &[], &[]);
    failed.error = Some("ORA-12541".to_string());
    let results = [result(1, "DEV", &["ID"], &[&[Some("1")]]), failed];
    assert_eq!(csv_string(&results), "ID\n1\n");
}

#[test]
fn csv_no_result_sets_writes_nothing() {
    assert_eq!(csv_string(&[]), "");
}

#[test]
fn csv_does_not_clip_long_values() {
    let long = "a".repeat(EXCEL_CELL_LIMIT + 10);
    let results = [result(1, "DEV", &["V"], &[&[Some(&long)]])];
    assert_eq!(csv_string(&results), format!("V\n{long}\n"));
}
