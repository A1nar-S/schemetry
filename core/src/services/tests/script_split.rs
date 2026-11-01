use super::*;

fn sqls(script: &str, db: DbType) -> Vec<String> {
    split_script(script, db)
        .into_iter()
        .map(|s| s.sql)
        .collect()
}

#[test]
fn plain_statements_split_at_semicolons_outside_strings_and_comments() {
    let script = "CREATE VIEW v AS SELECT ';' x FROM dual; -- a; b\nALTER TABLE t ADD c NUMBER;\n/* ; */ DROP VIEW w";
    let parts = sqls(script, DbType::Oracle);
    assert_eq!(parts.len(), 3);
    assert!(parts[0].contains("';'"));
    assert!(parts[1].contains("ALTER TABLE"));
    assert!(parts[2].ends_with("DROP VIEW w"));
}

#[test]
fn oracle_plsql_units_end_at_a_slash_line() {
    let script = "CREATE OR REPLACE PROCEDURE p IS\nBEGIN\n  NULL;\nEND;\n/\nCREATE VIEW v AS SELECT 1 FROM dual;\nBEGIN\n  NULL;\nEND;\n/\n";
    let parts = split_script(script, DbType::Oracle);
    assert_eq!(parts.len(), 3);
    assert!(parts[0].sql.ends_with("END;"));
    assert_eq!(parts[0].object_type, "PROCEDURE");
    assert_eq!(parts[0].object_name, "P");
    assert_eq!(parts[1].object_type, "VIEW");
    assert!(parts[2].sql.starts_with("BEGIN"));
}

#[test]
fn oracle_q_quotes_and_package_bodies_are_kept_whole() {
    let script =
        "CREATE OR REPLACE PACKAGE BODY app.pkg AS\n  s VARCHAR2(10) := q'[a;b]';\nEND;\n/";
    let parts = split_script(script, DbType::Oracle);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].object_type, "PACKAGE BODY");
    assert_eq!(parts[0].object_name, "PKG");
}

#[test]
fn postgres_dollar_quoted_bodies_do_not_split() {
    let script = "CREATE FUNCTION f() RETURNS int LANGUAGE plpgsql AS $$ BEGIN RETURN 1; END; $$;\nSELECT 1;";
    let parts = split_script(script, DbType::Postgres);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].object_type, "FUNCTION");
    assert_eq!(parts[0].object_name, "f");
}

#[test]
fn empty_and_comment_only_scripts_have_no_statements() {
    assert!(split_script("  \n-- nothing\n/* x */ ;\n", DbType::Postgres).is_empty());
    assert!(split_script("", DbType::Oracle).is_empty());
}
