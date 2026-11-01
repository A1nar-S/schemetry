use super::*;

fn oracle(ddl: &str, object_type: &str, name: &str) -> Result<Vec<DdlStatement>, String> {
    validate(&normalize(ddl), object_type, name, "APP", DbType::Oracle)
}

fn postgres(ddl: &str, object_type: &str, name: &str) -> Result<Vec<DdlStatement>, String> {
    validate(
        &normalize(ddl),
        object_type,
        name,
        "sales",
        DbType::Postgres,
    )
}

fn rejected(result: Result<Vec<DdlStatement>, String>) -> String {
    result.expect_err("expected the DDL to be rejected")
}

const PROC: &str = "CREATE OR REPLACE PROCEDURE app.fix_it IS\nBEGIN\n  NULL;\nEND fix_it;\n/\n";

#[test]
fn oracle_procedure_is_accepted_and_trailing_slash_dropped() {
    let statements = oracle(PROC, "procedure", "fix_it").unwrap();
    assert_eq!(statements.len(), 1);
    assert_eq!(statements[0].object_type, "PROCEDURE");
    assert!(
        statements[0]
            .sql
            .starts_with("CREATE OR REPLACE PROCEDURE app.fix_it")
    );
    assert!(statements[0].sql.ends_with("END fix_it;"));
}

#[test]
fn oracle_view_loses_its_trailing_semicolon_and_comments() {
    let ddl = "-- fixes the report\nCREATE OR REPLACE VIEW v_orders AS SELECT 1 AS one FROM dual; -- done";
    let statements = oracle(ddl, "VIEW", "V_ORDERS").unwrap();
    assert_eq!(
        statements[0].sql,
        "CREATE OR REPLACE VIEW v_orders AS SELECT 1 AS one FROM dual"
    );
}

#[test]
fn oracle_package_spec_and_body_are_allowed_together_only() {
    let both = "CREATE OR REPLACE PACKAGE pkg AS\n  PROCEDURE p;\nEND pkg;\n/\n\
                CREATE OR REPLACE PACKAGE BODY pkg AS\n  PROCEDURE p IS BEGIN NULL; END;\nEND pkg;\n/";
    let statements = oracle(both, "PACKAGE", "PKG").unwrap();
    let kinds: Vec<_> = statements.iter().map(|s| s.object_type.as_str()).collect();
    assert_eq!(kinds, ["PACKAGE", "PACKAGE BODY"]);

    assert!(rejected(oracle(both, "PACKAGE BODY", "PKG")).contains("object_type PACKAGE"));
    // Another object smuggled in as the second statement.
    let mixed = "CREATE OR REPLACE PACKAGE pkg AS\nEND;\n/\nCREATE OR REPLACE PROCEDURE pkg IS BEGIN NULL; END;\n/";
    assert!(rejected(oracle(mixed, "PACKAGE", "PKG")).contains("Two statements"));
    // Different package name in the body.
    let other_name = both.replace("PACKAGE BODY pkg", "PACKAGE BODY other");
    assert!(rejected(oracle(&other_name, "PACKAGE", "PKG")).contains("creates OTHER"));
    // Without OR REPLACE the pair is refused.
    let plain = both.replace("CREATE OR REPLACE PACKAGE BODY", "CREATE PACKAGE BODY");
    assert!(rejected(oracle(&plain, "PACKAGE", "PKG")).contains("Two statements"));
    // Three statements are never fine.
    let three = format!("{both}\nCREATE OR REPLACE PACKAGE BODY pkg AS END;\n/");
    assert!(rejected(oracle(&three, "PACKAGE", "PKG")).contains("At most"));
}

#[test]
fn a_single_package_body_is_allowed_with_its_own_type() {
    let body = "CREATE OR REPLACE PACKAGE BODY pkg AS\nEND pkg;\n/";
    assert!(oracle(body, "package body", "pkg").is_ok());
    assert!(rejected(oracle(body, "PACKAGE", "pkg")).contains("object_type"));
}

#[test]
fn destructive_and_unrelated_statements_are_rejected() {
    for (ddl, ty) in [
        ("DROP TABLE orders", "TABLE"),
        ("TRUNCATE TABLE orders", "TABLE"),
        ("DELETE FROM orders", "TABLE"),
        ("GRANT DBA TO app", "VIEW"),
        ("ALTER SYSTEM SET x = 1", "VIEW"),
        ("ALTER TABLE orders DROP COLUMN x", "TABLE"),
        ("SELECT 1 FROM dual", "VIEW"),
        ("BEGIN NULL; END;", "PROCEDURE"),
        ("SET DEFINE OFF", "VIEW"),
    ] {
        let message = rejected(oracle(ddl, ty, "ORDERS"));
        assert!(message.contains("Only CREATE"), "{ddl}: {message}");
    }
}

