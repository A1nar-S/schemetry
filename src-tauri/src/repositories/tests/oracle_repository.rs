use super::*;

// ── clean_opt ────────────────────────────────────────────────────

#[test]
fn clean_opt_none_stays_none() {
    assert_eq!(DbOracleRepository::clean_opt(None), None);
}

#[test]
fn clean_opt_empty_string_becomes_none() {
    assert_eq!(DbOracleRepository::clean_opt(Some(String::new())), None);
}

#[test]
fn clean_opt_whitespace_only_becomes_none() {
    assert_eq!(DbOracleRepository::clean_opt(Some("   ".to_string())), None);
}

#[test]
fn clean_opt_null_string_becomes_none() {
    assert_eq!(DbOracleRepository::clean_opt(Some("null".to_string())), None);
    assert_eq!(DbOracleRepository::clean_opt(Some("NULL".to_string())), None);
    assert_eq!(DbOracleRepository::clean_opt(Some("Null".to_string())), None);
}

#[test]
fn clean_opt_valid_string_trims_whitespace() {
    assert_eq!(
        DbOracleRepository::clean_opt(Some("  hello  ".to_string())),
        Some("hello".to_string())
    );
}

#[test]
fn clean_opt_strips_newlines() {
    assert_eq!(
        DbOracleRepository::clean_opt(Some("hel\nlo".to_string())),
        Some("hello".to_string())
    );
}

// ── fmt_type ─────────────────────────────────────────────────────

#[test]
fn fmt_type_number_with_precision() {
    assert_eq!(fmt_type("NUMBER", Some("10")), "NUMBER(10)");
}

#[test]
fn fmt_type_number_without_precision() {
    assert_eq!(fmt_type("NUMBER", None), "NUMBER");
}

#[test]
fn fmt_type_number_empty_precision() {
    assert_eq!(fmt_type("NUMBER", Some("")), "NUMBER");
    assert_eq!(fmt_type("NUMBER", Some("0")), "NUMBER");
}

#[test]
fn fmt_type_varchar2_with_length() {
    assert_eq!(fmt_type("VARCHAR2", Some("50")), "VARCHAR2(50)");
}

#[test]
fn fmt_type_date_ignores_length() {
    assert_eq!(fmt_type("DATE", Some("7")), "DATE");
}

#[test]
fn fmt_type_clob_ignores_length() {
    assert_eq!(fmt_type("CLOB", Some("999")), "CLOB");
}

// ── clean_raw_ddl (code objects) ──────────────────────────────────

#[test]
fn clean_raw_ddl_view_uses_create_or_replace() {
    let ddl = "CREATE VIEW MY_VIEW AS SELECT 1 FROM DUAL";
    let result = clean_raw_ddl("VIEW", "MY_VIEW", ddl);
    assert!(result.starts_with("CREATE OR REPLACE"), "result: {result}");
}

#[test]
fn clean_raw_ddl_procedure_keeps_create_or_replace() {
    let ddl = "CREATE OR REPLACE PROCEDURE MY_PROC AS BEGIN NULL; END;";
    let result = clean_raw_ddl("PROCEDURE", "MY_PROC", ddl);
    assert!(result.contains("CREATE OR REPLACE"), "result: {result}");
}

#[test]
fn clean_raw_ddl_procedure_strips_editionable() {
    let ddl = "CREATE OR REPLACE EDITIONABLE PROCEDURE MY_PROC AS BEGIN NULL; END;";
    let result = clean_raw_ddl("PROCEDURE", "MY_PROC", ddl);
    assert!(!result.contains("EDITIONABLE"), "result: {result}");
}

// ── reformat_table_ddl ────────────────────────────────────────────

#[test]
fn format_column_type_number_variants() {
    assert_eq!(format_column_type("NUMBER", None, Some(10), Some(0), None, None), "NUMBER(10)");
    assert_eq!(format_column_type("NUMBER", None, Some(10), Some(2), None, None), "NUMBER(10,2)");
    assert_eq!(format_column_type("NUMBER", None, None, None, None, None), "NUMBER");
}

