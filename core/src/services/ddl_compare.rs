//! Comparing a DDL file in the schema folder with the DDL the server reports: confined
//! file access, decoding, a pure normaliser and a bounded unified diff. Everything here
//! is deterministic and free of database access so the MCP `compare_object` tool can be
//! tested without a server.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::models::DbType;

pub const MAX_FILE_PATH_CHARS: usize = 512;
/// Longest diff line returned; longer lines are cut with an ellipsis.
const MAX_DIFF_LINE_CHARS: usize = 400;
/// Largest `lines × lines` table the diff will fill; beyond it the differing middle is
/// reported as one replaced block instead of an exact minimal diff.
const MAX_DIFF_CELLS: usize = 4_000_000;
const CONTEXT_LINES: usize = 3;

/// Why a file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileAccessError {
    /// The request itself is not allowed (bad or escaping path, file too large).
    Denied(String),
    /// The request was fine but the file system said no.
    Failed(String),
}

/// Checks a schema-folder-relative path without touching the disk: no absolute paths,
/// drive letters / stream syntax, `..` segments, empty or control-character names.
pub fn validate_relative_path(file_path: &str) -> Result<PathBuf, String> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return Err("file_path must not be empty.".to_string());
    }
    if trimmed.chars().count() > MAX_FILE_PATH_CHARS {
        return Err(format!(
            "file_path is longer than {MAX_FILE_PATH_CHARS} characters."
        ));
    }
    if trimmed.chars().any(|c| c.is_control() || c == ':') {
        return Err("file_path contains characters that are not allowed.".to_string());
    }
    if trimmed.starts_with(['/', '\\']) || Path::new(trimmed).is_absolute() {
        return Err("file_path must be relative to the schema folder.".to_string());
    }
    let mut clean = PathBuf::new();
    for segment in trimmed.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => return Err("file_path must not contain '..'.".to_string()),
            other => clean.push(other),
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("file_path must name a file.".to_string());
    }
    // The diff echoes differing lines, so only source-code files may be named.
    let extension = clean
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !DDL_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!(
            "file_path must be a DDL source file ({}).",
            DDL_EXTENSIONS.join(", ")
        ));
    }
    Ok(clean)
}

const DDL_EXTENSIONS: &[&str] = &[
    "sql", "ddl", "pck", "pks", "pkb", "pls", "plb", "prc", "fnc", "trg", "vw", "tps", "tpb",
];

/// Reads `relative` below `schema_dir`, refusing anything that resolves (symlinks
/// included) outside it, anything that is not a regular file and anything over
/// `max_bytes`.
pub fn read_confined_file(
    schema_dir: &Path,
    relative: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, FileAccessError> {
    let root = schema_dir.canonicalize().map_err(|_| {
        FileAccessError::Failed("The schema folder does not exist or cannot be read.".to_string())
    })?;
    let target = root.join(relative).canonicalize().map_err(|_| {
        FileAccessError::Failed("The file was not found in the schema folder.".to_string())
    })?;
    if !target.starts_with(&root) {
        return Err(FileAccessError::Denied(
            "file_path resolves outside the schema folder.".to_string(),
        ));
    }
    let metadata = fs::metadata(&target)
        .map_err(|_| FileAccessError::Failed("The file could not be read.".to_string()))?;
    if !metadata.is_file() {
        return Err(FileAccessError::Denied(
            "file_path must name a regular file.".to_string(),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(FileAccessError::Denied(format!(
            "The file is larger than the {max_bytes}-byte limit."
        )));
    }
    let file = fs::File::open(&target)
        .map_err(|_| FileAccessError::Failed("The file could not be read.".to_string()))?;
    // Bounded again at read time in case the file grew after the size check.
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FileAccessError::Failed("The file could not be read.".to_string()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(FileAccessError::Denied(format!(
            "The file is larger than the {max_bytes}-byte limit."
        )));
    }
    Ok(bytes)
}

/// Decodes a DDL file with the encoding the folder is configured to save in
/// (`utf8`, `utf8-bom` or `windows-1257`; anything else is treated as UTF-8, like the
/// writer does). A UTF-8 BOM is dropped.
pub fn decode_ddl_file(bytes: &[u8], encoding: &str) -> Result<String, String> {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "windows-1257" => Ok(encoding_rs::WINDOWS_1257.decode(bytes).0.into_owned()),
        _ => {
            let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                "The file is not valid UTF-8; check the folder's configured file encoding."
                    .to_string()
            })
        }
    }
}

