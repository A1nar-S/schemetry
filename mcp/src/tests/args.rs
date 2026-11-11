use std::ffi::OsString;

use super::{Command, parse_args};

fn parse(args: &[&str]) -> Result<Command, String> {
    parse_args(args.iter().map(OsString::from))
}

#[test]
fn no_arguments_serves_with_default_database() {
    assert_eq!(parse(&[]), Ok(Command::Serve { db_override: None }));
}

#[test]
fn db_flag_accepts_separate_and_inline_values() {
    let expected = Ok(Command::Serve {
        db_override: Some(OsString::from("a.db")),
    });
    assert_eq!(parse(&["--db", "a.db"]), expected);
    assert_eq!(parse(&["--db=a.db"]), expected);
}

#[test]
fn db_flag_requires_a_value() {
    assert!(parse(&["--db"]).is_err());
    assert!(parse(&["--db", ""]).is_err());
    assert!(parse(&["--db="]).is_err());
}

#[test]
fn db_flag_may_only_be_given_once() {
    assert!(parse(&["--db", "a.db", "--db=b.db"]).is_err());
}

#[test]
fn unknown_arguments_are_rejected() {
    assert!(parse(&["--bogus"]).is_err());
    assert!(parse(&["positional"]).is_err());
}

#[test]
fn help_and_version_short_circuit() {
    assert_eq!(parse(&["--help"]), Ok(Command::Help));
    assert_eq!(parse(&["-h"]), Ok(Command::Help));
    assert_eq!(parse(&["--version"]), Ok(Command::Version));
    assert_eq!(parse(&["-V"]), Ok(Command::Version));
}

#[test]
fn tool_arguments_default_to_auto_output_and_parse_the_result_tools() {
    use schemetry_core::services::mcp_gateway::{
        GetObjectDdlArgs, GrepResultArgs, OutputMode, ReadResultArgs,
    };
    use serde_json::json;

    let ddl: GetObjectDdlArgs = serde_json::from_value(
        json!({ "connection_id": 1, "object_name": "X", "object_type": "VIEW" }),
    )
    .unwrap();
    assert_eq!(ddl.output, OutputMode::Auto);
    let ddl: GetObjectDdlArgs = serde_json::from_value(
        json!({ "connection_id": 1, "object_name": "X", "object_type": "VIEW", "output": "file" }),
    )
    .unwrap();
    assert_eq!(ddl.output, OutputMode::File);
    assert!(
        serde_json::from_value::<GetObjectDdlArgs>(
            json!({ "connection_id": 1, "object_name": "X", "object_type": "VIEW", "output": "disk" })
        )
        .is_err()
    );

    let grep: GrepResultArgs =
        serde_json::from_value(json!({ "id": "r_1", "pattern": "ORDERS" })).unwrap();
    assert!(!grep.regex);
    assert_eq!(grep.ignore_case, None);
    let read: ReadResultArgs = serde_json::from_value(json!({ "id": "r_1" })).unwrap();
    assert_eq!((read.start_line, read.max_lines), (None, None));
}