#[test]
fn format_column_type_varchar_and_date() {
    assert_eq!(format_column_type("VARCHAR2", Some(20), None, None, Some(20), Some("B")), "VARCHAR2(20)");
    assert_eq!(format_column_type("VARCHAR2", Some(80), None, None, Some(20), Some("C")), "VARCHAR2(20 CHAR)");
    assert_eq!(format_column_type("DATE", Some(7), None, None, None, None), "DATE");
}

fn sample_model() -> TableModel {
    TableModel {
        name: "IESTATIJUMI".to_string(),
        columns: vec![
            ColumnDef { name: "IID".into(),    type_str: "NUMBER(10)".into(), default: None,                    not_null: true },
            ColumnDef { name: "PEDDAT".into(), type_str: "DATE".into(),       default: Some("sysdate".into()),  not_null: false },
        ],
        table_comment: Some("tabula".into()),
        column_comments: vec![("IID".into(), "iestatijuma ID".into())],
        indexes: vec![IndexDef { name: "IESTATIJUMI_INOS".into(), unique: false, parts: vec!["UPPER(INOS)".into()] }],
        constraints: vec![
            ConstraintDef { name: "IESTATIJUMI_IID_PK".into(), kind: ConstraintKind::Primary, cols: vec!["IID".into()], ref_table: None, ref_cols: vec![], delete_rule: None, novalidate: false, check_cond: None },
            ConstraintDef { name: "IESTATIJUMI_IAID_FK".into(), kind: ConstraintKind::ForeignKey, cols: vec!["IAID".into()], ref_table: Some("APLIKACIJAS".into()), ref_cols: vec!["APL_ID".into()], delete_rule: None, novalidate: true, check_cond: None },
        ],
    }
}

#[test]
fn render_table_raw_matches_plsql_developer_style() {
    let out = render_table_raw(&sample_model());
    assert!(out.starts_with("create table IESTATIJUMI\n(\n"), "header: {out}");
    assert!(out.contains("  iid    NUMBER(10) not null,"), "iid line: {out}");
    assert!(out.contains("  peddat DATE default sysdate"), "peddat line: {out}");
    assert!(out.contains("\n)\n;"), "close: {out}");
    assert!(out.contains("comment on table IESTATIJUMI\n  is 'tabula';"), "tab comment: {out}");
    assert!(out.contains("comment on column IESTATIJUMI.iid\n  is 'iestatijuma ID';"), "col comment: {out}");
    assert!(out.contains("create index IESTATIJUMI_INOS on IESTATIJUMI (UPPER(INOS));"), "index: {out}");
    assert!(out.contains("alter table IESTATIJUMI\n  add constraint IESTATIJUMI_IID_PK primary key (IID);"), "pk: {out}");
    assert!(out.contains("add constraint IESTATIJUMI_IAID_FK foreign key (IAID)\n  references APLIKACIJAS (APL_ID)\n  novalidate;"), "fk: {out}");
}

#[test]
fn split_ddl_statements_segments_correctly() {
    let raw = render_table_raw(&sample_model());
    let stmts = split_ddl_statements(&raw);
    assert_eq!(stmts.len(), 6, "stmts: {stmts:#?}");
    assert!(stmts[0].starts_with("create table"), "first: {}", stmts[0]);
    assert!(stmts[0].contains("\n)\n;"), "create keeps ) and ; : {}", stmts[0]);
    assert!(stmts.iter().any(|s| s.starts_with("create index")), "has index");
    assert_eq!(stmts.iter().filter(|s| s.starts_with("alter table")).count(), 2, "two alters");
}

#[test]
fn split_ddl_statements_keeps_semicolon_in_comment_text() {
    let raw = "create table T\n(\n  a NUMBER\n)\n;\ncomment on column T.a\n  is 'has ; semicolon';\ncreate index IX on T (A);";
    let stmts = split_ddl_statements(raw);
    assert_eq!(stmts.len(), 3, "stmts: {stmts:#?}");
    assert!(stmts[1].contains("has ; semicolon"), "comment intact: {}", stmts[1]);
}

// ── build_deploy_script ───────────────────────────────────────────

