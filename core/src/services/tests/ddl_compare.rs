use std::fs;

use super::*;

const SCHEMA: &str = "APP";

fn oracle(text: &str) -> String {
    normalize_ddl(text, DbType::Oracle, SCHEMA)
}

// ── relative paths ──────────────────────────────────────────────────────────────

#[test]
fn relative_paths_are_cleaned() {
    assert_eq!(
        validate_relative_path("code/app/ORDERS.sql").unwrap(),
        PathBuf::from("code").join("app").join("ORDERS.sql")
    );
    assert_eq!(
        validate_relative_path(" ./code\\app//ORDERS.sql ").unwrap(),
        PathBuf::from("code").join("app").join("ORDERS.sql")
    );
}

#[test]
fn escaping_or_odd_paths_are_rejected() {
    for bad in [
        "",
        "   ",
        "/etc/passwd",
        "\\windows\\system32",
        "C:\\secrets.sql",
        "C:secrets.sql",
        "../outside.sql",
        "code/../../outside.sql",
        "code\\..\\..\\outside.sql",
        "a/b/..",
        ".",
        "./",
        "file.sql:stream",
        "bad\0name.sql",
        "new\nline.sql",
    ] {
        assert!(validate_relative_path(bad).is_err(), "{bad:?} was accepted");
    }
    assert!(validate_relative_path(&"a".repeat(MAX_FILE_PATH_CHARS + 1)).is_err());
}

// ── confined reads ──────────────────────────────────────────────────────────────

#[test]
fn files_inside_the_folder_are_read() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("code")).unwrap();
    fs::write(dir.path().join("code").join("a.sql"), b"select 1").unwrap();
    let bytes = read_confined_file(dir.path(), &PathBuf::from("code").join("a.sql"), 100).unwrap();
    assert_eq!(bytes, b"select 1");
}

