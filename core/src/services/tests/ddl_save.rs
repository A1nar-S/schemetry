use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::models::SchemaFolderOverride;
use crate::repositories::folder_schema_override_repository::SchemaFolderOverrideRepository;
use crate::services::ddl_save::{
    DdlSaveArgs, DdlSaveOptions, FLYWAY_TEMPLATE, LineEnding, NameValues, PACKAGE_SPEC_AND_BODY,
    TIMESTAMP_TEMPLATE, apply_line_endings, combine_package_spec_and_body, encode_content,
    has_package_spec_and_body, is_package_spec_and_body, next_migration_version, plan_ddl_save,
    plan_ddl_save_raw, render_file_name, resolve_schema_dir,
    save_ddl_to_folder as save_ddl_service, settings_object_type, validate_naming_template,
};
use crate::services::folder_schema_overrides::SchemaFolderOverrideService;

fn args() -> DdlSaveArgs {
    DdlSaveArgs {
        schema: "APP".to_string(),
        object_name: "ORDERS".to_string(),
        object_type: "TABLE".to_string(),
        ddl: "CREATE TABLE ORDERS (ID INTEGER);".to_string(),
        description: "create_orders".to_string(),
        db_type: "oracle".to_string(),
    }
}

fn options(storage_mode: &str) -> DdlSaveOptions {
    DdlSaveOptions {
        code_folder: "code".to_string(),
        migration_folder: "migration".to_string(),
        storage_mode: storage_mode.to_string(),
        naming_convention: "timestamp".to_string(),
        folder_mode: "year".to_string(),
        version_label: String::new(),
        extension: "tab".to_string(),
        encoding: "utf8".to_string(),
        line_endings: String::new(),
        naming_template: String::new(),
    }
}

fn save(
    schema_root: &Path,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
) -> Result<crate::services::ddl_save::SaveDdlResult, String> {
    let folder_overrides =
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository { override_row: None }));
    save_ddl_service(
        &folder_overrides,
        &schema_root.to_string_lossy(),
        args,
        options,
    )
}

#[test]
fn storage_modes_write_the_expected_files() {
    for (mode, writes_code, writes_migration) in [
        ("code", true, false),
        ("migration", false, true),
        ("both", true, true),
        ("invalid", true, true),
    ] {
        let temp_dir = tempfile::tempdir().unwrap();
        let result = save(temp_dir.path(), &args(), &options(mode)).unwrap();

        assert_eq!(result.code_path.is_some(), writes_code, "mode {mode}");
        assert_eq!(
            result.migration_path.is_some(),
            writes_migration,
            "mode {mode}"
        );
    }
}

#[test]
fn raw_plans_keep_the_bytes_untouched_in_code_and_migration_files() {
    let folder_overrides =
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository { override_row: None }));
    let mut options = options("both");
    options.encoding = "windows-1257".to_string();
    let raw: &[u8] = b"\xEF\xBB\xBFCREATE TABLE ORDERS (ID INTEGER);\r\n\xE0";
    let temp_dir = tempfile::tempdir().unwrap();

    let plan = plan_ddl_save_raw(
        &folder_overrides,
        &temp_dir.path().to_string_lossy(),
        &args(),
        &options,
        raw,
    )
    .unwrap();

    assert_eq!(plan.files.len(), 2);
    for file in &plan.files {
        assert_eq!(file.content, raw, "{}", file.kind);
    }
    assert!(
        !temp_dir.path().join("app").exists(),
        "planning must not write"
    );
}

#[test]
fn flyway_naming_uses_repeatable_code_and_numbered_migration_names() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("both");
    save_options.naming_convention = "flyway".to_string();

    let result = save(temp_dir.path(), &args(), &save_options).unwrap();

    assert!(result.code_path.unwrap().ends_with("R__ORDERS.tab"));
    assert!(
        result
            .migration_path
            .unwrap()
            .ends_with("V1__app_create_orders.tab")
    );
}