#[test]
fn only_code_objects_and_views_can_be_created() {
    for kind in [
        "TABLE",
        "INDEX",
        "USER",
        "ROLE",
        "SEQUENCE",
        "DATABASE LINK",
        "JAVA SOURCE",
        "LIBRARY",
    ] {
        let ddl = format!("CREATE {kind} orders (id NUMBER)");
        let message = rejected(oracle(&ddl, kind, "ORDERS"));
        assert!(message.contains("cannot be deployed"), "{kind}: {message}");
    }
}

#[test]
fn a_second_statement_hidden_after_a_view_is_rejected() {
    let ddl = "CREATE OR REPLACE VIEW v AS SELECT 1 FROM dual; DROP TABLE orders;";
    assert!(rejected(oracle(ddl, "VIEW", "V")).contains("one statement"));
    let ddl = "CREATE OR REPLACE VIEW v AS SELECT 1 FROM dual;\n/\nDROP TABLE orders;\n/";
    assert!(rejected(oracle(ddl, "VIEW", "V")).contains("Only CREATE"));
}

#[test]
fn keywords_inside_literals_comments_and_quoted_names_are_not_statements() {
    let ddl = "CREATE OR REPLACE VIEW v AS SELECT 'x; DROP TABLE t' AS a, /* ; DROP */ \"DROP\" FROM dual";
    assert!(oracle(ddl, "VIEW", "V").is_ok());
    let q = "CREATE OR REPLACE PROCEDURE p IS\n  s VARCHAR2(100) := q'[it's; DROP TABLE t]';\nBEGIN NULL; END;\n/";
    assert!(oracle(q, "PROCEDURE", "P").is_ok());
    assert!(
        rejected(oracle(
            "CREATE OR REPLACE VIEW v AS SELECT 'oops FROM dual",
            "VIEW",
            "V"
        ))
        .contains("Unterminated")
    );
    assert!(
        rejected(oracle(
            "CREATE OR REPLACE VIEW v AS SELECT 1 /* never ends",
            "VIEW",
            "V"
        ))
        .contains("Unterminated")
    );
}

#[test]
fn the_declared_object_and_schema_must_match_the_ddl() {
    assert!(rejected(oracle(PROC, "PROCEDURE", "other")).contains("creates FIX_IT"));
    assert!(rejected(oracle(PROC, "FUNCTION", "fix_it")).contains("creates a PROCEDURE"));
    let foreign = PROC.replace("app.fix_it", "hr.fix_it");
    assert!(rejected(oracle(&foreign, "PROCEDURE", "fix_it")).contains("own schema"));
    let own = PROC.replace("app.fix_it", "\"APP\".fix_it");
    assert!(oracle(&own, "PROCEDURE", "fix_it").is_ok());
    let three_parts = PROC.replace("app.fix_it", "db.app.fix_it");
    assert!(
        rejected(oracle(&three_parts, "PROCEDURE", "fix_it")).contains("at most one qualifier")
    );
    // A quoted mixed-case name is a different object than the declared one.
    let quoted = PROC.replace("app.fix_it", "\"Fix_It\"");
    assert!(rejected(oracle(&quoted, "PROCEDURE", "fix_it")).contains("creates Fix_It"));
}

#[test]
fn oracle_modifiers_and_triggers_are_checked() {
    let view = "CREATE OR REPLACE FORCE EDITIONABLE VIEW v AS SELECT 1 FROM dual";
    assert!(oracle(view, "VIEW", "V").is_ok());

    let trigger = "CREATE OR REPLACE TRIGGER trg BEFORE INSERT ON orders FOR EACH ROW\nBEGIN :NEW.id := 1; END;\n/";
    assert!(oracle(trigger, "TRIGGER", "TRG").is_ok());
    let foreign = trigger.replace("ON orders", "ON hr.orders");
    assert!(rejected(oracle(&foreign, "TRIGGER", "TRG")).contains("schema APP"));
    let system = "CREATE OR REPLACE TRIGGER trg AFTER LOGON ON DATABASE\nBEGIN NULL; END;\n/";
    assert!(rejected(oracle(system, "TRIGGER", "TRG")).contains("Database-level"));
}

#[test]
fn without_or_replace_a_single_create_is_still_one_statement() {
    assert!(oracle("CREATE VIEW v AS SELECT 1 FROM dual", "VIEW", "V").is_ok());
}

