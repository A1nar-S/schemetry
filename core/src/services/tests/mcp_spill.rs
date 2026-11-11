use std::time::Duration;

use super::*;

fn meta() -> SpillMeta<'static> {
    SpillMeta {
        connection_ids: vec![1],
        tool: McpTool::GetObjectDdl,
        format: SpillFormat::Sql,
        label: Some("PKG_ORDERS"),
        target: None,
    }
}

fn store(root: &tempfile::TempDir) -> SpillStore {
    SpillStore::under(root.path().join("results"))
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn output_mode_decides_by_size_unless_forced() {
    assert!(!OutputMode::Auto.spills(INLINE_LIMIT_BYTES));
    assert!(OutputMode::Auto.spills(INLINE_LIMIT_BYTES + 1));
    assert!(!OutputMode::Inline.spills(usize::MAX));
    assert!(OutputMode::File.spills(0));
    assert_eq!(
        serde_json::from_str::<OutputMode>("\"file\"").unwrap(),
        OutputMode::File
    );
    assert_eq!(OutputMode::default(), OutputMode::Auto);
}

#[test]
fn write_then_get_round_trips_with_size_lines_and_digest() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root);
    let text = "CREATE PACKAGE BODY x AS\nEND;\n/";
    let entry = store.write(meta(), text.as_bytes()).unwrap();

    assert!(entry.id.starts_with("r_"));
    assert_eq!(std::fs::read_to_string(&entry.path).unwrap(), text);
    assert_eq!(entry.bytes, text.len() as u64);
    assert_eq!(entry.lines, 3);
    assert_eq!(entry.sha256, sha256_hex(text.as_bytes()));
    let name = entry
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.ends_with("-get_object_ddl-PKG_ORDERS.sql"), "{name}");
    assert!(entry.path.starts_with(store.dir()));

    let found = store.get(&entry.id).unwrap();
    assert_eq!(found.path, entry.path);
    assert!(store.get("r_nope").is_none());
    // No temp file is left next to it.
    assert_eq!(std::fs::read_dir(store.dir()).unwrap().count(), 1);
}

#[test]
fn labels_cannot_leave_the_folder() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root);
    let entry = store
        .write(
            SpillMeta {
                label: Some("../../etc/passwd"),
                ..meta()
            },
            b"x",
        )
        .unwrap();
    assert_eq!(entry.path.parent().unwrap(), store.dir());
}

#[test]
fn expired_results_are_gone() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root).with_limits(Duration::ZERO, 10, 1 << 20);
    let entry = store.write(meta(), b"x").unwrap();
    assert!(store.get(&entry.id).is_none());
    assert!(!entry.path.exists());
}

#[test]
fn the_oldest_files_make_room_for_new_ones() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root).with_limits(TTL, 2, 1 << 20);
    let first = store.write(meta(), b"1").unwrap();
    let second = store.write(meta(), b"2").unwrap();
    let third = store.write(meta(), b"3").unwrap();
    assert!(store.get(&first.id).is_none());
    assert!(!first.path.exists());
    assert!(store.get(&second.id).is_some());
    assert!(store.get(&third.id).is_some());

    // The byte budget works the same way.
    let store = SpillStore::under(root.path().join("bytes")).with_limits(TTL, 100, 10);
    let a = store.write(meta(), b"123456").unwrap();
    let b = store.write(meta(), b"123456").unwrap();
    assert!(store.get(&a.id).is_none());
    assert!(store.get(&b.id).is_some());
}

#[test]
fn oversized_results_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root);
    let big = vec![b'x'; MAX_FILE_BYTES + 1];
    assert!(store.write(meta(), &big).is_err());
}

#[test]
fn dropping_the_store_deletes_its_folder() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root);
    store.write(meta(), b"x").unwrap();
    let dir = store.dir().to_path_buf();
    assert!(dir.is_dir());
    drop(store);
    assert!(!dir.exists());
}

#[test]
fn old_folders_of_other_processes_are_swept_but_ours_is_kept() {
    let root = tempfile::tempdir().unwrap();
    let ours = root.path().join("ours");
    let theirs = root.path().join("theirs");
    std::fs::create_dir_all(&ours).unwrap();
    std::fs::create_dir_all(&theirs).unwrap();
    std::fs::write(theirs.join("r_1.sql"), "x").unwrap();
    std::fs::write(root.path().join("a-file"), "x").unwrap();

    sweep_stale_dirs(root.path(), &ours, Duration::from_secs(3600));
    assert!(theirs.exists(), "a recent folder is kept");

    sweep_stale_dirs(root.path(), &ours, Duration::ZERO);
    assert!(!theirs.exists());
    assert!(ours.exists());
    assert!(
        root.path().join("a-file").exists(),
        "only folders are swept"
    );
}