#[test]
fn timestamp_migration_names_include_schema_and_description() {
    let temp_dir = tempfile::tempdir().unwrap();
    let schema_dir = temp_dir.path().join("app");
    let result = save(temp_dir.path(), &args(), &options("migration")).unwrap();
    let filename = Path::new(result.migration_path.as_deref().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy();

    assert!(
        regex::Regex::new(r"^\d{6}_\d{4}_app\$create_orders\.tab$")
            .unwrap()
            .is_match(&filename)
    );
}

#[test]
fn migration_folder_selection_prefers_primary_then_new_then_base() {
    let temp_dir = tempfile::tempdir().unwrap();
    let schema_dir = temp_dir.path().join("app");
    let migration_dir = schema_dir.join("migration");
    let year_dir = migration_dir.join(chrono::Local::now().format("%Y").to_string());
    let new_dir = migration_dir.join("new");
    fs::create_dir_all(&year_dir).unwrap();
    fs::create_dir_all(&new_dir).unwrap();

    let first = save(temp_dir.path(), &args(), &options("migration")).unwrap();
    assert_eq!(
        Path::new(first.migration_path.as_deref().unwrap()).parent(),
        Some(year_dir.as_path())
    );

    fs::remove_dir_all(&year_dir).unwrap();
    let second = save(temp_dir.path(), &args(), &options("migration")).unwrap();
    assert_eq!(
        Path::new(second.migration_path.as_deref().unwrap()).parent(),
        Some(new_dir.as_path())
    );

    fs::remove_dir_all(&new_dir).unwrap();
    let third = save(temp_dir.path(), &args(), &options("migration")).unwrap();
    assert_eq!(
        Path::new(third.migration_path.as_deref().unwrap()).parent(),
        Some(migration_dir.as_path())
    );
}

#[test]
fn version_folder_mode_uses_existing_version_folder() {
    let temp_dir = tempfile::tempdir().unwrap();
    let schema_dir = temp_dir.path().join("app");
    let version_dir = schema_dir.join("migration/release-1");
    fs::create_dir_all(&version_dir).unwrap();
    let mut save_options = options("migration");
    save_options.folder_mode = "version".to_string();
    save_options.version_label = "release-1".to_string();

    let result = save(temp_dir.path(), &args(), &save_options).unwrap();

    assert_eq!(
        Path::new(result.migration_path.as_deref().unwrap()).parent(),
        Some(version_dir.as_path())
    );
}

#[test]
fn version_scan_checks_direct_and_one_level_subdirectories() {
    let temp_dir = tempfile::tempdir().unwrap();
    let migration_dir = temp_dir.path();
    let nested_dir = migration_dir.join("new");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(migration_dir.join("V3__direct.sql"), "").unwrap();
    fs::write(nested_dir.join("V8__nested.sql"), "").unwrap();
    fs::write(nested_dir.join("V99_invalid.sql"), "").unwrap();
    fs::create_dir_all(nested_dir.join("deeper")).unwrap();
    fs::write(nested_dir.join("deeper/V20__ignored.sql"), "").unwrap();

    assert_eq!(next_migration_version(migration_dir), 9);
}

#[test]
fn encoding_supports_utf8_bom_windows_1257_and_unknown_fallback() {
    assert_eq!(encode_content("abc", "utf8").unwrap(), b"abc");
    assert_eq!(
        encode_content("abc", "utf8-bom").unwrap(),
        b"\xEF\xBB\xBFabc"
    );
    assert_eq!(
        encode_content("Ā", "windows-1257").unwrap(),
        encoding_rs::WINDOWS_1257.encode("Ā").0.into_owned()
    );
    assert_eq!(encode_content("abc", "unknown").unwrap(), b"abc");
}

#[test]
fn unrepresentable_characters_refuse_the_save_and_name_the_offenders() {
    let error = encode_content("a\nbЖc€d", "windows-1257").unwrap_err();

    assert!(error.contains("'Ж' (U+0416)"), "{error}");
    assert!(error.contains("line 2"), "{error}");
    assert!(error.contains("windows-1257"), "{error}");
    assert!(!error.contains("&#"), "{error}");
    // Baltic letters do exist in Windows-1257.
    assert!(encode_content("ĀāĒē", "windows-1257").is_ok());
}

#[test]
fn saving_lossy_content_writes_nothing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("both");
    save_options.encoding = "windows-1257".to_string();
    let mut save_args = args();
    save_args.ddl = "-- привет".to_string();

    let error = save(temp_dir.path(), &save_args, &save_options).unwrap_err();

    assert!(error.contains("U+043F"), "{error}");
    assert_eq!(fs::read_dir(temp_dir.path()).unwrap().count(), 0);
}