#[test]
fn missing_oversized_and_non_file_targets_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("big.sql"), vec![b'x'; 101]).unwrap();

    assert!(matches!(
        read_confined_file(dir.path(), Path::new("nope.sql"), 100),
        Err(FileAccessError::Failed(_))
    ));
    assert!(matches!(
        read_confined_file(dir.path(), Path::new("big.sql"), 100),
        Err(FileAccessError::Denied(_))
    ));
    assert!(matches!(
        read_confined_file(dir.path(), Path::new("sub"), 100),
        Err(FileAccessError::Denied(_))
    ));
    assert!(matches!(
        read_confined_file(&dir.path().join("missing"), Path::new("a.sql"), 100),
        Err(FileAccessError::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_leaving_the_folder_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.sql"), b"secret").unwrap();
    let schema_dir = root.path().join("app");
    fs::create_dir_all(&schema_dir).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.sql"),
        schema_dir.join("link.sql"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), schema_dir.join("dir")).unwrap();

    assert!(matches!(
        read_confined_file(&schema_dir, Path::new("link.sql"), 100),
        Err(FileAccessError::Denied(_))
    ));
    assert!(matches!(
        read_confined_file(&schema_dir, &PathBuf::from("dir").join("secret.sql"), 100),
        Err(FileAccessError::Denied(_))
    ));
}

// ── decoding ────────────────────────────────────────────────────────────────────

#[test]
fn decoding_follows_the_configured_encoding() {
    assert_eq!(decode_ddl_file("ā".as_bytes(), "utf8").unwrap(), "ā");
    assert_eq!(
        decode_ddl_file(b"\xEF\xBB\xBFselect", "utf8-bom").unwrap(),
        "select"
    );
    // The BOM is tolerated even when the folder is configured without one.
    assert_eq!(
        decode_ddl_file(b"\xEF\xBB\xBFselect", "utf8").unwrap(),
        "select"
    );
    // 0xE2 is "ā" in Windows-1257.
    assert_eq!(decode_ddl_file(&[0xE2], "windows-1257").unwrap(), "ā");
    assert_eq!(decode_ddl_file(&[0xE2], "WINDOWS-1257").unwrap(), "ā");
    assert!(decode_ddl_file(&[0xE2], "utf8").is_err());
}

// ── generic normalisation ───────────────────────────────────────────────────────

#[test]
fn line_endings_trailing_whitespace_and_blank_edges_are_ignored() {
    let a = "\u{feff}\r\n\r\nSELECT 1  \r\n  FROM dual\t\r\n\r\n\r\n";
    let b = "SELECT 1\nFROM dual";
    for db in [DbType::Oracle, DbType::Postgres] {
        // Leading indentation is meaningful and kept.
        assert_eq!(normalize_ddl(a, db, SCHEMA), "SELECT 1\n  FROM dual");
        assert_ne!(normalize_ddl(a, db, SCHEMA), normalize_ddl(b, db, SCHEMA));
        assert_eq!(
            normalize_ddl("a\r\nb\rc", db, SCHEMA),
            normalize_ddl("a\nb\nc\n", db, SCHEMA)
        );
    }
}

#[test]
fn a_final_semicolon_is_ignored_but_inner_ones_are_not() {
    for db in [DbType::Oracle, DbType::Postgres] {
        assert_eq!(
            normalize_ddl("select 1;\nselect 2;\n", db, SCHEMA),
            normalize_ddl("select 1;\nselect 2", db, SCHEMA)
        );
        assert_ne!(
            normalize_ddl("select 1;\nselect 2", db, SCHEMA),
            normalize_ddl("select 1\nselect 2", db, SCHEMA)
        );
    }
}

#[test]
fn a_trailing_slash_terminator_is_oracle_only() {
    assert_eq!(oracle("select 1\n/\n"), oracle("select 1"));
    assert_eq!(oracle("end;\n/"), oracle("end;"));
    assert_ne!(
        normalize_ddl("select 1\n/", DbType::Postgres, SCHEMA),
        normalize_ddl("select 1", DbType::Postgres, SCHEMA)
    );
}

#[test]
fn normalisation_is_idempotent() {
    for text in [
        "create or replace   package body \"APP\".\"PKG\" as\r\nend;\r\n/\r\n",
        "CREATE TABLE app.orders (id number);",
        "garbage",
        "",
    ] {
        let once = oracle(text);
        assert_eq!(oracle(&once), once, "{text:?}");
    }
}

// ── Oracle header ───────────────────────────────────────────────────────────────

#[test]
fn oracle_header_noise_is_tolerated() {
    let canonical = "CREATE VIEW ORDERS_V AS\nSELECT 1 FROM dual";
    for header in [
        "CREATE VIEW ORDERS_V AS",
        "create view orders_v as",
        "CREATE OR REPLACE VIEW ORDERS_V AS",
        "create  or\n  replace   view   orders_v   as",
        "CREATE OR REPLACE EDITIONABLE VIEW ORDERS_V AS",
        "CREATE VIEW \"APP\".\"ORDERS_V\" AS",
        "CREATE OR REPLACE VIEW app.orders_v AS",
        "CREATE VIEW \"ORDERS_V\" AS",
    ] {
        let text = format!("{header}\nSELECT 1 FROM dual\n");
        assert_eq!(oracle(&text), canonical, "{header}");
    }
}

#[test]
fn oracle_header_handles_package_bodies_and_triggers() {
    assert_eq!(
        oracle("CREATE OR REPLACE EDITIONABLE PACKAGE BODY \"APP\".\"PKG\" AS\nEND;"),
        oracle("create package body pkg as\nEND")
    );
    assert_eq!(
        oracle("CREATE OR REPLACE TRIGGER \"APP\".\"TRG\"\nBEFORE INSERT ON T\nBEGIN NULL; END;"),
        oracle("create trigger trg\nBEFORE INSERT ON T\nBEGIN NULL; END;")
    );
    assert_ne!(
        oracle("CREATE PACKAGE BODY pkg AS\nEND;"),
        oracle("CREATE PACKAGE pkg AS\nEND;")
    );
}

#[test]
fn oracle_header_differences_that_matter_are_kept() {
    // Another schema's prefix, different names/types, NONEDITIONABLE and quoted
    // mixed-case names are real differences.
    let base = oracle("CREATE VIEW v AS SELECT 1 FROM dual");
    for other in [
        "CREATE VIEW OTHER.v AS SELECT 1 FROM dual",
        "CREATE VIEW w AS SELECT 1 FROM dual",
        "CREATE NONEDITIONABLE VIEW v AS SELECT 1 FROM dual",
        "CREATE VIEW \"v\" AS SELECT 1 FROM dual",
        "CREATE FORCE VIEW v AS SELECT 1 FROM dual",
        "CREATE VIEW v AS SELECT 2 FROM dual",
    ] {
        assert_ne!(oracle(other), base, "{other}");
    }
    assert_ne!(
        oracle("CREATE TABLE t (a number)"),
        oracle("CREATE VIEW t (a number)")
    );
}

#[test]
fn text_without_a_create_header_is_left_alone() {
    assert_eq!(
        oracle("ALTER TABLE t ADD x NUMBER"),
        "ALTER TABLE t ADD x NUMBER"
    );
    assert_eq!(oracle("CREATE ORDERS"), "CREATE ORDERS");
    assert_eq!(oracle("CREATE OR REPLACE"), "CREATE OR REPLACE");
    assert_eq!(oracle("CREATE TABLE"), "CREATE TABLE");
}

#[test]
fn the_header_is_not_normalised_for_postgres() {
    assert_ne!(
        normalize_ddl(
            "CREATE OR REPLACE VIEW v AS SELECT 1",
            DbType::Postgres,
            "app"
        ),
        normalize_ddl("CREATE VIEW v AS SELECT 1", DbType::Postgres, "app")
    );
}

#[test]
fn spacing_behind_the_name_is_collapsed() {
    assert_eq!(
        oracle("CREATE SEQUENCE \"APP\".\"SEQ\"    MINVALUE 1  MAXVALUE 9"),
        "CREATE SEQUENCE SEQ MINVALUE 1  MAXVALUE 9"
    );
    assert_eq!(
        oracle("CREATE TABLE \"APP\".\"ORDERS\" \n   (\"ID\" NUMBER)"),
        "CREATE TABLE ORDERS\n   (\"ID\" NUMBER)"
    );
}

// ── diff ────────────────────────────────────────────────────────────────────────

fn diff(old: &str, new: &str) -> TextDiff {
    unified_diff(old, new, "server", "file", 400).unwrap()
}

#[test]
fn identical_texts_have_no_diff() {
    assert!(unified_diff("a\nb", "a\nb", "s", "f", 10).is_none());
}

#[test]
fn a_changed_line_is_a_removal_and_an_addition() {
    let d = diff("a\nb\nc", "a\nB\nc");
    assert_eq!((d.added, d.removed, d.hunks), (1, 1, 1));
    assert_eq!(
        d.unified,
        "--- server\n+++ file\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c"
    );
    assert!(!d.truncated);
    assert_eq!(d.output_lines, 7);
}

#[test]
fn pure_additions_and_removals_are_counted() {
    let added = diff("a\nc", "a\nb\nc");
    assert_eq!((added.added, added.removed), (1, 0));
    assert!(added.unified.contains("\n+b\n"));
    let removed = diff("a\nb\nc", "a\nc");
    assert_eq!((removed.added, removed.removed), (0, 1));
    assert!(removed.unified.contains("\n-b\n"));
}

#[test]
fn distant_changes_make_separate_hunks_with_context() {
    let old: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
    let mut new = old.clone();
    new[1] = "changed 2".into();
    new[27] = "changed 28".into();
    let d = diff(&old.join("\n"), &new.join("\n"));
    assert_eq!((d.added, d.removed, d.hunks), (2, 2, 2));
    assert!(d.unified.contains("@@ -1,5 +1,5 @@"), "{}", d.unified);
    assert!(d.unified.contains("@@ -25,6 +25,6 @@"), "{}", d.unified);
    assert!(!d.unified.contains("line 15"));
}

#[test]
fn empty_sides_use_the_zero_length_range_convention() {
    let d = diff("", "x\ny");
    // "" splits to one empty line, which is replaced.
    assert_eq!((d.added, d.removed), (2, 1));
    let d = diff("a", "a\nb");
    assert!(d.unified.contains("@@ -1 +1,2 @@"), "{}", d.unified);
}

#[test]
fn output_is_bounded_but_counts_cover_the_whole_change() {
    let old: Vec<String> = (0..1000).map(|i| format!("old {i}")).collect();
    let new: Vec<String> = (0..1000).map(|i| format!("new {i}")).collect();
    let d = unified_diff(&old.join("\n"), &new.join("\n"), "s", "f", 50).unwrap();
    assert_eq!((d.added, d.removed), (1000, 1000));
    assert!(d.truncated);
    assert_eq!(d.output_lines, 50);
    assert_eq!(d.unified.lines().count(), 50);
}

#[test]
fn very_long_lines_are_clipped() {
    let long = "x".repeat(5_000);
    let d = diff("a", &long);
    let line = d.unified.lines().find(|l| l.starts_with("+x")).unwrap();
    assert_eq!(line.chars().count(), MAX_DIFF_LINE_CHARS + 2);
    assert!(line.ends_with('…'));
}

#[test]
fn huge_differing_blocks_fall_back_to_a_block_replacement() {
    let old: Vec<String> = (0..3000).map(|i| format!("o{i}")).collect();
    let new: Vec<String> = (0..3000).map(|i| format!("n{i}")).collect();
    // 3000 x 3000 exceeds the exact-diff table; the result must still be correct counts.
    let d = unified_diff(&old.join("\n"), &new.join("\n"), "s", "f", 10).unwrap();
    assert_eq!((d.added, d.removed, d.hunks), (3000, 3000, 1));
}

#[test]
fn the_diff_is_minimal_for_interleaved_edits() {
    let d = diff("a\nb\nc\nd\ne", "a\nc\nd\nx\ne");
    assert_eq!((d.added, d.removed), (1, 1));
    assert!(d.unified.contains("\n-b\n"));
    assert!(d.unified.contains("\n+x\n"));
}