// ── normalisation ───────────────────────────────────────────────────────────────

/// Normalises DDL text so cosmetic differences do not count as differences. Applied to
/// both sides of a comparison, so every step must be symmetric.
///
/// For every engine:
/// * a leading BOM is dropped and `\r\n` / `\r` become `\n`;
/// * trailing whitespace is removed from every line;
/// * leading and trailing blank lines are removed;
/// * one trailing `;` is removed.
///
/// For Oracle additionally:
/// * a trailing `/` line (SQL*Plus terminator) is removed;
/// * the `CREATE` header is canonicalised — see [`normalize_oracle_header`].
pub fn normalize_ddl(text: &str, db_type: DbType, schema: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = unified.split('\n').map(str::trim_end).collect();

    fn trim_blank_edges(lines: &mut Vec<&str>) {
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        let leading = lines.iter().take_while(|line| line.is_empty()).count();
        lines.drain(..leading);
    }

    trim_blank_edges(&mut lines);
    if db_type == DbType::Oracle && lines.last().is_some_and(|line| line.trim() == "/") {
        lines.pop();
        trim_blank_edges(&mut lines);
    }
    let mut joined = lines.join("\n");
    if joined.ends_with(';') {
        joined.pop();
        let trimmed = joined.trim_end().len();
        joined.truncate(trimmed);
    }
    match db_type {
        DbType::Oracle => normalize_oracle_header(&joined, schema),
        DbType::Postgres => joined,
    }
}

const ORACLE_TYPE_PREFIXES: [&str; 7] = [
    "GLOBAL",
    "TEMPORARY",
    "PRIVATE",
    "UNIQUE",
    "BITMAP",
    "MATERIALIZED",
    "PUBLIC",
];
const ORACLE_TYPE_HEADS: [&str; 8] = [
    "TABLE",
    "INDEX",
    "VIEW",
    "PACKAGE",
    "TYPE",
    "SEQUENCE",
    "PROCEDURE",
    "FUNCTION",
];
const ORACLE_EXTRA_HEADS: [&str; 2] = ["TRIGGER", "SYNONYM"];

fn skip_whitespace(text: &str, mut pos: usize) -> usize {
    while text[pos..].starts_with(|c: char| c.is_whitespace()) {
        pos += text[pos..].chars().next().map_or(1, char::len_utf8);
    }
    pos
}

/// The next whitespace-delimited bare word (letters, digits, `_`), as `(word, end)`.
fn next_word(text: &str, pos: usize) -> Option<(&str, usize)> {
    let start = skip_whitespace(text, pos);
    let len = text[start..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len() - start);
    (len > 0).then(|| (&text[start..start + len], start + len))
}

/// One identifier part starting exactly at `pos`: `"Quoted"` or a bare name.
/// Returns `(canonical form, end, content compared case-insensitively)`.
fn identifier_part(text: &str, pos: usize) -> Option<(String, usize, String)> {
    let rest = &text[pos..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let close = quoted.find('"')?;
        let content = &quoted[..close];
        if content.is_empty() {
            return None;
        }
        let plain = content.starts_with(|c: char| c.is_ascii_uppercase())
            && content.chars().all(|c| {
                c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '_' | '$' | '#')
            });
        // Quoting an all-upper-case simple name changes nothing in Oracle.
        let canonical = if plain {
            content.to_string()
        } else {
            format!("\"{content}\"")
        };
        return Some((canonical, pos + close + 2, content.to_ascii_uppercase()));
    }
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '#')))
        .unwrap_or(rest.len());
    if len == 0 || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let name = rest[..len].to_ascii_uppercase();
    Some((name.clone(), pos + len, name))
}