#[test]
fn line_endings_are_normalised_before_encoding() {
    assert_eq!(
        apply_line_endings("a\r\nb\rc\nd", LineEnding::Keep),
        "a\r\nb\rc\nd"
    );
    assert_eq!(
        apply_line_endings("a\r\nb\rc\nd", LineEnding::Lf),
        "a\nb\nc\nd"
    );
    assert_eq!(
        apply_line_endings("a\r\nb\rc\nd", LineEnding::Crlf),
        "a\r\nb\r\nc\r\nd"
    );
    assert!(LineEnding::parse("bogus").is_err());
    assert_eq!(LineEnding::parse("").unwrap(), LineEnding::Keep);
    assert_eq!(LineEnding::parse("CRLF").unwrap(), LineEnding::Crlf);
}

#[test]
fn saved_files_use_the_configured_line_endings_and_report_them() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("both");
    save_options.line_endings = "crlf".to_string();
    save_options.encoding = "utf8-bom".to_string();
    let mut save_args = args();
    save_args.ddl = "CREATE TABLE ORDERS (\n  ID INTEGER\n);".to_string();

    let folder_overrides =
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository { override_row: None }));
    let plan = plan_ddl_save(
        &folder_overrides,
        &temp_dir.path().to_string_lossy(),
        &save_args,
        &save_options,
    )
    .unwrap();
    let result = plan.result();

    assert_eq!(result.encoding, "utf8-bom");
    assert_eq!(result.line_endings, "crlf");
    assert_eq!(result.naming_template, TIMESTAMP_TEMPLATE);
    for file in &plan.files {
        let text = String::from_utf8(file.content[3..].to_vec()).unwrap();
        assert!(text.contains("\r\n"), "{}", file.kind);
        assert!(!text.replace("\r\n", "").contains('\n'), "{}", file.kind);
    }
}

#[test]
fn folder_override_wins_for_line_endings_and_unknown_encoding_reports_utf8() {
    let temp_dir = tempfile::tempdir().unwrap();
    let overrides = SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository {
        override_row: Some(SchemaFolderOverride {
            id: 1,
            schema_name: "APP".to_string(),
            line_endings: "lf".to_string(),
            ..empty_override()
        }),
    }));
    let mut save_options = options("code");
    save_options.line_endings = "crlf".to_string();
    save_options.encoding = "whatever".to_string();

    let plan = plan_ddl_save(
        &overrides,
        &temp_dir.path().to_string_lossy(),
        &args(),
        &save_options,
    )
    .unwrap();

    assert_eq!(plan.result().line_endings, "lf");
    assert_eq!(plan.result().encoding, "utf8");

    save_options.line_endings = "sideways".to_string();
    let service =
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository { override_row: None }));
    assert!(
        plan_ddl_save(
            &service,
            &temp_dir.path().to_string_lossy(),
            &args(),
            &save_options
        )
        .is_err()
    );
}

fn values<'a>(description: &'a str) -> NameValues<'a> {
    NameValues {
        date: "260415",
        time: "1200",
        schema: "KOPLIET",
        object: "LIETOTAJS_TIESIBAS",
        object_type: "PACKAGE BODY",
        description,
        version: 7,
        extension: "pck",
    }
}