#[test]
fn postgres_function_with_dollar_quoted_body_is_accepted() {
    let ddl = "CREATE OR REPLACE FUNCTION sales.total(a int) RETURNS int AS $body$\n\
               BEGIN\n  PERFORM 1; -- ; DROP TABLE x\n  RETURN a;\nEND;\n$body$ LANGUAGE plpgsql;";
    let statements = postgres(ddl, "FUNCTION", "total").unwrap();
    assert_eq!(statements.len(), 1);
    assert!(statements[0].sql.ends_with("LANGUAGE plpgsql"));
}

#[test]
fn postgres_names_follow_case_folding_and_schema_rules() {
    let view = "CREATE OR REPLACE VIEW Orders_V AS SELECT 1";
    assert!(postgres(view, "VIEW", "orders_v").is_ok());
    assert!(rejected(postgres(view, "VIEW", "Orders_V")).contains("creates orders_v"));
    assert!(
        postgres(
            "CREATE OR REPLACE VIEW \"Orders_V\" AS SELECT 1",
            "VIEW",
            "Orders_V"
        )
        .is_ok()
    );
    assert!(postgres("CREATE OR REPLACE VIEW sales.v AS SELECT 1", "VIEW", "v").is_ok());
    assert!(
        rejected(postgres(
            "CREATE OR REPLACE VIEW public.v AS SELECT 1",
            "VIEW",
            "v"
        ))
        .contains("own schema")
    );
}

#[test]
fn postgres_rejects_extra_statements_unsafe_languages_and_escape_strings() {
    assert!(
        rejected(postgres(
            "CREATE OR REPLACE VIEW v AS SELECT 1; DROP TABLE t;",
            "VIEW",
            "v"
        ))
        .contains("one statement")
    );
    assert!(rejected(postgres("DROP VIEW v", "VIEW", "v")).contains("Only CREATE"));
    let c = "CREATE OR REPLACE FUNCTION f() RETURNS int AS '$libdir/x', 'f' LANGUAGE c";
    assert!(rejected(postgres(c, "FUNCTION", "f")).contains("LANGUAGE c"));
    let py = "CREATE FUNCTION f() RETURNS int AS $$ return 1 $$ LANGUAGE plpython3u";
    assert!(rejected(postgres(py, "FUNCTION", "f")).contains("plpython3u"));
    let e = "CREATE OR REPLACE VIEW v AS SELECT E'\\x41'";
    assert!(rejected(postgres(e, "VIEW", "v")).contains("E'"));
    assert!(
        rejected(postgres(
            "CREATE OR REPLACE VIEW v AS SELECT $x$ never",
            "VIEW",
            "v"
        ))
        .contains("Unterminated")
    );
    // Oracle-only types are not offered for Postgres.
    assert!(
        rejected(postgres("CREATE PACKAGE p AS END", "PACKAGE", "p"))
            .contains("cannot be deployed")
    );
    // A lone `/` is a division in Postgres, not a terminator.
    assert!(postgres("CREATE OR REPLACE VIEW v AS SELECT 4\n/\n2", "VIEW", "v").is_ok());
}

#[test]
fn empty_ddl_and_nul_free_comments_only_are_rejected() {
    assert!(rejected(oracle("", "VIEW", "V")).contains("empty"));
    assert!(rejected(oracle("-- nothing\n/* here */", "VIEW", "V")).contains("empty"));
    assert!(rejected(oracle("/\n/\n", "VIEW", "V")).contains("empty"));
}

#[test]
fn normalize_makes_the_digest_independent_of_line_endings_and_padding() {
    assert_eq!(
        normalize("\r\nCREATE VIEW v\r\nAS SELECT 1  \r\n"),
        "CREATE VIEW v\nAS SELECT 1"
    );
}

#[test]
fn summary_names_the_statement_kinds() {
    let statements = oracle(
        "CREATE OR REPLACE PACKAGE p AS END;\n/\nCREATE OR REPLACE PACKAGE BODY p AS END;\n/",
        "PACKAGE",
        "P",
    )
    .unwrap();
    assert_eq!(summarize(&statements), "CREATE PACKAGE + PACKAGE BODY");
}

#[test]
fn a_lone_carriage_return_does_not_hide_a_second_statement() {
    let ddl = "CREATE VIEW v AS SELECT 1 AS a -- x\r; DROP TABLE t; SELECT 1 AS z -- \n , 2";
    assert!(rejected(postgres(ddl, "VIEW", "v")).contains("Only one statement"));
    // Even when the caller skips `normalize`, the comment ends at the `\r`.
    let raw = validate(ddl, "VIEW", "v", "sales", DbType::Postgres);
    assert!(rejected(raw).contains("Only one statement"));
}

