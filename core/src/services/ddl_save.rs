use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;

use crate::services::folder_schema_overrides::SchemaFolderOverrideService;
use crate::services::git;

/// Pseudo object type for an Oracle package's spec and body kept together in one file
/// (the `.pck` convention): spec, a `/` line, body, a `/` line.
pub const PACKAGE_SPEC_AND_BODY: &str = "PACKAGE SPEC AND BODY";

pub fn is_package_spec_and_body(object_type: &str) -> bool {
    object_type
        .trim()
        .eq_ignore_ascii_case(PACKAGE_SPEC_AND_BODY)
}

/// The object type whose folder/extension/storage settings apply. The combined type has
/// no settings of its own and follows PACKAGE, so it is saved with the spec's `.pck`.
pub fn settings_object_type(object_type: &str) -> &str {
    if is_package_spec_and_body(object_type) {
        "PACKAGE"
    } else {
        object_type
    }
}

/// Drops a trailing SQL*Plus `/` line (as emitted by DBMS_METADATA) and surrounding whitespace.
fn strip_slash_terminator(ddl: &str) -> &str {
    let ddl = ddl.trim();
    match ddl.rsplit_once('\n') {
        Some((head, last)) if last.trim() == "/" => head.trim_end(),
        _ if ddl == "/" => "",
        _ => ddl,
    }
}

/// Joins a package spec and body into one script: each statement is followed by its own
/// `/` line, separated by a blank line.
pub fn combine_package_spec_and_body(spec: &str, body: &str) -> String {
    format!(
        "{}\n/\n\n{}\n/\n",
        strip_slash_terminator(spec),
        strip_slash_terminator(body)
    )
}

/// Whether the DDL holds both a `CREATE [OR REPLACE] PACKAGE` and a `... PACKAGE BODY` statement.
pub fn has_package_spec_and_body(ddl: &str) -> bool {
    let create = regex::Regex::new(
        r#"(?i)\bCREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:NON)?EDITIONABLE\s+)?PACKAGE\s+(BODY\s+)?"#,
    )
    .expect("valid regex");
    let (mut spec, mut body) = (false, false);
    for found in create.captures_iter(ddl) {
        if found.get(1).is_some() {
            body = true;
        } else {
            spec = true;
        }
    }
    spec && body
}

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

/// The configured schema root folder, or the message to show when it isn't set.
pub fn schema_root_from_settings() -> Result<String, String> {
    crate::services::settings::load_schema_root_folder()
        .filter(|root| !root.trim().is_empty())
        .ok_or_else(|| "Schema folder not configured. Please set it in Settings.".to_string())
}

/// Save options from the global settings, with the same fallbacks the GUI applies.
pub fn options_from_settings(object_type: &str) -> DdlSaveOptions {
    use crate::services::settings as s;
    let object_type = settings_object_type(object_type);
    DdlSaveOptions {
        code_folder: s::load_code_folder_name().unwrap_or_else(|| "code".to_string()),
        migration_folder: s::load_migration_folder_name()
            .unwrap_or_else(|| "migration".to_string()),
        storage_mode: s::resolve_storage_mode(object_type),
        naming_convention: s::load_naming_convention().unwrap_or_else(|| "timestamp".to_string()),
        folder_mode: s::load_migration_folder_mode().unwrap_or_else(|| "year".to_string()),
        version_label: s::load_migration_version_label().unwrap_or_default(),
        extension: s::resolve_ddl_extension(object_type),
        encoding: s::load_ddl_file_encoding().unwrap_or_else(|| "utf8".to_string()),
    }
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

/// One file a save would write.
#[derive(Clone, Debug)]
pub struct PlannedFile {
    /// `"code"` or `"migration"`.
    pub kind: &'static str,
    pub path: PathBuf,
    pub content: Vec<u8>,
    /// The file is already there and would be overwritten.
    pub exists: bool,
    /// Canonical name of the encoding `content` is in, see [`encoding_label`].
    pub encoding: &'static str,
}

/// Canonical name of the encoding `encode_content` produces for a configured value.
pub fn encoding_label(encoding: &str) -> &'static str {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "utf8-bom" => "utf8-bom",
        "windows-1257" => "windows-1257",
        _ => "utf8",
    }
}