/// Canonicalises the `CREATE …` header of an Oracle object so these compare equal:
///
/// * keyword case and whitespace (`create   or replace\npackage body`);
/// * `OR REPLACE` present or absent;
/// * `EDITIONABLE` present or absent (`NONEDITIONABLE`, `FORCE`, … are kept: they differ
///   in meaning);
/// * a schema prefix equal to `schema` (`"APP"."ORDERS"`, `app.orders`, `orders`);
/// * quoted all-upper-case names (`"ORDERS"` = `orders` = `ORDERS`).
///
/// The result starts `CREATE <TYPE> <NAME>`; the text after the name is untouched apart
/// from collapsing the spacing right behind the name. Text that does not start with a
/// recognisable `CREATE` header is returned unchanged.
pub fn normalize_oracle_header(text: &str, schema: &str) -> String {
    parse_oracle_header(text, schema).unwrap_or_else(|| text.to_string())
}

fn parse_oracle_header(text: &str, schema: &str) -> Option<String> {
    let (create, mut pos) = next_word(text, 0)?;
    if !create.eq_ignore_ascii_case("CREATE") {
        return None;
    }

    let mut modifiers: Vec<String> = Vec::new();
    loop {
        let (word, end) = next_word(text, pos)?;
        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            "OR" => {
                let (next, next_end) = next_word(text, end)?;
                if !next.eq_ignore_ascii_case("REPLACE") {
                    return None;
                }
                pos = next_end;
            }
            "EDITIONABLE" => pos = end,
            "NONEDITIONABLE" | "FORCE" | "NOFORCE" | "EDITIONING" => {
                modifiers.push(upper);
                pos = end;
            }
            _ => break,
        }
    }

    let mut type_words: Vec<String> = Vec::new();
    loop {
        let (word, end) = next_word(text, pos)?;
        let upper = word.to_ascii_uppercase();
        pos = end;
        if ORACLE_TYPE_PREFIXES.contains(&upper.as_str()) {
            type_words.push(upper);
        } else if ORACLE_TYPE_HEADS.contains(&upper.as_str())
            || ORACLE_EXTRA_HEADS.contains(&upper.as_str())
        {
            let takes_body = matches!(upper.as_str(), "PACKAGE" | "TYPE");
            type_words.push(upper);
            if takes_body
                && let Some((body, body_end)) = next_word(text, pos)
                && body.eq_ignore_ascii_case("BODY")
            {
                type_words.push("BODY".to_string());
                pos = body_end;
            }
            break;
        } else {
            return None;
        }
    }

    let name_start = skip_whitespace(text, pos);
    let (first, mut end, first_key) = identifier_part(text, name_start)?;
    let mut parts = vec![(first, first_key)];
    if text[end..].starts_with('.') {
        let (second, second_end, second_key) = identifier_part(text, end + 1)?;
        parts.push((second, second_key));
        end = second_end;
    }
    let name = match parts.as_slice() {
        [(_, qualifier), (name, _)] if qualifier.eq_ignore_ascii_case(schema.trim()) => {
            name.clone()
        }
        [(qualifier, _), (name, _)] => format!("{qualifier}.{name}"),
        [(name, _)] => name.clone(),
        _ => return None,
    };

    let rest = &text[end..];
    let after_spacing = rest.trim_start_matches([' ', '\t']);
    let keyword_len = after_spacing
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(after_spacing.len());
    let keyword = &after_spacing[..keyword_len];
    let after_spacing = if keyword.eq_ignore_ascii_case("AS") || keyword.eq_ignore_ascii_case("IS")
    {
        format!(
            "{}{}",
            keyword.to_ascii_uppercase(),
            &after_spacing[keyword_len..]
        )
    } else {
        after_spacing.to_string()
    };
    let rest = if after_spacing.is_empty() || after_spacing.starts_with('\n') {
        after_spacing
    } else {
        format!(" {after_spacing}")
    };

    let mut header = String::from("CREATE");
    for word in modifiers.iter().chain(type_words.iter()) {
        header.push(' ');
        header.push_str(word);
    }
    Some(format!("{header} {name}{rest}"))
}

// ── diff ────────────────────────────────────────────────────────────────────────

/// A unified diff between two texts plus summary counts. `added` / `removed` count lines
/// of the whole change even when `unified` was cut at the output limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDiff {
    pub unified: String,
    pub added: usize,
    pub removed: usize,
    pub hunks: usize,
    /// Lines in `unified`.
    pub output_lines: usize,
    pub truncated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// The edit script as `(op, old index, new index)`; for an insert the old index is where