#[test]
fn non_ascii_outside_strings_comments_and_quoted_names_is_refused() {
    for ddl in [
        "CREATE VIEW v AS SELECT 1 AS \u{20ac}$$; DROP TABLE public.victim; --$$",
        "CREATE VIEW v AS SELECT 1 AS \u{a0}$$; DROP TABLE public.victim; --$$",
        "CREATE VIEW v AS SELECT 1 AS $\u{20ac}$; DROP TABLE public.victim; --$\u{20ac}$",
        "CREATE VIEW v AS SELECT 1 AS a\u{2028}; DROP TABLE t",
    ] {
        assert!(
            rejected(postgres(ddl, "VIEW", "v")).contains("Non-ASCII"),
            "{ddl}"
        );
    }
    // Comments, strings and quoted identifiers may still carry any text.
    let fine = "-- lietot\u{101}js\nCREATE VIEW v AS SELECT '\u{20ac}' AS \"\u{101}\" FROM t";
    postgres(fine, "VIEW", "v").unwrap();
    // Postgres folds only ASCII, so the Kelvin sign is not a `k`.
    rejected(postgres(
        "CREATE VIEW \u{212a}ey AS SELECT 1",
        "VIEW",
        "key",
    ));
}

#[test]
fn a_number_cannot_run_into_a_dollar_quote() {
    for ddl in [
        "CREATE VIEW v AS SELECT 1e$$ ; DROP TABLE x; --$$",
        "CREATE VIEW v AS SELECT 1e$a$ ; DROP TABLE x; --$a$",
    ] {
        assert!(
            rejected(postgres(ddl, "VIEW", "v")).contains("directly followed"),
            "{ddl}"
        );
    }
}

#[test]
fn invisible_and_bidi_characters_are_refused_but_a_bom_is_dropped() {
    let spoof = "CREATE VIEW v AS SELECT 1 -- \u{202e}safe\n";
    assert!(rejected(postgres(spoof, "VIEW", "v")).contains("invisible"));
    postgres("\u{feff}CREATE VIEW v AS SELECT 1", "VIEW", "v").unwrap();
}

#[test]
fn only_sql_and_plpgsql_functions_can_be_deployed() {
    let function = |language: &str| {
        format!("CREATE FUNCTION f() RETURNS int LANGUAGE {language} AS 'select 1'")
    };
    postgres(&function("sql"), "FUNCTION", "f").unwrap();
    postgres(&function("plpgsql"), "FUNCTION", "f").unwrap();
    for language in ["pljavau", "plsh", "plperlu", "c"] {
        rejected(postgres(&function(language), "FUNCTION", "f"));
    }
}

#[test]
fn a_quoted_oracle_schema_must_match_exactly() {
    rejected(oracle(
        "CREATE VIEW \"app\".v AS SELECT 1 FROM dual",
        "VIEW",
        "V",
    ));
    oracle("CREATE VIEW app.v AS SELECT 1 FROM dual", "VIEW", "V").unwrap();
    oracle("CREATE VIEW \"APP\".v AS SELECT 1 FROM dual", "VIEW", "V").unwrap();
}

#[test]
fn a_hash_does_not_join_a_word_to_a_dollar_quote_on_postgres() {
    let ddl = "CREATE VIEW v AS SELECT i#$$ 2 $$ FROM (SELECT 1 AS i) s ; DROP TABLE t; -- $$";
    assert!(rejected(postgres(ddl, "VIEW", "v")).contains("Only one statement"));
    // Oracle identifiers may contain `#`.
    oracle("CREATE VIEW v AS SELECT col# FROM t", "VIEW", "V").unwrap();
}

#[test]
fn oracle_external_routines_are_refused() {
    for ddl in [
        "CREATE FUNCTION f RETURN NUMBER AS LANGUAGE JAVA NAME 'x.y() return int';",
        "CREATE FUNCTION f RETURN NUMBER AS LANGUAGE C LIBRARY l NAME \"n\";",
    ] {
        assert!(
            rejected(oracle(ddl, "FUNCTION", "F")).contains("External routines"),
            "{ddl}"
        );
    }
}

#[test]
fn oracle_database_level_triggers_and_wrapped_source_are_refused() {
    for ddl in [
        "CREATE OR REPLACE TRIGGER v AFTER LOGON ON PLUGGABLE DATABASE BEGIN NULL; END;",
        "CREATE OR REPLACE TRIGGER v INSTEAD OF INSERT ON NESTED TABLE c OF other.vw BEGIN NULL; END;",
        "CREATE OR REPLACE PROCEDURE v wrapped\na000000\n1\nabcd",
    ] {
        rejected(oracle(
            ddl,
            if ddl.contains("TRIGGER") {
                "TRIGGER"
            } else {
                "PROCEDURE"
            },
            "V",
        ));
    }
}
