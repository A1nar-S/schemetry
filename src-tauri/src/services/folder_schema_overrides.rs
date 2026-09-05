use std::sync::Arc;

use anyhow::Result;

use crate::models::SchemaFolderOverride;
use crate::repositories::folder_schema_override_repository::SchemaFolderOverrideRepository;

pub struct SchemaFolderOverrideService {
    repo: Arc<dyn SchemaFolderOverrideRepository>,
}

impl SchemaFolderOverrideService {
    pub fn new(repo: Arc<dyn SchemaFolderOverrideRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    pub fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>> {
        self.repo.list_overrides()
    }

    /// Insert a new override (id == 0) or update an existing one.
    pub fn save_override(&self, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        if data.id == 0 {
            self.repo.insert_override(data)
        } else {
            self.repo.update_override(data.id, data)
        }
    }

    pub fn delete_override(&self, id: i64) -> Result<()> {
        self.repo.delete_override(id)
    }

    /// The folder root for a schema: its overridden folder if one is set, otherwise
    /// `<schema_root_folder>/<schema_lower>`.
    pub fn resolve_schema_dir(&self, schema_name: &str, schema_root_folder: &str) -> Result<std::path::PathBuf> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.folder_path.trim().is_empty() => Ok(std::path::PathBuf::from(o.folder_path)),
            _ => Ok(std::path::PathBuf::from(schema_root_folder).join(schema_name.to_lowercase())),
        }
    }

    /// The DDL file encoding for a schema: its overridden encoding if one is set,
    /// otherwise the global default.
    pub fn resolve_encoding(&self, schema_name: &str, default_encoding: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.encoding.trim().is_empty() => Ok(o.encoding),
            _ => Ok(default_encoding.to_string()),
        }
    }

    /// The DDL file extension for a schema and object type: the schema's overridden
    /// extension for that type if one is set, otherwise `default_ext` (the resolved
    /// global setting for that type).
    pub fn resolve_extension(&self, schema_name: &str, object_type: &str, default_ext: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        if let Some(o) = over {
            if !o.extensions.trim().is_empty() {
                if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(&o.extensions) {
                    let key = object_type.trim().to_ascii_uppercase();
                    if let Some(ext) = map.get(&key).filter(|v| !v.trim().is_empty()) {
                        return Ok(ext.trim().to_string());
                    }
                }
            }
        }
        Ok(default_ext.to_string())
    }

    /// The DDL storage mode ("code", "migration", or "both") for a schema and object
    /// type: the schema's overridden mode for that type if one is set, otherwise
    /// `default_mode` (the resolved global setting for that type).
    pub fn resolve_storage_mode(&self, schema_name: &str, object_type: &str, default_mode: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        if let Some(o) = over {
            if !o.storage_modes.trim().is_empty() {
                if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(&o.storage_modes) {
                    let key = object_type.trim().to_ascii_uppercase();
                    if let Some(mode) = map.get(&key).filter(|v| !v.trim().is_empty()) {
                        return Ok(mode.trim().to_string());
                    }
                }
            }
        }
        Ok(default_mode.to_string())
    }

    /// The DDL file naming convention for a schema: its overridden convention if one is
    /// set, otherwise the global default.
    pub fn resolve_naming_convention(&self, schema_name: &str, default_convention: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.naming_convention.trim().is_empty() => Ok(o.naming_convention),
            _ => Ok(default_convention.to_string()),
        }
    }

    /// The subfolder name for raw code DDL files for a schema: its overridden name if one
    /// is set, otherwise the global default.
    pub fn resolve_code_folder_name(&self, schema_name: &str, default_name: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.code_folder_name.trim().is_empty() => Ok(o.code_folder_name),
            _ => Ok(default_name.to_string()),
        }
    }

    /// The subfolder name for migration scripts for a schema: its overridden name if one
    /// is set, otherwise the global default.
    pub fn resolve_migration_folder_name(&self, schema_name: &str, default_name: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.migration_folder_name.trim().is_empty() => Ok(o.migration_folder_name),
            _ => Ok(default_name.to_string()),
        }
    }

    /// The migration subfolder mode ("year" or "version") for a schema: its overridden
    /// mode if one is set, otherwise the global default.
    pub fn resolve_migration_folder_mode(&self, schema_name: &str, default_mode: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.migration_folder_mode.trim().is_empty() => Ok(o.migration_folder_mode),
            _ => Ok(default_mode.to_string()),
        }
    }

    /// The manually-set migration version-folder label for a schema: its overridden label
    /// if one is set, otherwise the global default.
    pub fn resolve_migration_version_label(&self, schema_name: &str, default_label: &str) -> Result<String> {
        let over = self.repo.find_for_schema(schema_name)?;
        match over {
            Some(o) if !o.migration_version_label.trim().is_empty() => Ok(o.migration_version_label),
            _ => Ok(default_label.to_string()),
        }
    }
}