/// Inverse of `encode_content`. The flag is true when bytes were not valid in the encoding
/// and had to be replaced.
pub fn decode_content(bytes: &[u8], encoding: &str) -> (String, bool) {
    match encoding_label(encoding) {
        "windows-1257" => {
            let (text, had_errors) = encoding_rs::WINDOWS_1257.decode_without_bom_handling(bytes);
            (text.into_owned(), had_errors)
        }
        label => {
            let bytes = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
                Some(rest) if label == "utf8-bom" => rest,
                _ => bytes,
            };
            match std::str::from_utf8(bytes) {
                Ok(text) => (text.to_string(), false),
                Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DdlSavePlan {
    pub files: Vec<PlannedFile>,
}

impl DdlSavePlan {
    pub fn result(&self) -> SaveDdlResult {
        let path_of = |kind: &str| {
            self.files
                .iter()
                .find(|file| file.kind == kind)
                .map(|file| file.path.to_string_lossy().into_owned())
        };
        SaveDdlResult {
            code_path: path_of("code"),
            migration_path: path_of("migration"),
        }
    }
}

/// Writes every planned file, creating folders as needed.
pub fn write_plan(plan: &DdlSavePlan) -> Result<SaveDdlResult, String> {
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create {} dir: {error}", file.kind))?;
        }
        fs::write(&file.path, &file.content)
            .map_err(|error| format!("Failed to write {} file: {error}", file.kind))?;
    }
    Ok(plan.result())
}

pub fn save_ddl_to_folder(
    folder_overrides: &SchemaFolderOverrideService,
    schema_root: &str,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
) -> Result<SaveDdlResult, String> {
    write_plan(&plan_ddl_save(
        folder_overrides,
        schema_root,
        args,
        options,
    )?)
}

/// Works out which files a save produces — paths and content — without touching disk.
pub fn plan_ddl_save(
    folder_overrides: &SchemaFolderOverrideService,
    schema_root: &str,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
) -> Result<DdlSavePlan, String> {
    plan_inner(folder_overrides, schema_root, args, options, None)
}

/// Like [`plan_ddl_save`], but every planned file gets `raw` exactly as given: no
/// idempotency wrapping and no re-encoding. `args.ddl` is ignored.
pub fn plan_ddl_save_raw(
    folder_overrides: &SchemaFolderOverrideService,
    schema_root: &str,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
    raw: &[u8],
) -> Result<DdlSavePlan, String> {
    plan_inner(folder_overrides, schema_root, args, options, Some(raw))
}

fn plan_inner(
    folder_overrides: &SchemaFolderOverrideService,
    schema_root: &str,
    args: &DdlSaveArgs,
    options: &DdlSaveOptions,
    raw: Option<&[u8]>,
) -> Result<DdlSavePlan, String> {
    if is_package_spec_and_body(&args.object_type) && args.db_type.eq_ignore_ascii_case("postgres")
    {
        return Err(format!(
            "{PACKAGE_SPEC_AND_BODY} applies to Oracle packages only; PostgreSQL has no packages."
        ));
    }
    let schema_lower = args.schema.to_lowercase();
    let schema_dir = resolve_schema_dir(folder_overrides, &schema_lower, schema_root)?;
    let code_folder = folder_overrides
        .resolve_code_folder_name(&schema_lower, &options.code_folder)
        .map_err(|error| error.to_string())?;
    let migration_folder = folder_overrides
        .resolve_migration_folder_name(&schema_lower, &options.migration_folder)
        .map_err(|error| error.to_string())?;
    let resolved_storage_mode = folder_overrides
        .resolve_storage_mode(
            &schema_lower,
            settings_object_type(&args.object_type),
            &options.storage_mode,
        )
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
        .resolve_extension(
            &schema_lower,
            settings_object_type(&args.object_type),
            &options.extension,
        )
        .map_err(|error| error.to_string())?;
    let encoding = folder_overrides
        .resolve_encoding(&schema_lower, &options.encoding)
        .map_err(|error| error.to_string())?;

    let code_dir = schema_dir.join(code_folder).join(&schema_lower);
    let migration_dir = schema_dir.join(migration_folder);
    let write_code = storage_mode != "migration";
    let write_migration = storage_mode != "code";
    let mut plan = DdlSavePlan::default();

    if write_code {
        let code_filename = if naming_convention == "flyway" {
            format!("R__{}.{}", args.object_name, extension)
        } else {
            format!("{}.{}", args.object_name, extension)
        };
        let code_file = code_dir.join(code_filename);
        plan.files.push(PlannedFile {
            kind: "code",
            exists: code_file.exists(),
            content: match raw {
                Some(bytes) => bytes.to_vec(),
                None => encode_content(&args.ddl, &encoding),
            },
            encoding: encoding_label(&encoding),
            path: code_file,
        });
    }

    if write_migration {
        let migration_ddl = if raw.is_some() {
            String::new()
        } else if args.db_type.eq_ignore_ascii_case("postgres") {
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
        let migration_content = match raw {
            Some(bytes) => bytes.to_vec(),
            None => encode_content(&migration_ddl, &encoding),
        };
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
        plan.files.push(PlannedFile {
            kind: "migration",
            exists: migration_file.exists(),
            content: migration_content,
            encoding: encoding_label(&encoding),
            path: migration_file,
        });
    }

    Ok(plan)
}

const DIFF_CONTEXT_LINES: usize = 2;
const MAX_DIFF_LINES: usize = 40;
const MAX_DIFF_CHARS: usize = 3_000;
/// Line-pair budget for the LCS table; larger edits are summarised by counts only.
const MAX_DIFF_CELLS: usize = 4_000_000;

/// Overwrite-safety verdict for one planned file.
#[derive(Clone, Debug)]
pub struct FileGuard {
    pub kind: &'static str,
    pub path: PathBuf,
    pub exists: bool,
    pub git_status: git::FileStatus,
    /// The file has uncommitted work that the new content would replace.
    pub at_risk: bool,
    /// Bounded unified-style diff (existing → new); only set for at-risk files.
    pub diff: Option<String>,
}

impl FileGuard {
    /// `"new"` when nothing exists yet, otherwise the git status.
    pub fn status_label(&self) -> &'static str {
        if self.exists {
            self.git_status.as_str()
        } else {
            "new"
        }
    }
}

#[derive(Clone, Debug)]
pub struct OverwriteGuard {
    pub git: git::RepoStatus,
    pub files: Vec<FileGuard>,
}

impl OverwriteGuard {
    pub fn at_risk(&self) -> impl Iterator<Item = &FileGuard> {
        self.files.iter().filter(|file| file.at_risk)
    }

    /// Why the save is refused, with a diff summary per file; `None` when it is safe.
    pub fn refusal_message(&self) -> Option<String> {
        let mut parts = Vec::new();
        for file in self.at_risk() {
            let mut part = if file.git_status == git::FileStatus::Unknown {
                format!(
                    "{} could not be checked against git, so this save would overwrite it unverified.",
                    file.path.display()
                )
            } else {
                format!(
                    "{} has uncommitted changes in git ({}) that this save would overwrite.",
                    file.path.display(),
                    file.git_status.as_str()
                )
            };
            // No diff here: this text is also written to the append-only audit log. The
            // dry run returns the diff.
            if let Some(diff) = &file.diff {
                part.push_str(&format!(
                    " ({} changed lines; run with dry_run to see the diff)",
                    diff.lines()
                        .filter(|l| l.starts_with('+') || l.starts_with('-'))
                        .count()
                ));
            }
            parts.push(part);
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "Refusing to overwrite {}. Commit or stash the local changes, or pass force: true to overwrite anyway.\n\n{}",
            if parts.len() == 1 { "a file" } else { "files" },
            parts.join("\n\n")
        ))
    }
}