#[test]
fn templates_render_every_placeholder() {
    assert_eq!(
        render_file_name("{date}_{time}_{schema}${object}{ext}", &values("")).unwrap(),
        "260415_1200_kopliet$LIETOTAJS_TIESIBAS.pck"
    );
    assert_eq!(
        render_file_name("V{version}__{type}_{description}{ext}", &values("fix-1")).unwrap(),
        "V7__package_body_fix-1.pck"
    );
    assert_eq!(
        render_file_name("{object}#{description}.{ext}", &values("x")).unwrap(),
        "LIETOTAJS_TIESIBAS#x.pck"
    );
}

#[test]
fn presets_match_the_historical_names_and_drop_dangling_separators() {
    assert_eq!(
        render_file_name(TIMESTAMP_TEMPLATE, &values("create_orders")).unwrap(),
        "260415_1200_kopliet$create_orders.pck"
    );
    assert_eq!(
        render_file_name(TIMESTAMP_TEMPLATE, &values("")).unwrap(),
        "260415_1200_kopliet.pck"
    );
    assert_eq!(
        render_file_name(FLYWAY_TEMPLATE, &values("")).unwrap(),
        "V7__kopliet.pck"
    );
    assert_eq!(
        render_file_name("{description}_{schema}_{object}{ext}", &values("")).unwrap(),
        "kopliet_LIETOTAJS_TIESIBAS.pck"
    );
    assert_eq!(
        render_file_name("{date}_{description}_{description}{ext}", &values("")).unwrap(),
        "260415.pck"
    );
}

#[test]
fn invalid_templates_are_rejected() {
    for template in [
        "",
        "   ",
        "{unknown}_{object}",
        "{object",
        "object}",
        "{object}/{ext}",
        "{object}\\{ext}",
        "../{object}",
        "{object} x{ext}",
        "{object}*{ext}",
        &"a".repeat(121),
    ] {
        assert!(
            validate_naming_template(template).is_err(),
            "expected {template:?} to be rejected"
        );
    }
    assert!(validate_naming_template(TIMESTAMP_TEMPLATE).is_ok());
    assert!(validate_naming_template(FLYWAY_TEMPLATE).is_ok());
    assert!(validate_naming_template("{date}_{time}_{schema}${object}{ext}").is_ok());
}

#[test]
fn rendered_names_that_could_escape_or_are_unsafe_are_rejected() {
    for description in ["../x", "a/b", "a\\b", "x..y", "C:", "a*b", "a\0b"] {
        assert!(
            render_file_name("{schema}_{description}{ext}", &values(description)).is_err(),
            "expected description {description:?} to be rejected"
        );
    }
    assert!(render_file_name("{description}", &values("NUL")).is_err());
    assert!(render_file_name("{description}", &values("")).is_err());
    assert!(
        render_file_name(
            "{object}",
            &NameValues {
                object: "a/b",
                ..values("")
            }
        )
        .is_err()
    );
    assert!(render_file_name(&"{object}".repeat(15), &values("")).is_err());
}

#[test]
fn a_template_saves_with_an_optional_description() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("migration");
    save_options.naming_template = "{date}_{time}_{schema}${object}{ext}".to_string();
    let mut save_args = args();
    save_args.description = String::new();

    let result = save(temp_dir.path(), &save_args, &save_options).unwrap();

    let filename = Path::new(result.migration_path.as_deref().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy();
    assert!(
        regex::Regex::new(r"^\d{6}_\d{4}_app\$ORDERS\.tab$")
            .unwrap()
            .is_match(&filename),
        "{filename}"
    );
    assert_eq!(
        result.naming_template,
        "{date}_{time}_{schema}${object}{ext}"
    );
}

#[test]
fn an_invalid_template_or_description_refuses_the_save_without_writing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("both");
    save_options.naming_template = "{bogus}{ext}".to_string();
    assert!(save(temp_dir.path(), &args(), &save_options).is_err());

    let mut save_args = args();
    save_args.description = "../../escape".to_string();
    assert!(save(temp_dir.path(), &save_args, &options("both")).is_err());
    assert_eq!(fs::read_dir(temp_dir.path()).unwrap().count(), 0);
}

