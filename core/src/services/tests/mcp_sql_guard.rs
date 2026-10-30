use super::*;

fn ok(sql: &str, db: DbType) -> AnalyzedSql {
    analyze(sql, db).unwrap_or_else(|e| panic!("expected {sql:?} to pass, got: {e}"))
}

fn rejected(sql: &str, db: DbType) -> String {
    analyze(sql, db).expect_err(&format!("expected {sql:?} to be rejected"))
}

fn tables(sql: &str, db: DbType) -> Vec<String> {
    ok(sql, db)
        .tables
        .into_iter()
        .map(|t| match t.schema {
            Some(schema) => format!("{schema}.{}", t.name),
            None => t.name,
        })
        .collect()
}

const ORA: DbType = DbType::Oracle;
const PG: DbType = DbType::Postgres;

#[test]
fn a_plain_select_passes_and_reports_its_table() {
    let analyzed = ok("SELECT id, name FROM orders WHERE id = 1;", ORA);
    assert_eq!(analyzed.sql, "SELECT id, name FROM orders WHERE id = 1");
    assert_eq!(tables("select * from orders", ORA), ["ORDERS"]);
}

#[test]
fn unquoted_names_fold_to_the_engine_case_and_quoted_ones_keep_theirs() {
    assert_eq!(tables("SELECT 1 FROM Orders", PG), ["orders"]);
    assert_eq!(tables("SELECT 1 FROM Orders", ORA), ["ORDERS"]);
    assert_eq!(tables("SELECT 1 FROM \"Orders\"", PG), ["Orders"]);
    assert_eq!(tables("SELECT 1 FROM app.Orders", ORA), ["APP.ORDERS"]);
}

#[test]
fn joins_and_comma_joins_list_every_table() {
    assert_eq!(
        tables(
            "SELECT * FROM orders o JOIN items i ON i.order_id = o.id LEFT JOIN notes n ON n.id = o.id",
            ORA
        ),
        ["ORDERS", "ITEMS", "NOTES"]
    );
    assert_eq!(
        tables("SELECT * FROM a x, b y, c WHERE x.id = y.id", ORA),
        ["A", "B", "C"]
    );
}

#[test]
fn a_comma_after_a_join_condition_still_starts_a_new_table() {
    assert_eq!(
        tables("SELECT * FROM a JOIN b ON a.id = b.id, secret", ORA),
        ["A", "B", "SECRET"]
    );
}

#[test]
fn subqueries_and_unions_are_scanned() {
    assert_eq!(
        tables("SELECT * FROM (SELECT * FROM inner_t) x, outer_t", ORA),
        ["INNER_T", "OUTER_T"]
    );
    assert_eq!(
        tables(
            "SELECT 1 FROM a WHERE id IN (SELECT id FROM b) UNION SELECT 2 FROM c",
            ORA
        ),
        ["A", "B", "C"]
    );
    assert_eq!(
        tables("SELECT (SELECT max(x) FROM hidden) FROM visible", ORA),
        ["HIDDEN", "VISIBLE"]
    );
}

#[test]
fn ctes_are_not_tables_but_their_bodies_are_scanned() {
    assert_eq!(
        tables(
            "WITH recent AS (SELECT * FROM orders) SELECT * FROM recent",
            PG
        ),
        ["orders"]
    );
    assert_eq!(
        tables(
            "WITH a AS (SELECT 1 FROM t1), b AS (SELECT 2 FROM t2) SELECT * FROM a, b",
            PG
        ),
        ["t1", "t2"]
    );
}

#[test]
fn from_inside_extract_and_trim_is_not_a_table() {
    assert_eq!(
        tables("SELECT EXTRACT(YEAR FROM created) FROM orders", PG),
        ["orders"]
    );
    assert_eq!(
        tables("SELECT TRIM(BOTH ' ' FROM name) FROM people", ORA),
        ["PEOPLE"]
    );
}

#[test]
fn dual_is_free_on_oracle_only() {
    assert!(tables("SELECT 1 FROM DUAL", ORA).is_empty());
    assert_eq!(tables("SELECT 1 FROM dual", PG), ["dual"]);
}

#[test]
fn row_limiting_clauses_are_fine() {
    ok("SELECT * FROM t ORDER BY id FETCH FIRST 5 ROWS ONLY", ORA);
    ok("SELECT * FROM t ORDER BY id LIMIT 5 OFFSET 2", PG);
}