/// it happens and vice versa.
fn edit_script(old: &[&str], new: &[&str]) -> Vec<(Op, usize, usize)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old_mid, new_mid) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );

    let mut ops: Vec<(Op, usize, usize)> = (0..prefix).map(|i| (Op::Equal, i, i)).collect();
    let (n, m) = (old_mid.len(), new_mid.len());
    if n == 0 || m == 0 || n.saturating_mul(m) > MAX_DIFF_CELLS {
        ops.extend((0..n).map(|i| (Op::Delete, prefix + i, prefix)));
        ops.extend((0..m).map(|j| (Op::Insert, prefix + n, prefix + j)));
    } else {
        // lcs[i][j] = longest common subsequence of old_mid[i..] and new_mid[j..].
        let width = m + 1;
        let mut lcs = vec![0u32; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * width + j] = if old_mid[i] == new_mid[j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && old_mid[i] == new_mid[j] {
                ops.push((Op::Equal, prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
                ops.push((Op::Delete, prefix + i, prefix + j));
                i += 1;
            } else {
                ops.push((Op::Insert, prefix + i, prefix + j));
                j += 1;
            }
        }
    }
    let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
    ops.extend((0..suffix).map(|k| (Op::Equal, old_end + k, new_end + k)));
    ops
}

fn clip_line(line: &str) -> String {
    if line.chars().count() <= MAX_DIFF_LINE_CHARS {
        line.to_string()
    } else {
        line.chars().take(MAX_DIFF_LINE_CHARS).collect::<String>() + "…"
    }
}

fn range_header(start: usize, count: usize) -> String {
    // Unified diff convention: an empty range is numbered by the line before it.
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{count}", start + 1),
    }
}

/// Line-based unified diff of `old` → `new` (3 lines of context), or `None` when they
/// are identical. At most `max_output_lines` lines are rendered.
pub fn unified_diff(
    old: &str,
    new: &str,
    old_label: &str,
    new_label: &str,
    max_output_lines: usize,
) -> Option<TextDiff> {
    if old == new {
        return None;
    }
    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();
    let ops = edit_script(&old_lines, &new_lines);

    let added = ops.iter().filter(|(op, ..)| *op == Op::Insert).count();
    let removed = ops.iter().filter(|(op, ..)| *op == Op::Delete).count();

    let mut keep = vec![false; ops.len()];
    for (index, (op, ..)) in ops.iter().enumerate() {
        if *op != Op::Equal {
            let from = index.saturating_sub(CONTEXT_LINES);
            let to = (index + CONTEXT_LINES + 1).min(ops.len());
            keep[from..to].iter_mut().for_each(|k| *k = true);
        }
    }

    let mut out: Vec<String> = vec![format!("--- {old_label}"), format!("+++ {new_label}")];
    let mut hunks = 0usize;
    let mut index = 0usize;
    while index < ops.len() {
        if !keep[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < ops.len() && keep[index] {
            index += 1;
        }
        let hunk = &ops[start..index];
        let old_count = hunk.iter().filter(|(op, ..)| *op != Op::Insert).count();
        let new_count = hunk.iter().filter(|(op, ..)| *op != Op::Delete).count();
        let (_, old_start, new_start) = hunk[0];
        hunks += 1;
        out.push(format!(
            "@@ -{} +{} @@",
            range_header(old_start, old_count),
            range_header(new_start, new_count)
        ));
        for (op, old_index, new_index) in hunk {
            out.push(match op {
                Op::Equal => format!(" {}", clip_line(old_lines[*old_index])),
                Op::Delete => format!("-{}", clip_line(old_lines[*old_index])),
                Op::Insert => format!("+{}", clip_line(new_lines[*new_index])),
            });
        }
    }

    let truncated = out.len() > max_output_lines;
    out.truncate(max_output_lines);
    Some(TextDiff {
        output_lines: out.len(),
        unified: out.join("\n"),
        added,
        removed,
        hunks,
        truncated,
    })
}

#[cfg(test)]
#[path = "tests/ddl_compare.rs"]
mod tests;