#[test]
fn template_precedence_is_folder_template_folder_preset_global_template_global_preset() {
    let service = |template: &str, convention: &str| {
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository {
            override_row: Some(SchemaFolderOverride {
                id: 1,
                schema_name: "APP".to_string(),
                naming_template: template.to_string(),
                naming_convention: convention.to_string(),
                ..empty_override()
            }),
        }))
    };

    let resolve = |svc: &SchemaFolderOverrideService| {
        svc.resolve_naming_template("app", "timestamp", "{object}{ext}")
            .unwrap()
    };
    assert_eq!(
        resolve(&service("{schema}{ext}", "flyway")),
        "{schema}{ext}"
    );
    assert_eq!(resolve(&service("", "flyway")), FLYWAY_TEMPLATE);
    assert_eq!(resolve(&service("", "")), "{object}{ext}");
    assert_eq!(
        service("", "")
            .resolve_naming_template("app", "flyway", "")
            .unwrap(),
        FLYWAY_TEMPLATE
    );
}

#[test]
fn saving_an_override_validates_template_and_line_endings() {
    let service =
        SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository { override_row: None }));
    let good = SchemaFolderOverride {
        schema_name: "app".to_string(),
        naming_template: "{date}_{object}{ext}".to_string(),
        line_endings: "crlf".to_string(),
        ..empty_override()
    };
    assert!(service.save_override(&good).is_ok());
    assert!(
        service
            .save_override(&SchemaFolderOverride {
                naming_template: "{nope}".to_string(),
                ..good.clone()
            })
            .is_err()
    );
    assert!(
        service
            .save_override(&SchemaFolderOverride {
                line_endings: "cr".to_string(),
                ..good
            })
            .is_err()
    );
}

fn empty_override() -> SchemaFolderOverride {
    SchemaFolderOverride {
        id: 0,
        schema_name: String::new(),
        folder_path: String::new(),
        encoding: String::new(),
        extensions: String::new(),
        storage_modes: String::new(),
        naming_convention: String::new(),
        code_folder_name: String::new(),
        migration_folder_name: String::new(),
        migration_folder_mode: String::new(),
        migration_version_label: String::new(),
        line_endings: String::new(),
        naming_template: String::new(),
    }
}

#[test]
fn write_errors_are_returned_to_the_caller() {
    let temp_dir = tempfile::tempdir().unwrap();
    let schema_dir = temp_dir.path().join("app");
    fs::create_dir_all(&schema_dir).unwrap();
    fs::write(schema_dir.join("not-a-folder"), "file").unwrap();
    let mut save_options = options("code");
    save_options.code_folder = "not-a-folder".to_string();

    let error = save(temp_dir.path(), &args(), &save_options).unwrap_err();

    assert!(error.contains("Failed to create code dir"));
}

#[test]
fn schema_directory_resolution_honors_override_without_creating_it() {
    let temp_dir = tempfile::tempdir().unwrap();
    let custom_dir = temp_dir.path().join("custom");
    let service = SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository {
        override_row: Some(SchemaFolderOverride {
            id: 1,
            schema_name: "APP".to_string(),
            folder_path: custom_dir.to_string_lossy().into_owned(),
            encoding: String::new(),
            extensions: String::new(),
            storage_modes: String::new(),
            naming_convention: String::new(),
            code_folder_name: String::new(),
            migration_folder_name: String::new(),
            migration_folder_mode: String::new(),
            migration_version_label: String::new(),
            line_endings: String::new(),
            naming_template: String::new(),
        }),
    }));

    let resolved = resolve_schema_dir(&service, "APP", "ignored-root").unwrap();

    assert_eq!(resolved, custom_dir);
    assert!(!resolved.exists());
}