/// Checks every planned file that already exists against git. Files whose content would not
/// change are never at risk; files git cannot vouch for (no repository) are allowed but
/// reported as `unknown`.
pub fn assess_overwrites(plan: &DdlSavePlan) -> OverwriteGuard {
    let git = plan
        .files
        .first()
        .and_then(|file| file.path.parent())
        .map(git::repo_status)
        .unwrap_or_else(|| git::RepoStatus::Unknown("nothing to save".to_string()));
    let files = plan
        .files
        .iter()
        .map(|file| {
            let mut guard = FileGuard {
                kind: file.kind,
                path: file.path.clone(),
                exists: file.exists,
                git_status: git::FileStatus::Unknown,
                at_risk: false,
                diff: None,
            };
            if !file.exists {
                return guard;
            }
            // Git failing inside a work tree (missing binary, dubious ownership, ...) says
            // nothing about the file, so an overwrite is refused rather than waved through.
            let in_work_tree = file
                .path
                .ancestors()
                .skip(1)
                .any(|dir| dir.join(".git").exists());
            if !git.is_known() && !in_work_tree {
                return guard;
            }
            if git.is_known() {
                guard.git_status = git::file_status(&file.path);
                if guard.git_status != git::FileStatus::Unknown
                    && !guard.git_status.has_uncommitted_changes()
                {
                    return guard;
                }
            }
            match fs::read(&file.path) {
                Ok(existing) if existing == file.content => {}
                Ok(existing) => {
                    guard.at_risk = true;
                    guard.diff = Some(unified_diff_summary(
                        &decode_lossy(&existing),
                        &decode_lossy(&file.content),
                    ));
                }
                Err(_) => guard.at_risk = true,
            }
            guard
        })
        .collect();
    OverwriteGuard { git, files }
}

