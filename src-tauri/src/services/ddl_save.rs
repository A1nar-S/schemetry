use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;

use crate::services::folder_schema_overrides::SchemaFolderOverrideService;

#[derive(Clone, Debug)]
pub struct DdlSaveArgs {
    pub schema: String,
    pub object_name: String,
    pub object_type: String,
    pub ddl: String,
    pub description: String,
    pub db_type: String,
}

#[derive(Clone, Debug)]
pub struct DdlSaveOptions {
    pub code_folder: String,
    pub migration_folder: String,
    pub storage_mode: String,
    pub naming_convention: String,
    pub folder_mode: String,
    pub version_label: String,
    pub extension: String,
    pub encoding: String,
}

#[derive(Debug, Serialize)]
pub struct SaveDdlResult {
    pub code_path: Option<String>,
    pub migration_path: Option<String>,
}

pub fn resolve_schema_dir(
    folder_overrides: &SchemaFolderOverrideService,
    schema: &str,
    schema_root: &str,
) -> Result<PathBuf, String> {
    folder_overrides
        .resolve_schema_dir(&schema.to_lowercase(), schema_root)
        .map_err(|error| error.to_string())
}

fn encode_content(content: &str, encoding: &str) -> Vec<u8> {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "utf8-bom" => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(content.as_bytes());
            bytes
        }
        "windows-1257" => {
            let (encoded, _, _) = encoding_rs::WINDOWS_1257.encode(content);
            encoded.into_owned()
        }
        _ => content.as_bytes().to_vec(),
    }
}

/// Return the next Flyway version found in direct files or one level of subfolders.
fn next_migration_version(migration_dir: &Path) -> u32 {
    fn parse_version(name: &str) -> Option<u32> {
        let rest = name.strip_prefix('V')?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || !rest[digits.len()..].starts_with("__") {
            return None;
        }
        digits.parse().ok()
    }

    fn scan_dir_max(dir: &Path, max: &mut u32) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if let Some(version) = parse_version(name) {
                    *max = (*max).max(version);
                }
            }
        }
    }

    let mut max = 0u32;
    scan_dir_max(migration_dir, &mut max);
    if let Ok(entries) = fs::read_dir(migration_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir_max(&path, &mut max);
            }
        }
    }
    max + 1
}

pub fn save_ddl_to_folder(
    folder_overrides: &SchemaFolderOverrideService,
    schema_root: &str,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
) -> Result<SaveDdlResult, String> {
    let schema_lower = args.schema.to_lowercase();
    let schema_dir = resolve_schema_dir(folder_overrides, &schema_lower, schema_root)?;
    let code_folder = folder_overrides
        .resolve_code_folder_name(&schema_lower, &options.code_folder)
        .map_err(|error| error.to_string())?;
    let migration_folder = folder_overrides
        .resolve_migration_folder_name(&schema_lower, &options.migration_folder)
        .map_err(|error| error.to_string())?;
    let resolved_storage_mode = folder_overrides
        .resolve_storage_mode(&schema_lower, &args.object_type, &options.storage_mode)
        .map_err(|error| error.to_string())?;
    let storage_mode = match resolved_storage_mode.trim() {
        mode @ ("code" | "migration" | "both") => mode,
        _ => "both",
    };
    let naming_convention = folder_overrides
        .resolve_naming_convention(&schema_lower, &options.naming_convention)
        .map_err(|error| error.to_string())?;
    let folder_mode = folder_overrides
        .resolve_migration_folder_mode(&schema_lower, &options.folder_mode)
        .map_err(|error| error.to_string())?;
    let version_label = folder_overrides
        .resolve_migration_version_label(&schema_lower, &options.version_label)
        .map_err(|error| error.to_string())?;
    let extension = folder_overrides
        .resolve_extension(&schema_lower, &args.object_type, &options.extension)
        .map_err(|error| error.to_string())?;
    let encoding = folder_overrides
        .resolve_encoding(&schema_lower, &options.encoding)
        .map_err(|error| error.to_string())?;

    let code_dir = schema_dir.join(code_folder).join(&schema_lower);
    let migration_dir = schema_dir.join(migration_folder);
    let write_code = storage_mode != "migration";
    let write_migration = storage_mode != "code";
    let mut code_path = None;
    let mut migration_path = None;

    if write_code {
        fs::create_dir_all(&code_dir)
            .map_err(|error| format!("Failed to create code dir: {error}"))?;
        let code_filename = if naming_convention == "flyway" {
            format!("R__{}.{}", args.object_name, extension)
        } else {
            format!("{}.{}", args.object_name, extension)
        };
        let code_file = code_dir.join(code_filename);
        fs::write(&code_file, encode_content(&args.ddl, &encoding))
            .map_err(|error| format!("Failed to write code file: {error}"))?;
        code_path = Some(code_file.to_string_lossy().into_owned());
    }

    if write_migration {
        fs::create_dir_all(&migration_dir)
            .map_err(|error| format!("Failed to create migration dir: {error}"))?;

        let migration_ddl = if args.db_type.eq_ignore_ascii_case("postgres") {
            crate::repositories::postgres_repository::build_deploy_script(
                &args.object_type.to_ascii_uppercase(),
                &args.object_name,
                &args.ddl,
            )
        } else {
            crate::repositories::oracle_repository::build_deploy_script(
                &args.object_type.to_ascii_uppercase(),
                &args.object_name,
                &args.ddl,
            )
        };
        let migration_content = encode_content(&migration_ddl, &encoding);
        let now = Local::now();

        let primary_name = if folder_mode == "version" {
            version_label.trim().to_string()
        } else {
            now.format("%Y").to_string()
        };
        let primary_dir = (!primary_name.is_empty()).then(|| migration_dir.join(&primary_name));
        let new_dir = migration_dir.join("new");
        let actual_migration_dir = match primary_dir {
            Some(dir) if dir.is_dir() => dir,
            _ if new_dir.is_dir() => new_dir,
            _ => migration_dir.clone(),
        };

        let migration_filename = if naming_convention == "flyway" {
            let version = next_migration_version(&migration_dir);
            format!(
                "V{}__{}_{}.{}",
                version, schema_lower, args.description, extension
            )
        } else {
            let timestamp = now.format("%y%m%d_%H%M").to_string();
            format!(
                "{}_{}${}.{}",
                timestamp, schema_lower, args.description, extension
            )
        };
        let migration_file = actual_migration_dir.join(&migration_filename);
        fs::write(&migration_file, &migration_content)
            .map_err(|error| format!("Failed to write migration file: {error}"))?;
        migration_path = Some(migration_file.to_string_lossy().into_owned());
    }

    Ok(SaveDdlResult {
        code_path,
        migration_path,
    })
}

#[cfg(test)]
#[path = "tests/ddl_save.rs"]
mod tests;
