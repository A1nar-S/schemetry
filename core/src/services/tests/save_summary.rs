use std::path::PathBuf;

use super::*;
use crate::services::ddl_save::{decode_content, encoding_label};

fn planned(path: PathBuf, content: &[u8], exists: bool, encoding: &'static str) -> PlannedFile {
    PlannedFile {
        kind: "code",
        path,
        content: content.to_vec(),
        exists,
        encoding,
    }
}

#[test]
fn line_endings_are_classified() {
    assert_eq!(line_ending_style(b"a\r\nb\r\n"), "CRLF");
    assert_eq!(line_ending_style(b"a\nb\n"), "LF");
    assert_eq!(line_ending_style(b"a\r\nb\n"), "mixed");
    assert_eq!(line_ending_style(b"single line"), "none");
    assert_eq!(line_ending_style(b""), "none");
}

#[test]
fn lines_are_counted_with_or_without_a_final_break() {
    assert_eq!(line_count(b""), 0);
    assert_eq!(line_count(b"a"), 1);
    assert_eq!(line_count(b"a\n"), 1);
    assert_eq!(line_count(b"a\r\nb"), 2);
    assert_eq!(line_count(b"\n\n"), 2);
}

#[test]
fn line_diff_counts_added_removed_and_ignores_line_ending_style() {
    assert_eq!(line_diff_counts("a\nb\nc\n", "a\nb\nc\n"), (0, 0));
    assert_eq!(line_diff_counts("a\nb\nc\n", "a\nx\nc\nd\n"), (2, 1));
    assert_eq!(line_diff_counts("", "a\nb\n"), (2, 0));
    assert_eq!(line_diff_counts("a\nb\n", ""), (0, 2));
    assert_eq!(line_diff_counts("a\r\nb\r\n", "a\nb\n"), (0, 0));
}

#[test]
fn new_files_are_described_without_a_diff_or_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = planned(dir.path().join("a.sql"), b"one\r\ntwo\r\n", false, "utf8");
    let entry = describe_planned_file(&file, false);

    assert_eq!(entry["kind"], "code");
    assert_eq!(entry["overwrites_existing"], false);
    assert_eq!(entry["size"], 10);
    assert_eq!(entry["lines"], 2);
    assert_eq!(entry["encoding"], "utf8");
    assert_eq!(entry["line_ending"], "CRLF");
    assert_eq!(entry["sha256"].as_str().unwrap().len(), 64);
    assert!(entry.get("diff").is_none());
    assert!(entry.get("content").is_none());
}

#[test]
fn existing_files_get_a_line_diff_against_the_disk_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sql");
    std::fs::write(&path, "a\nb\nc\n").unwrap();

    let entry = describe_planned_file(&planned(path.clone(), b"a\nB\nc\nd\n", true, "utf8"), false);
    let diff = &entry["diff"];
    assert_eq!(diff["added"], 2);
    assert_eq!(diff["removed"], 1);
    assert_eq!(diff["identical"], false);
    assert_eq!(diff["existing_size"], 6);
    assert_eq!(diff["existing_line_ending"], "LF");

    let same = describe_planned_file(&planned(path, b"a\nb\nc\n", true, "utf8"), false);
    assert_eq!(same["diff"]["added"], 0);
    assert_eq!(same["diff"]["removed"], 0);
    assert_eq!(same["diff"]["identical"], true);
}

#[test]
fn the_diff_decodes_the_existing_file_with_the_planned_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sql");
    // "Žvaigždė" in Windows-1257.
    let old = [0xDE, b'v', b'a', b'i', b'g', 0xBE, b'd', 0xEB, b'\n'];
    std::fs::write(&path, old).unwrap();

    let entry = describe_planned_file(&planned(path, &old, true, "windows-1257"), false);
    assert_eq!(entry["diff"]["added"], 0);
    assert_eq!(entry["diff"]["removed"], 0);
    assert_eq!(entry["diff"]["existing_not_valid_in_encoding"], false);
    assert_eq!(entry["encoding"], "windows-1257");
}

#[test]
fn existing_bytes_invalid_in_the_encoding_are_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sql");
    std::fs::write(&path, [0xFF, 0xFE, b'\n']).unwrap();

    let entry = describe_planned_file(&planned(path, b"x\n", true, "utf8"), false);
    assert_eq!(entry["diff"]["existing_not_valid_in_encoding"], true);
}

#[test]
fn an_unreadable_existing_path_degrades_to_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.sql");
    let entry = describe_planned_file(&planned(missing, b"x\n", true, "utf8"), false);
    assert!(entry["diff"]["unavailable"].as_str().is_some());
}

#[test]
fn content_is_only_returned_on_request_and_below_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let small = planned(
        dir.path().join("a.sql"),
        "žalias\n".as_bytes(),
        false,
        "utf8",
    );
    assert_eq!(describe_planned_file(&small, true)["content"], "žalias\n");

    let big = planned(
        dir.path().join("b.sql"),
        &vec![b'x'; MAX_INCLUDED_CONTENT_BYTES + 1],
        false,
        "utf8",
    );
    let entry = describe_planned_file(&big, true);
    assert!(entry.get("content").is_none());
    assert!(entry["content_omitted"].as_str().is_some());
}

#[test]
fn decoding_round_trips_the_supported_encodings() {
    let bom = [&[0xEF, 0xBB, 0xBF][..], b"abc"].concat();
    assert_eq!(decode_content(&bom, "utf8-bom"), ("abc".to_string(), false));
    assert_eq!(decode_content(b"abc", "UTF8"), ("abc".to_string(), false));
    assert_eq!(
        decode_content(&[0xDE], "windows-1257"),
        ("Ž".to_string(), false)
    );
    assert_eq!(encoding_label(" Windows-1257 "), "windows-1257");
    assert_eq!(encoding_label("whatever"), "utf8");
}