#[test]
fn only_selects_are_accepted() {
    for sql in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET a = 1",
        "DELETE FROM t",
        "DROP TABLE t",
        "TRUNCATE TABLE t",
        "CREATE TABLE x (a int)",
        "ALTER TABLE t ADD c int",
        "GRANT ALL ON t TO x",
        "BEGIN NULL; END;",
        "CALL proc()",
        "EXPLAIN SELECT 1",
        "SET search_path = public",
        "COPY t TO '/tmp/x'",
        "",
    ] {
        rejected(sql, PG);
    }
}

#[test]
fn data_modifying_ctes_and_locking_selects_are_rejected() {
    rejected("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", PG);
    rejected("SELECT * FROM t FOR UPDATE", ORA);
    rejected("SELECT * INTO backup FROM t", PG);
    rejected("SELECT * FROM t; DROP TABLE t", PG);
}

#[test]
fn quoted_identifiers_named_like_keywords_are_fine() {
    ok("SELECT \"update\", \"delete\" FROM t", PG);
}

#[test]
fn multiple_statements_are_rejected_but_one_trailing_semicolon_is_ok() {
    rejected("SELECT 1; SELECT 2", ORA);
    rejected("SELECT 1;;", ORA);
    ok("SELECT 1 FROM DUAL ;  ", ORA);
    rejected("SELECT 1 FROM DUAL; -- trailing comment", ORA);
}

#[test]
fn semicolons_inside_strings_and_comments_do_not_count() {
    ok("SELECT 'a;b' FROM DUAL", ORA);
    ok("SELECT 1 /* ; DROP */ FROM DUAL", ORA);
    ok("SELECT 1 -- ; DROP\nFROM DUAL", ORA);
}

#[test]
fn table_names_hidden_in_comments_or_strings_are_not_misread() {
    assert_eq!(
        tables("SELECT 'FROM secret' FROM visible -- FROM other", ORA),
        ["VISIBLE"]
    );
    assert_eq!(
        tables("SELECT 1 /* FROM secret */ FROM visible", ORA),
        ["VISIBLE"]
    );
}

#[test]
fn postgres_nested_comments_cannot_smuggle_sql() {
    // Postgres treats the whole thing as one comment; Oracle ends it at the first */.
    let sql = "SELECT 1 FROM a /* x /* y */ FROM secret */";
    assert_eq!(tables(sql, PG), ["a"]);
    // On Oracle the text after the first */ is code and is analysed as such.
    assert_eq!(tables(sql, ORA), ["A", "SECRET"]);
}

#[test]
fn string_forms_the_tokenizer_cannot_follow_are_rejected() {
    rejected("SELECT E'a\\'b' FROM t", PG);
    rejected("SELECT q'[a]' FROM DUAL", ORA);
    rejected("SELECT $$x$$", PG);
    rejected("SELECT $1", PG);
    rejected("SELECT 'unterminated", ORA);
    rejected("SELECT \"unterminated", ORA);
    rejected("SELECT 1 /* open", ORA);
}

#[test]
fn table_functions_laterals_and_db_links_are_rejected() {
    rejected("SELECT * FROM generate_series(1, 10)", PG);
    rejected("SELECT * FROM TABLE(some_fn())", ORA);
    rejected("SELECT * FROM a, LATERAL (SELECT 1) x", PG);
    rejected("SELECT * FROM remote_t@link", ORA);
    rejected("SELECT * FROM a.b.c", PG);
    rejected("SELECT * FROM ", PG);
    rejected("SELECT * FROM *", PG);
}

#[test]
fn schema_qualified_tables_report_their_schema() {
    let analyzed = ok("SELECT * FROM other.t JOIN app.u ON 1=1", ORA);
    assert_eq!(analyzed.tables[0].schema.as_deref(), Some("OTHER"));
    assert_eq!(analyzed.tables[1].schema.as_deref(), Some("APP"));
}

#[test]
fn functions_that_read_outside_the_allowlist_are_rejected() {
    rejected(
        "SELECT query_to_xml('select * from secret', true, true, '')",
        PG,
    );
    rejected("SELECT dblink('host=x', 'select 1')", PG);
    rejected("SELECT pg_read_file('/etc/passwd')", PG);
    rejected(
        "SELECT DBMS_XMLGEN.getxml('select * from secret') FROM DUAL",
        ORA,
    );
    rejected("SELECT UTL_HTTP.request('http://x') FROM DUAL", ORA);
    rejected("SELECT pg_sleep(100)", PG);
    // DBMS_LOB can read BFILEs and temporary LOBs outside the table allowlist.
    rejected("SELECT DBMS_LOB.SUBSTR(body, 100, 1) FROM docs", ORA);
}