#[test]
fn build_deploy_script_table_fans_out_guarded_blocks() {
    let raw = render_table_raw(&sample_model());
    let result = build_deploy_script("TABLE", "IESTATIJUMI", &raw);
    assert!(result.contains("SELECT COUNT(*) INTO v_count FROM USER_TABLES"), "table guard: {result}");
    assert!(result.contains("WHERE TABLE_NAME = 'IESTATIJUMI'"), "table name: {result}");
    // comment lives inside the table block (before its END IF)
    let tbl = result.find("USER_TABLES").unwrap();
    let comment = result.find("comment on column").unwrap();
    let first_endif = result.find("END IF;").unwrap();
    assert!(tbl < comment && comment < first_endif, "comment inside table block: {result}");
    // index + constraints each guarded by their own catalog view + name
    assert!(result.contains("FROM USER_INDEXES"), "index guard: {result}");
    assert!(result.contains("WHERE INDEX_NAME = 'IESTATIJUMI_INOS'"), "index name: {result}");
    assert!(result.contains("FROM USER_CONSTRAINTS"), "cons guard: {result}");
    assert!(result.contains("WHERE CONSTRAINT_NAME = 'IESTATIJUMI_IID_PK'"), "pk name: {result}");
    assert!(result.contains("WHERE CONSTRAINT_NAME = 'IESTATIJUMI_IAID_FK'"), "fk name: {result}");
    assert!(!result.contains("SQLCODE = -955"), "old pattern leaked: {result}");
    assert!(result.trim_end().ends_with('/'), "ends with slash: {result}");
}

#[test]
fn build_deploy_script_sequence_uses_user_sequences() {
    let ddl = "CREATE SEQUENCE MY_SEQ MINVALUE 1 INCREMENT BY 1 START WITH 1 CACHE 20";
    let result = build_deploy_script("SEQUENCE", "MY_SEQ", ddl);
    assert!(result.contains("FROM USER_SEQUENCES"), "result: {result}");
    assert!(result.contains("WHERE SEQUENCE_NAME = 'MY_SEQ'"), "result: {result}");
}

#[test]
fn build_deploy_script_code_object_passes_through() {
    let ddl = "CREATE OR REPLACE VIEW MY_VIEW AS SELECT 1 FROM DUAL";
    let result = build_deploy_script("VIEW", "MY_VIEW", ddl);
    assert_eq!(result, ddl);
}

// ── clean_sequence_ddl ────────────────────────────────────────────

#[test]
fn clean_sequence_ddl_strips_schema_qualified_name() {
    let ddl = r#"CREATE SEQUENCE "MY_SCHEMA"."MY_SEQ" START WITH 1"#;
    let result = clean_sequence_ddl(ddl, "MY_SEQ");
    assert!(!result.contains("MY_SCHEMA"), "result: {result}");
    assert!(result.contains("MY_SEQ"), "result: {result}");
}

#[test]
fn clean_sequence_ddl_strips_noorder_keyword() {
    let ddl = "CREATE SEQUENCE MY_SEQ START WITH 1 NOORDER NOCYCLE";
    let result = clean_sequence_ddl(ddl, "MY_SEQ");
    assert!(!result.contains("NOORDER"), "result: {result}");
    assert!(!result.contains("NOCYCLE"), "result: {result}");
}

#[test]
fn clean_sequence_ddl_normalizes_start_with_to_one() {
    let ddl = "CREATE SEQUENCE MY_SEQ MINVALUE 1 INCREMENT BY 1 START WITH 12345 CACHE 20";
    let result = clean_sequence_ddl(ddl, "MY_SEQ");
    assert!(result.contains("START WITH 1 "), "result: {result}");
    assert!(!result.contains("12345"), "exported start value leaked: {result}");
}

#[test]
fn clean_sequence_ddl_collapses_extra_spaces() {
    let ddl = "CREATE  SEQUENCE  MY_SEQ  START  WITH  1";
    let result = clean_sequence_ddl(ddl, "MY_SEQ");
    assert!(!result.contains("  "), "double space found in: {result}");
}
