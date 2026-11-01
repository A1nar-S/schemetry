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

#[test]
fn unknown_functions_are_rejected_by_name_with_an_allowlist_message() {
    let message = rejected("SELECT my_secret_fn(1) FROM t", PG);
    assert!(message.contains("my_secret_fn"), "{message}");
    assert!(message.contains("allowlist"), "{message}");
    let message = rejected("SELECT pkg.func(1) FROM t", ORA);
    assert!(message.contains("pkg.func"), "{message}");
    assert!(
        message.contains("not on the run_query function allowlist"),
        "{message}"
    );
    // A blocklisted name says so as well, and still names the function.
    let message = rejected("SELECT pg_read_file('/etc/passwd')", PG);
    assert!(
        message.contains("PG_READ_FILE") && message.contains("allowlist"),
        "{message}"
    );
}

#[test]
fn object_type_constructors_and_member_calls_are_rejected() {
    for sql in [
        "SELECT my_type(1, 2) FROM allowed_t",
        "SELECT my_type(1, 2).total() FROM allowed_t",
        "SELECT t.geom.get_wkt() FROM allowed_t t",
        "SELECT (SELECT x FROM allowed_t).getclob() FROM DUAL",
        "SELECT UPPER('a').getclob() FROM DUAL",
        "SELECT sys.dbms_xmlgen.getxml('x') FROM DUAL",
        "SELECT hr.helper_fn(1) FROM DUAL",
        r#"SELECT "DBURITYPE"('/HR/SALARIES').getclob() FROM allowed_t"#,
        "SELECT CURSOR(SELECT 1 FROM allowed_t) FROM DUAL",
        "SELECT XMLQUERY('1' RETURNING CONTENT) FROM DUAL",
        "SELECT seq.NEXTVAL FROM DUAL",
        "SELECT seq.CURRVAL FROM DUAL",
        "SELECT UTL_INADDR.GET_HOST_NAME FROM DUAL",
        "SELECT dbms_utility.get_time FROM DUAL",
        "SELECT * FROM t PIVOT (SUM(x) FOR y IN (1))",
    ] {
        rejected(sql, ORA);
    }
}

#[test]
fn postgres_bypass_attempts_are_rejected() {
    for sql in [
        r#"SELECT "query_to_xml"('select * from secret', true, false, '') FROM t"#,
        r#"SELECT "pg_read_file"('/etc/passwd') FROM t"#,
        "SELECT pg_catalog.pg_read_file('/etc/passwd')",
        "SELECT pg_catalog.set_config('log_statement', 'all', false)",
        "SELECT public.my_fn(1)",
        "SELECT nextval('s')",
        "SELECT lo_import('/etc/passwd')",
        "SELECT set_config('a.b', 'c', false)",
        "SELECT pg_sleep(1)",
        "SELECT pg_ls_dir('.')",
        "SELECT current_setting('x')",
        "SELECT (t).f() FROM t",
        "SELECT query_to_xml('select 1', true, true, '')",
        "SELECT xmlparse(document '<a/>')",
        "SELECT * FROM generate_series(1, 10)",
        "SELECT random()",
        "SELECT 1 FROM t x(a, b)",
        r#"SELECT pg_catalog."Pg_Read_File"('x')"#,
    ] {
        rejected(sql, PG);
    }
}