#[test]
fn save_applies_per_schema_folder_storage_extension_and_encoding_overrides() {
    let temp_dir = tempfile::tempdir().unwrap();
    let custom_dir = temp_dir.path().join("custom");
    let overrides = SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository {
        override_row: Some(SchemaFolderOverride {
            id: 1,
            schema_name: "APP".to_string(),
            folder_path: custom_dir.to_string_lossy().into_owned(),
            encoding: "utf8-bom".to_string(),
            extensions: r#"{"TABLE":"sql"}"#.to_string(),
            storage_modes: r#"{"TABLE":"code"}"#.to_string(),
            naming_convention: "flyway".to_string(),
            code_folder_name: "source".to_string(),
            migration_folder_name: "migrations".to_string(),
            migration_folder_mode: String::new(),
            migration_version_label: String::new(),
            line_endings: String::new(),
            naming_template: String::new(),
        }),
    }));

    let result = save_ddl_service(
        &overrides,
        &temp_dir.path().to_string_lossy(),
        &args(),
        &options("migration"),
    )
    .unwrap();

    let code_path = Path::new(result.code_path.as_deref().unwrap());
    assert_eq!(code_path, custom_dir.join("source/app/R__ORDERS.sql"));
    assert!(result.migration_path.is_none());
    assert_eq!(fs::read(code_path).unwrap()[..3], [0xEF, 0xBB, 0xBF]);
}

const SPEC: &str = "CREATE OR REPLACE PACKAGE PKG AS\n  PROCEDURE p;\nEND PKG;";
const BODY: &str =
    "CREATE OR REPLACE PACKAGE BODY PKG AS\n  PROCEDURE p IS BEGIN NULL; END;\nEND PKG;";

fn package_args() -> DdlSaveArgs {
    DdlSaveArgs {
        object_name: "PKG".to_string(),
        object_type: PACKAGE_SPEC_AND_BODY.to_string(),
        ddl: combine_package_spec_and_body(SPEC, BODY),
        ..args()
    }
}

#[test]
fn combine_puts_a_slash_line_after_spec_and_body_without_doubling_terminators() {
    let expected = format!("{SPEC}\n/\n\n{BODY}\n/\n");
    assert_eq!(combine_package_spec_and_body(SPEC, BODY), expected);
    // DBMS_METADATA output already ends with a `/` line.
    let with_slashes =
        combine_package_spec_and_body(&format!("{SPEC}\n/\n"), &format!("{BODY}\n/"));
    assert_eq!(with_slashes, expected);
    // A `/` inside code (division) is not a terminator.
    let division = "BEGIN x := a /\n b; END;";
    assert!(combine_package_spec_and_body(SPEC, division).contains("a /\n b"));
}

#[test]
fn has_package_spec_and_body_needs_both_statements() {
    assert!(has_package_spec_and_body(&combine_package_spec_and_body(
        SPEC, BODY
    )));
    assert!(has_package_spec_and_body(
        "create package a is end;\n/\ncreate or replace editionable package body a is begin null; end;"
    ));
    assert!(!has_package_spec_and_body(SPEC));
    assert!(!has_package_spec_and_body(BODY));
    assert!(!has_package_spec_and_body(
        "CREATE TABLE PACKAGE_X (ID NUMBER);"
    ));
}

#[test]
fn combined_type_follows_the_package_settings() {
    assert!(is_package_spec_and_body(" package spec and body "));
    assert!(!is_package_spec_and_body("PACKAGE BODY"));
    assert_eq!(settings_object_type(PACKAGE_SPEC_AND_BODY), "PACKAGE");
    assert_eq!(settings_object_type("PACKAGE BODY"), "PACKAGE BODY");
    assert_eq!(settings_object_type("TABLE"), "TABLE");
    assert_eq!(
        crate::services::settings::default_ddl_extension("PACKAGE"),
        "pck"
    );
}

#[test]
fn combined_package_is_saved_as_one_file_with_the_package_extension() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut save_options = options("both");
    save_options.extension = "pck".to_string();

    let result = save(temp_dir.path(), &package_args(), &save_options).unwrap();

    let code_path = result.code_path.unwrap();
    assert!(code_path.ends_with("PKG.pck"), "{code_path}");
    assert_eq!(fs::read_to_string(&code_path).unwrap(), package_args().ddl);
    let migration = fs::read_to_string(result.migration_path.unwrap()).unwrap();
    assert_eq!(migration, package_args().ddl.trim());
}