#[test]
fn preview_keeps_the_first_lines() {
    let text = numbered(100);
    let preview = preview(&text);
    assert!(preview.starts_with("line 1\n"));
    assert!(preview.contains("line 40\n"));
    assert!(!preview.contains("line 41\n"));
    // One very long line is clipped rather than dropped, and the 4 KB cap holds.
    let long = super::preview(&"y\n".repeat(5_000));
    assert!(long.len() <= 4 * 1024);
}

#[test]
fn outline_finds_statements_and_subprograms() {
    let oracle = "\
CREATE OR REPLACE EDITIONABLE PACKAGE BODY \"APP\".\"ORDER_API\" AS
  -- helper
  PROCEDURE create_order(p_id NUMBER) IS
  BEGIN NULL; END;

  FUNCTION total(p_id NUMBER) RETURN NUMBER IS
  BEGIN RETURN 0; END;
END;
/
";
    let (items, cut) = outline_sql(oracle);
    assert!(!cut);
    assert_eq!(
        items,
        vec![
            OutlineItem {
                line: 1,
                kind: "PACKAGE BODY".into(),
                name: "\"APP\".\"ORDER_API\"".into()
            },
            OutlineItem {
                line: 3,
                kind: "PROCEDURE".into(),
                name: "create_order".into()
            },
            OutlineItem {
                line: 6,
                kind: "FUNCTION".into(),
                name: "total".into()
            },
        ]
    );

    let postgres = "CREATE TABLE IF NOT EXISTS sales.orders (id int);\n\
                    ALTER TABLE sales.orders ADD COLUMN x int;\n\
                    create or replace function sales.f() returns int as $$ select 1 $$ language sql;\n";
    let (items, _) = outline_sql(postgres);
    let kinds: Vec<(&str, &str)> = items
        .iter()
        .map(|i| (i.kind.as_str(), i.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("TABLE", "sales.orders"),
            ("ALTER TABLE", "sales.orders"),
            ("FUNCTION", "sales.f")
        ]
    );

    let script = "-- Reference server: A\n-- Target data source: B\n\n-- discrepancy #1\n-- table: ORDERS\n-- column: X\n";
    let (items, _) = outline_sql(script);
    let kinds: Vec<(usize, &str, &str)> = items
        .iter()
        .map(|i| (i.line, i.kind.as_str(), i.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![(2, "TARGET DATA SOURCE", "B"), (5, "TABLE", "ORDERS")]
    );
}

#[test]
fn outline_is_capped() {
    let text: String = (0..600)
        .map(|i| format!("CREATE VIEW v{i} AS SELECT 1;\n"))
        .collect();
    let (items, cut) = outline_sql(&text);
    assert!(cut);
    assert_eq!(items.len(), 500);
}

fn file_with(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.sql");
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

#[test]
fn read_lines_returns_a_numbered_range() {
    let (_dir, path) = file_with(&numbered(10).replace('\n', "\r\n"));
    let out = read_lines(&path, 3, 2, 10).unwrap();
    assert_eq!(out.text, "3: line 3\n4: line 4\n");
    assert_eq!((out.start_line, out.end_line, out.returned), (3, 4, 2));
    assert!(out.more);

    let out = read_lines(&path, 9, 50, 10).unwrap();
    assert_eq!(out.text, "9: line 9\n10: line 10\n");
    assert!(!out.more);

    let out = read_lines(&path, 20, 5, 10).unwrap();
    assert_eq!((out.returned, out.end_line), (0, 19));
    assert!(out.text.is_empty());
}

#[test]
fn read_lines_clips_long_lines_and_stops_at_the_response_cap() {
    let long = "z".repeat(5_000);
    let text: String = (0..100).map(|_| format!("{long}\n")).collect();
    let (_dir, path) = file_with(&text);
    let out = read_lines(&path, 1, 2_000, 100).unwrap();
    assert!(
        out.text
            .lines()
            .all(|l| l.chars().count() <= MAX_LINE_CHARS + 10)
    );
    assert!(out.text.len() <= MAX_RESPONSE_BYTES);
    assert!(out.returned < 100);
    assert!(out.more);
}

#[test]
fn grep_is_literal_and_case_insensitive_by_default() {
    let (_dir, path) = file_with("select a.b from t\nSELECT X FROM T\nnothing\n");
    let out = grep(&path, &build_matcher("a.b", false, true).unwrap(), 0, 50).unwrap();
    assert_eq!(out.hunks, "1:select a.b from t\n");
    assert_eq!(out.match_count, 1);

    let out = grep(&path, &build_matcher("select", false, true).unwrap(), 0, 50).unwrap();
    assert_eq!(out.match_count, 2);
    let out = grep(
        &path,
        &build_matcher("select", false, false).unwrap(),
        0,
        50,
    )
    .unwrap();
    assert_eq!(out.match_count, 1);

    let out = grep(
        &path,
        &build_matcher(r"^SELECT \w+", true, false).unwrap(),
        0,
        50,
    )
    .unwrap();
    assert_eq!(out.hunks, "2:SELECT X FROM T\n");
}

#[test]
fn grep_merges_context_and_separates_groups() {
    let (_dir, path) = file_with(&numbered(20));
    let matcher = build_matcher(r"^line (3|5|15)$", true, true).unwrap();
    let out = grep(&path, &matcher, 1, 50).unwrap();
    assert_eq!(
        out.hunks,
        "2-line 2\n3:line 3\n4-line 4\n5:line 5\n6-line 6\n--\n14-line 14\n15:line 15\n16-line 16\n"
    );
    assert_eq!((out.match_count, out.returned), (3, 3));
    assert!(!out.truncated);
}

#[test]
fn grep_stops_returning_at_max_matches_but_counts_everything() {
    let (_dir, path) = file_with(&numbered(100));
    let out = grep(&path, &build_matcher("line", false, true).unwrap(), 0, 5).unwrap();
    assert_eq!(out.returned, 5);
    assert_eq!(out.match_count, 100);
    assert!(out.truncated);
    assert_eq!(out.hunks.lines().count(), 5);
}

#[test]
fn grep_matches_non_ascii_text() {
    let (_dir, path) = file_with("-- Größe\nother\n");
    let out = grep(&path, &build_matcher("größe", false, true).unwrap(), 0, 5).unwrap();
    // ASCII-only case folding: the lower-case 'g' still matches 'G'.
    assert_eq!(out.hunks, "1:-- Größe\n");
}

#[test]
fn bad_patterns_are_refused() {
    assert!(build_matcher("", false, true).is_err());
    assert!(build_matcher("(unclosed", true, true).is_err());
    assert!(build_matcher(&"a".repeat(1_001), false, true).is_err());
    assert!(
        build_matcher("(unclosed", false, true).is_ok(),
        "literal mode escapes it"
    );
}

#[test]
fn sniff_names_common_lob_formats() {
    let ext = |bytes: &[u8]| sniff(bytes).ext;
    assert_eq!(ext(b"%PDF-1.7\n..."), "pdf");
    assert_eq!(ext(b"\x89PNG\r\n\x1a\n\0\0"), "png");
    assert_eq!(ext(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
    assert_eq!(ext(b"GIF89a..."), "gif");
    assert_eq!(ext(b"RIFF\0\0\0\0WEBPVP8 "), "webp");
    assert_eq!(ext(b"PK\x03\x04....word/document.xml"), "docx");
    assert_eq!(ext(b"PK\x03\x04....xl/workbook.xml"), "xlsx");
    assert_eq!(ext(b"PK\x03\x04....other"), "zip");
    assert_eq!(ext(&[0x1F, 0x8B, 0x08]), "gz");
    assert_eq!(ext(b"<?xml version=\"1.0\"?><svg xmlns=\"x\"/>"), "svg");
    assert_eq!(ext(b"  <!DOCTYPE html><html></html>"), "html");
    assert_eq!(ext(b"<?xml version=\"1.0\"?><order/>"), "xml");
    assert_eq!(ext(b"{\"a\": [1, 2]}"), "json");
    assert_eq!(ext(b"{not json"), "txt");
    assert_eq!(ext(b"plain words"), "txt");
    assert_eq!(ext(&[0x00, 0x9F, 0x92, 0x96]), "bin");
    assert_eq!(ext(b"text\0with nul"), "bin");

    assert!(sniff(b"%PDF-").binary);
    assert!(!sniff(b"{}").binary);
    assert_eq!(sniff(b"%PDF-").mime, "application/pdf");
}

#[test]
fn lob_files_carry_their_type_and_binary_flag() {
    let root = tempfile::tempdir().unwrap();
    let store = store(&root);
    let entry = store
        .write(
            SpillMeta {
                format: SpillFormat::Lob(sniff(b"%PDF-1.4")),
                label: Some("DOCS-CONTENT-row1"),
                ..meta()
            },
            b"%PDF-1.4",
        )
        .unwrap();
    assert!(
        entry
            .path
            .to_string_lossy()
            .ends_with("-DOCS-CONTENT-row1.pdf")
    );
    assert!(entry.format.binary());
    let handle = entry.handle(TTL);
    assert_eq!(handle["mime"], "application/pdf");
    assert_eq!(handle["binary"], true);
    assert_eq!(handle["format"], "pdf");
}