#[test]
fn quoted_function_names_do_not_bypass_the_blocklist() {
    rejected(
        r#"SELECT "query_to_xml"('select * from secret', true, false, '') FROM allowed_t"#,
        PG,
    );
    rejected(r#"SELECT "pg_read_file"('/etc/passwd') FROM allowed_t"#, PG);
    rejected(
        r#"SELECT pg_catalog."pg_read_file"('/etc/passwd') FROM allowed_t"#,
        PG,
    );
    rejected(r#"SELECT "PG_Read_File"('/etc/passwd') FROM allowed_t"#, PG);
    rejected(
        r#"SELECT "DBMS_XMLGEN".getxml('select * from hr.salaries') FROM allowed_t"#,
        ORA,
    );
    rejected(
        r#"SELECT sys."DBMS_XMLGEN".getxml('x') FROM allowed_t"#,
        ORA,
    );
}

#[test]
fn oracle_uri_xml_lob_metadata_and_context_functions_are_rejected() {
    for sql in [
        "SELECT DBURITYPE('/HR/SALARIES').getclob() FROM allowed_t",
        "SELECT HTTPURITYPE('http://x').getclob() FROM allowed_t",
        "SELECT XDBURITYPE('/x').getclob() FROM allowed_t",
        "SELECT XMLTYPE('<a/>') FROM allowed_t",
        "SELECT DBMS_XMLQUERY.getxml('select 1') FROM allowed_t",
        "SELECT DBMS_METADATA.get_ddl('TABLE', 'T') FROM allowed_t",
        "SELECT SYS_CONTEXT('USERENV', 'SESSION_USER') FROM allowed_t",
        r#"SELECT "DBURITYPE"('/HR/SALARIES').getclob() FROM allowed_t"#,
        r#"SELECT "dburitype"('/HR/SALARIES').getclob() FROM allowed_t"#,
    ] {
        rejected(sql, ORA);
    }
}

#[test]
fn unicode_escaped_identifiers_are_rejected() {
    // U&"\0071uery_to_xml" is "query_to_xml" once the engine decodes it.
    rejected(
        r#"SELECT U&"\0071uery_to_xml"('select * from secret', true, false, '') FROM allowed_t"#,
        PG,
    );
    rejected(
        r#"SELECT u&"\0071uery_to_xml"('x', true, false, '') FROM t"#,
        PG,
    );
    rejected("SELECT U&'\\0041' FROM allowed_t", PG);
}

#[test]
fn quoted_names_that_are_not_forbidden_still_pass() {
    ok(r#"SELECT "Name", "order" FROM "Orders""#, PG);
    // A quoted keyword is just an identifier.
    ok(r#"SELECT "INSERT" FROM orders"#, ORA);
}

#[test]
fn overlong_sql_is_rejected() {
    let sql = format!("SELECT '{}' FROM DUAL", "x".repeat(MAX_SQL_LEN));
    assert!(rejected(&sql, ORA).contains("longer than"));
}

#[test]
fn the_postgres_table_shorthand_is_rejected() {
    rejected("SELECT 1 UNION TABLE secret", PG);
    rejected("WITH x AS (SELECT 1) TABLE secret", PG);
    rejected("SELECT * FROM (TABLE secret) s", PG);
}

#[test]
fn parenthesised_joins_are_rejected_but_subqueries_are_fine() {
    rejected("SELECT * FROM (secret CROSS JOIN allowed) j", PG);
    rejected("SELECT * FROM a JOIN (secret JOIN b ON true) ON true", ORA);
    rejected("SELECT * FROM a, (secret) s", ORA);
    assert_eq!(
        tables(
            "SELECT * FROM (SELECT * FROM t) s JOIN (WITH q AS (SELECT 1 FROM u) SELECT * FROM q) w ON 1=1",
            PG
        ),
        // `q` is a nested CTE: not recognised, so it is (safely) treated as a table.
        ["t", "u", "q"]
    );
}

#[test]
fn only_top_level_ctes_shadow_real_tables() {
    // The CTE named `secret` lives inside the subquery, so the outer `secret` is a real table.
    let found = tables(
        "SELECT * FROM (WITH secret AS (SELECT 1 AS a FROM allowed_t) SELECT a FROM secret) s, secret",
        PG,
    );
    assert!(found.contains(&"secret".to_string()), "{found:?}");
    // A real top-level CTE is still recognised.
    assert_eq!(
        tables(
            "WITH r AS (SELECT * FROM t) SELECT * FROM r JOIN r r2 ON 1=1",
            PG
        ),
        ["t"]
    );
}

#[test]
fn oracle_national_alternative_quoting_is_rejected() {
    rejected("SELECT nq'[a']' FROM DUAL", ORA);
    rejected("SELECT NQ'(x)' FROM DUAL", ORA);
    rejected("SELECT Q'<x>' FROM DUAL", ORA);
}