#[test]
fn quoted_function_names_are_case_sensitive_like_the_engine() {
    // On Postgres only an all-lower-case quoted name is the built-in.
    ok(r#"SELECT "count"(*) FROM t"#, PG);
    ok(r#"SELECT pg_catalog."lower"(name) FROM t"#, PG);
    ok(r#"SELECT "pg_catalog"."lower"(name) FROM t"#, PG);
    rejected(r#"SELECT "COUNT"(*) FROM t"#, PG);
    rejected(r#"SELECT "Lower"(name) FROM t"#, PG);
    // On Oracle only an all-upper-case quoted name is.
    ok(r#"SELECT "COUNT"(*) FROM t"#, ORA);
    rejected(r#"SELECT "count"(*) FROM t"#, ORA);
    rejected(r#"SELECT "Upper"(name) FROM t"#, ORA);
    // A quoted name that merely contains a dot is not a qualified call.
    rejected(r#"SELECT "count.x"(*) FROM t"#, PG);
    // Non-ASCII look-alikes never match.
    rejected("SELECT c\u{0131}unt(1) FROM t", PG);
    let message = rejected(r#"SELECT "Evil""Fn"(1) FROM t"#, PG);
    assert!(message.contains("Evil"), "{message}");
}

#[test]
fn engine_specific_functions_are_only_allowed_on_their_engine() {
    ok("SELECT NVL(a, 0), DECODE(a, 1, 'x', 'y') FROM t", ORA);
    rejected("SELECT NVL(a, 0) FROM t", PG);
    ok("SELECT string_agg(a, ',') FROM t", PG);
    rejected("SELECT string_agg(a, ',') FROM t", ORA);
    ok("SELECT date_trunc('day', created) FROM t", PG);
    rejected("SELECT date_trunc('day', created) FROM t", ORA);
}

#[test]
fn legitimate_oracle_queries_pass() {
    for sql in [
        "SELECT COUNT(*), SUM(amount), AVG(amount), MIN(created), MAX(created) FROM orders",
        "SELECT NVL(name, 'n/a'), NVL2(a, b, c), COALESCE(a, b, c), NULLIF(a, 0) FROM t",
        "SELECT DECODE(status, 'A', 1, 'B', 2, 0) FROM t",
        "SELECT UPPER(name), LOWER(name), INITCAP(name), TRIM(name), LTRIM(name), RTRIM(name) FROM t",
        "SELECT SUBSTR(name, 1, 3), LENGTH(name), REPLACE(name, 'a', 'b'), INSTR(name, 'x') FROM t",
        "SELECT TRIM(BOTH ' ' FROM name), TRIM(LEADING '0' FROM code) FROM t",
        "SELECT ROUND(x, 2), TRUNC(x), TRUNC(created, 'MM'), MOD(x, 2), CEIL(x), FLOOR(x), ABS(x) FROM t",
        "SELECT TO_CHAR(created, 'YYYY-MM-DD'), TO_DATE('2024-01-01', 'YYYY-MM-DD'), TO_NUMBER(s) FROM t",
        "SELECT CAST(x AS NUMBER(10, 2)), CAST(x AS VARCHAR2(30)), CAST(d AS TIMESTAMP(6)) FROM t",
        "SELECT EXTRACT(YEAR FROM created), EXTRACT(MONTH FROM created) FROM t",
        "SELECT SYSDATE, SYSTIMESTAMP, CURRENT_DATE, CURRENT_TIMESTAMP(3) FROM DUAL",
        "SELECT ADD_MONTHS(created, 1), MONTHS_BETWEEN(a, b), LAST_DAY(created) FROM t",
        "SELECT ROW_NUMBER() OVER (PARTITION BY dept ORDER BY id) FROM t",
        "SELECT RANK() OVER (ORDER BY x DESC), DENSE_RANK() OVER (ORDER BY x) FROM t",
        "SELECT LAG(x, 1, 0) OVER (ORDER BY id), LEAD(x) OVER (ORDER BY id) FROM t",
        "SELECT SUM(x) OVER (PARTITION BY a ORDER BY id ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t",
        "SELECT FIRST_VALUE(x) OVER (PARTITION BY a ORDER BY id), NTILE(4) OVER (ORDER BY x) FROM t",
        "SELECT LISTAGG(name, ', ') WITHIN GROUP (ORDER BY name) FROM t",
        "SELECT MAX(x) KEEP (DENSE_RANK FIRST ORDER BY y) FROM t",
        "SELECT dept, COUNT(*) FROM t GROUP BY ROLLUP (dept)",
        "SELECT a, b, SUM(x) FROM t GROUP BY GROUPING SETS ((a), (b))",
        "SELECT REGEXP_SUBSTR(s, '[0-9]+'), REGEXP_REPLACE(s, 'a', 'b'), REGEXP_COUNT(s, 'a') FROM t",
        "SELECT CASE WHEN x > 1 THEN 'a' WHEN (y IS NULL) THEN 'b' ELSE 'c' END FROM t",
        "SELECT * FROM t WHERE id IN (1, 2, 3) AND (a = 1 OR b = 2) AND NOT (c = 3)",
        "SELECT * FROM t WHERE EXISTS (SELECT 1 FROM u WHERE u.id = t.id)",
        "SELECT * FROM t WHERE x > ALL (SELECT y FROM u) OR x = ANY (SELECT y FROM u)",
        "SELECT * FROM t WHERE name LIKE 'a%' ESCAPE '\\' AND d BETWEEN (1) AND (2)",
        "SELECT * FROM a UNION ALL (SELECT * FROM b)",
        "SELECT * FROM a JOIN b ON (a.id = b.id) JOIN c USING (id)",
        "SELECT LEAST(a, b), GREATEST(a, b, c), SIGN(x), POWER(x, 2), SQRT(x), LN(x) FROM t",
        "SELECT NUMTODSINTERVAL(5, 'DAY'), TO_TIMESTAMP('2024-01-01', 'YYYY-MM-DD') FROM DUAL",
        "SELECT JSON_VALUE(doc, '$.a'), JSON_OBJECT('a' VALUE x) FROM t",
        "SELECT * FROM t ORDER BY id FETCH FIRST 5 ROWS ONLY",
        "WITH r(n) AS (SELECT 1 FROM DUAL) SELECT n FROM r",
        "WITH a(x, y) AS (SELECT 1, 2 FROM DUAL), b AS (SELECT 3 FROM DUAL) SELECT * FROM a, b",
        "SELECT count (*) FROM t",
        "SELECT count/**/(*) FROM t",
        r#"SELECT "Name", COUNT(*) FROM "Orders" GROUP BY "Name""#,
    ] {
        ok(sql, ORA);
    }
}

#[test]
fn legitimate_postgres_queries_pass() {
    for sql in [
        "SELECT count(*), sum(x), avg(x), min(x), max(x), count(DISTINCT x) FROM t",
        "SELECT coalesce(a, b), nullif(a, 0), greatest(a, b), least(a, b) FROM t",
        "SELECT upper(n), lower(n), trim(n), btrim(n), ltrim(n), rtrim(n), initcap(n) FROM t",
        "SELECT substring(n FROM 1 FOR 3), substring(n, 1, 3), substr(n, 1), length(n) FROM t",
        "SELECT left(n, 2), right(n, 2), split_part(n, ',', 1), strpos(n, 'a'), replace(n, 'a', 'b') FROM t",
        "SELECT concat(a, b), concat_ws(',', a, b), format('%s-%s', a, b), md5(n) FROM t",
        "SELECT round(x, 2), trunc(x), ceil(x), ceiling(x), floor(x), abs(x), mod(x, 2), power(x, 2) FROM t",
        "SELECT to_char(created, 'YYYY'), to_date(s, 'YYYY-MM-DD'), to_number(s, '999'), to_timestamp(s, 'x') FROM t",
        "SELECT x::numeric(10, 2), CAST(x AS varchar(30)), CAST(x AS character varying(30)), x::timestamp(3) FROM t",
        "SELECT extract(year FROM created), date_part('month', created), date_trunc('day', created) FROM t",
        "SELECT now(), current_date, current_timestamp, age(a, b), make_date(2024, 1, 1) FROM t",
        "SELECT row_number() OVER (PARTITION BY a ORDER BY b), rank() OVER w FROM t WINDOW w AS (ORDER BY id)",
        "SELECT lag(x) OVER (ORDER BY id), lead(x, 2, 0) OVER (ORDER BY id) FROM t",
        "SELECT count(*) FILTER (WHERE x > 1) FROM t",
        "SELECT string_agg(n, ',' ORDER BY n), array_agg(n), bool_and(b) FROM t",
        "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY x) FROM t",
        "SELECT array_length(arr, 1), array_to_string(arr, ','), cardinality(arr), ARRAY[1, 2] FROM t",
        "SELECT ARRAY(SELECT id FROM u) FROM t",
        "SELECT to_json(t), row_to_json(t), jsonb_build_object('a', x), jsonb_agg(x), jsonb_typeof(j) FROM t",
        "SELECT pg_catalog.lower(n), PG_CATALOG.count(*) FROM t",
        "SELECT * FROM t WHERE id IN (SELECT id FROM u) AND x = ANY (ARRAY[1, 2])",
        "SELECT DISTINCT ON (a) a, b FROM t ORDER BY a, b",
        "SELECT * FROM t LIMIT (5) OFFSET (2)",
        "SELECT a, b, sum(x) FROM t GROUP BY CUBE (a, b)",
        "SELECT x FROM t WHERE (a, b) IN (SELECT a, b FROM u)",
        "SELECT ROW(a, b) FROM t",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT n FROM r",
        "WITH a AS (SELECT 1), b(x) AS (SELECT 2) SELECT * FROM a, b",
        r#"SELECT "Name", "count"(*) FROM "Orders" GROUP BY "Name""#,
    ] {
        ok(sql, PG);
    }
}

#[test]
fn a_cte_header_is_not_a_function_call_but_a_call_posing_as_one_is_rejected() {
    assert_eq!(
        tables("WITH r(n) AS (SELECT n FROM t) SELECT * FROM r", PG),
        ["t"]
    );
    rejected(
        "SELECT 1 FROM t WHERE x IN (WITH q(n) AS (SELECT 1) SELECT n FROM q)",
        PG,
    );
    rejected("SELECT 1 AS x, my_fn(1) AS (SELECT 1)", PG);
    rejected("WITH r(my_fn(1)) AS (SELECT 1) SELECT * FROM r", PG);
}

#[test]
fn the_oracle_package_families_are_blocked_even_without_parentheses() {
    for sql in [
        "SELECT dbms_random.value FROM DUAL",
        "SELECT UTL_HTTP.request FROM DUAL",
        r#"SELECT "DBMS_RANDOM".value FROM DUAL"#,
        r#"SELECT "UTL_FILE"(1) FROM DUAL"#,
    ] {
        rejected(sql, ORA);
    }
}

#[test]
fn a_number_cannot_swallow_the_keyword_after_it() {
    for db in [ORA, PG] {
        rejected("SELECT 2FROM secret", db);
        rejected("SELECT 1.FROM secret", db);
        rejected("SELECT 1e5FROM secret", db);
    }
    assert_eq!(tables("SELECT 1e5, 2.5E-3, 7 FROM t", PG), ["t"]);
    assert_eq!(tables("SELECT 1.5f, 2d FROM t", ORA), ["T"]);
}

#[test]
fn a_lone_carriage_return_ends_a_line_comment() {
    for db in [ORA, PG] {
        rejected("SELECT 1 FROM t -- x\r; DROP TABLE t", db);
    }
}

#[test]
fn a_cte_cannot_hide_the_real_table_it_reads() {
    for db in [ORA, PG] {
        rejected(
            "WITH secret AS (SELECT * FROM secret) SELECT * FROM secret",
            db,
        );
        rejected(
            "WITH a AS (SELECT * FROM b), b AS (SELECT 1 AS x FROM dual) SELECT * FROM a",
            db,
        );
        rejected("WITH a(x) AS (SELECT x FROM a) SELECT * FROM a", db);
    }
    assert_eq!(
        tables(
            "WITH a AS (SELECT * FROM t), b AS (SELECT * FROM a) SELECT * FROM b",
            PG
        ),
        ["t"]
    );
}

#[test]
fn a_window_name_does_not_hide_a_table() {
    let sql = "SELECT count(*) OVER secret FROM secret WINDOW secret AS ()";
    assert_eq!(tables(sql, PG), ["secret"]);
}

#[test]
fn user_functions_named_like_soft_keywords_are_checked() {
    rejected("SELECT filter('x') FROM t", PG);
    rejected("SELECT range('x') FROM t", PG);
    rejected("SELECT start('x') FROM t", ORA);
    ok("SELECT count(*) FILTER (WHERE x > 1) FROM t", PG);
    ok("SELECT SUM(x) OVER (PARTITION BY y ORDER BY z) FROM t", ORA);
}

#[test]
fn a_quoted_cte_name_does_not_hide_a_differently_cased_table() {
    assert_eq!(
        tables(
            "WITH \"Secret\" AS (SELECT 1 AS x) SELECT * FROM secret",
            PG
        ),
        ["secret"]
    );
    assert_eq!(
        tables(
            "WITH \"secret\" AS (SELECT 1 x FROM dual) SELECT * FROM secret",
            ORA
        ),
        ["SECRET"]
    );
    assert_eq!(
        tables(
            "WITH secret AS (SELECT 1 AS x) SELECT * FROM \"Secret\"",
            PG
        ),
        ["Secret"]
    );
    assert_eq!(tables("SELECT * FROM \"dual\"", ORA), ["dual"]);
}

#[test]
fn an_alias_named_like_a_clause_does_not_end_the_from_list() {
    assert_eq!(tables("SELECT * FROM a start, secret", PG), ["a", "secret"]);
    assert_eq!(
        tables("SELECT * FROM a AS minus, secret", PG),
        ["a", "secret"]
    );
    assert_eq!(
        tables("SELECT * FROM a connect, secret", PG),
        ["a", "secret"]
    );
    assert_eq!(
        tables("SELECT * FROM a qualify, secret", ORA),
        ["A", "SECRET"]
    );
    assert_eq!(
        tables("SELECT * FROM (SELECT 1 FROM a start, secret) q", PG),
        ["a", "secret"]
    );
    assert_eq!(
        tables("SELECT * FROM a WHERE x IN (1, 2) ORDER BY x, y", PG),
        ["a"]
    );
}

#[test]
fn non_ascii_outside_strings_and_quoted_names_is_refused() {
    for db in [ORA, PG] {
        rejected("SELECT 1 FROM t WHERE x = 1\u{a0}OR y = 2", db);
        rejected("SELECT 1 FROM \u{20ac}$$secret", db);
        ok(
            "SELECT 'l\u{101}tvija' AS \"\u{101}\" FROM t -- \u{101}\n",
            db,
        );
    }
}

#[test]
fn a_keyword_used_as_a_label_does_not_confuse_the_table_scan() {
    assert_eq!(
        tables(
            "WITH \"from\" AS (SELECT 1) SELECT x AS from FROM secret s",
            PG
        ),
        ["secret"]
    );
    assert_eq!(
        tables("SELECT * FROM a b JOIN a c ON b.id = c.group, secret", PG),
        ["a", "a", "secret"]
    );
    assert_eq!(
        tables(
            "SELECT * FROM (SELECT 1 AS \"order\") b JOIN (SELECT 1 AS x) c ON b.order = c.x, secret",
            PG
        ),
        ["secret"]
    );
}

#[test]
fn an_oracle_recursive_cte_needs_a_top_level_union_all() {
    rejected(
        "WITH secret(x) AS (SELECT x FROM secret UNION SELECT 1 FROM dual) SELECT * FROM secret",
        ORA,
    );
    rejected(
        "WITH secret(x) AS (SELECT x FROM secret WHERE x IN (SELECT 1 FROM dual UNION ALL SELECT 2 FROM dual)) SELECT * FROM secret",
        ORA,
    );
    ok(
        "WITH r(n) AS (SELECT 1 FROM dual UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT n FROM r",
        ORA,
    );
}
