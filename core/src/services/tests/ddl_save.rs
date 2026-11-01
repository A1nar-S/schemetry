use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::models::SchemaFolderOverride;
use crate::repositories::folder_schema_override_repository::SchemaFolderOverrideRepository;
use crate::services::ddl_save::{
    DdlSaveArgs, DdlSaveOptions, PACKAGE_SPEC_AND_BODY, combine_package_spec_and_body,
    encode_content, has_package_spec_and_body, is_package_spec_and_body, next_migration_version,
    plan_ddl_save, resolve_schema_dir, save_ddl_to_folder as save_ddl_service,
    settings_object_type,
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
    assert_eq!(encode_content("abc", "utf8"), b"abc");
    assert_eq!(encode_content("abc", "utf8-bom"), b"\xEF\xBB\xBFabc");
    assert_eq!(
        encode_content("Ā", "windows-1257"),
        encoding_rs::WINDOWS_1257.encode("Ā").0.into_owned()
    );
    assert_eq!(encode_content("abc", "unknown"), b"abc");
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