fn decode_lossy(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// A bounded unified-style line diff from `old` to `new`.
pub fn unified_diff_summary(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];

    if old_mid.is_empty() && new_mid.is_empty() {
        return "(only line endings or encoding differ)".to_string();
    }
    if old_mid.len().saturating_mul(new_mid.len()) > MAX_DIFF_CELLS {
        return format!(
            "(too large for a line diff: {} existing lines, {} new lines, {} unchanged lines at the top/bottom excluded)",
            old_lines.len(),
            new_lines.len(),
            prefix + suffix
        );
    }

    // (marker, line, 1-based old line, 1-based new line) of every line in the merged view.
    let mut ops: Vec<(char, &str, usize, usize)> = Vec::new();
    let (mut o, mut n) = (1usize, 1usize);
    for line in &old_lines[..prefix] {
        ops.push((' ', line, o, n));
        o += 1;
        n += 1;
    }
    for (marker, line) in lcs_ops(old_mid, new_mid) {
        ops.push((marker, line, o, n));
        if marker != '+' {
            o += 1;
        }
        if marker != '-' {
            n += 1;
        }
    }
    for line in &old_lines[old_lines.len() - suffix..] {
        ops.push((' ', line, o, n));
        o += 1;
        n += 1;
    }

    let added = ops.iter().filter(|op| op.0 == '+').count();
    let removed = ops.iter().filter(|op| op.0 == '-').count();
    let mut out = format!("--- existing\n+++ new (+{added} -{removed} lines)\n");
    let mut lines_written = 0usize;
    let mut truncated = false;

    let changed: Vec<usize> = (0..ops.len()).filter(|&i| ops[i].0 != ' ').collect();
    let mut idx = 0;
    'hunks: while idx < changed.len() {
        let start = changed[idx].saturating_sub(DIFF_CONTEXT_LINES);
        let mut end = changed[idx];
        while idx < changed.len() && changed[idx] <= end + 2 * DIFF_CONTEXT_LINES + 1 {
            end = changed[idx];
            idx += 1;
        }
        let end = (end + DIFF_CONTEXT_LINES).min(ops.len() - 1);
        let hunk = &ops[start..=end];
        let old_count = hunk.iter().filter(|op| op.0 != '+').count();
        let new_count = hunk.iter().filter(|op| op.0 != '-').count();
        let header = format!(
            "@@ -{},{} +{},{} @@\n",
            hunk[0].2, old_count, hunk[0].3, new_count
        );
        if lines_written + 1 > MAX_DIFF_LINES || out.len() + header.len() > MAX_DIFF_CHARS {
            truncated = true;
            break;
        }
        out.push_str(&header);
        lines_written += 1;
        for (marker, line, _, _) in hunk {
            let text = format!("{marker}{line}\n");
            if lines_written >= MAX_DIFF_LINES || out.len() + text.len() > MAX_DIFF_CHARS {
                truncated = true;
                break 'hunks;
            }
            out.push_str(&text);
            lines_written += 1;
        }
    }
    if truncated {
        out.push_str("... (diff truncated)\n");
    }
    out.trim_end().to_string()
}

/// Edit script (` `, `-`, `+`) from `old` to `new` via longest common subsequence.
fn lcs_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(char, &'a str)> {
    let (n, m) = (old.len(), new.len());
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if old[i] == new[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(n + m);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push((' ', old[i]));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            ops.push(('-', old[i]));
            i += 1;
        } else {
            ops.push(('+', new[j]));
            j += 1;
        }
    }
    ops.extend(old[i..].iter().map(|line| ('-', *line)));
    ops.extend(new[j..].iter().map(|line| ('+', *line)));
    ops
}

#[cfg(test)]
#[path = "tests/ddl_save.rs"]
mod tests;