#[test]
fn combined_package_uses_the_package_overrides_of_the_schema() {
    let temp_dir = tempfile::tempdir().unwrap();
    let overrides = SchemaFolderOverrideService::new(Arc::new(TestOverrideRepository {
        override_row: Some(SchemaFolderOverride {
            id: 1,
            schema_name: "APP".to_string(),
            folder_path: String::new(),
            encoding: String::new(),
            extensions: r#"{"PACKAGE":"pks"}"#.to_string(),
            storage_modes: r#"{"PACKAGE":"code"}"#.to_string(),
            naming_convention: String::new(),
            code_folder_name: String::new(),
            migration_folder_name: String::new(),
            migration_folder_mode: String::new(),
            migration_version_label: String::new(),
            line_endings: String::new(),
            naming_template: String::new(),
        }),
    }));

    let plan = plan_ddl_save(
        &overrides,
        &temp_dir.path().to_string_lossy(),
        &package_args(),
        &options("both"),
    )
    .unwrap();

    assert_eq!(plan.files.len(), 1);
    assert!(plan.files[0].path.ends_with("PKG.pks"));
}

#[test]
fn combined_package_is_rejected_for_postgres() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut pg_args = package_args();
    pg_args.db_type = "postgres".to_string();

    let error = save(temp_dir.path(), &pg_args, &options("both")).unwrap_err();

    assert!(
        error.contains("Oracle") && error.contains("PostgreSQL"),
        "{error}"
    );
    assert_eq!(fs::read_dir(temp_dir.path()).unwrap().count(), 0);
}

struct TestOverrideRepository {
    override_row: Option<SchemaFolderOverride>,
}

impl SchemaFolderOverrideRepository for TestOverrideRepository {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }

    fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>> {
        Ok(self.override_row.clone().into_iter().collect())
    }

    fn insert_override(&self, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        Ok(data.clone())
    }

    fn update_override(
        &self,
        _id: i64,
        data: &SchemaFolderOverride,
    ) -> Result<SchemaFolderOverride> {
        Ok(data.clone())
    }

    fn delete_override(&self, _id: i64) -> Result<()> {
        Ok(())
    }

    fn find_for_schema(&self, _schema_name: &str) -> Result<Option<SchemaFolderOverride>> {
        Ok(self.override_row.clone())
    }
}

#[test]
fn unified_diff_marks_changed_lines_with_context() {
    let old = "a\nb\nc\nd\ne\nf\ng\n";
    let new = "a\nb\nc\nD\ne\nf\ng\nh\n";
    let diff = crate::services::ddl_save::unified_diff_summary(old, new);
    assert!(diff.contains("(+2 -1 lines)"), "{diff}");
    assert!(diff.contains("-d\n+D"), "{diff}");
    assert!(diff.contains("+h"), "{diff}");
    assert!(diff.contains(" c\n"), "{diff}");
    assert!(!diff.contains(" a\n"), "{diff}");
}

#[test]
fn unified_diff_is_bounded() {
    let old: String = (0..5_000).map(|i| format!("old line {i}\n")).collect();
    let new: String = (0..5_000).map(|i| format!("new line {i}\n")).collect();
    let diff = crate::services::ddl_save::unified_diff_summary(&old, &new);
    assert!(diff.len() < 4_000, "{}", diff.len());
    assert!(diff.contains("too large"), "{diff}");

    let old: String = (0..200).map(|i| format!("old {i}\n")).collect();
    let new: String = (0..200).map(|i| format!("new {i}\n")).collect();
    let diff = crate::services::ddl_save::unified_diff_summary(&old, &new);
    assert!(diff.len() < 4_000, "{}", diff.len());
    assert!(diff.contains("diff truncated"), "{diff}");
}

#[test]
fn unified_diff_notes_line_ending_only_changes() {
    let diff = crate::services::ddl_save::unified_diff_summary("a\r\nb\r\n", "a\nb\n");
    assert!(diff.contains("line endings"), "{diff}");
}
