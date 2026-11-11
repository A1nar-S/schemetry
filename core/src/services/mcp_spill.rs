//! Large MCP results go to files instead of into the agent's context.
//!
//! A tool whose result is over [`INLINE_LIMIT_BYTES`] (or that was asked for
//! `output: "file"`) writes it here and returns a small handle: path, size, line count,
//! sha256 and a preview. The agent then greps or reads parts of the file, either with its
//! own file tools or through the audited `grep_result` / `read_result` MCP tools.
//!
//! Files live in a per-process folder under the OS temp dir, expire after [`TTL`], and are
//! deleted when the process exits. Folders left behind by crashed processes are removed
//! the next time a store is created.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::services::mcp_gateway::sha256_hex;
use crate::services::mcp_policy::McpTool;

/// Results larger than this go to a file when the agent did not choose.
pub const INLINE_LIMIT_BYTES: usize = 16 * 1024;
/// Upper bound for one result file.
pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
/// How long a result file can be read.
pub const TTL: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_FILES: usize = 200;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Folders of other processes older than this are treated as left behind by a crash.
const STALE_DIR_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const PREVIEW_LINES: usize = 40;
const PREVIEW_BYTES: usize = 4 * 1024;
const MAX_OUTLINE_ITEMS: usize = 500;
/// Lines echoed by read/grep are cut at this many characters.
pub const MAX_LINE_CHARS: usize = 2_000;
/// Text returned by one read/grep call.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_PATTERN_CHARS: usize = 1_000;

/// How a tool hands its result back.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Inline when small, a file when over 16 KB.
    #[default]
    Auto,
    /// Always inline, within the tool's own size cap.
    Inline,
    /// Always a file.
    File,
}

impl OutputMode {
    /// Whether a result of `len` bytes goes to a file.
    pub fn spills(self, len: usize) -> bool {
        match self {
            OutputMode::Auto => len > INLINE_LIMIT_BYTES,
            OutputMode::Inline => false,
            OutputMode::File => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpillFormat {
    Sql,
    Diff,
    Jsonl,
    /// One LOB cell, typed by its content.
    Lob(LobType),
}

impl SpillFormat {
    /// Also the file extension.
    pub fn name(self) -> &'static str {
        match self {
            SpillFormat::Sql => "sql",
            SpillFormat::Diff => "diff",
            SpillFormat::Jsonl => "jsonl",
            SpillFormat::Lob(ty) => ty.ext,
        }
    }

    /// Binary files can't be grepped or read as lines through MCP.
    pub fn binary(self) -> bool {
        matches!(self, SpillFormat::Lob(ty) if ty.binary)
    }

    pub fn mime(self) -> &'static str {
        match self {
            SpillFormat::Sql => "application/sql",
            SpillFormat::Diff => "text/x-diff",
            SpillFormat::Jsonl => "application/jsonl",
            SpillFormat::Lob(ty) => ty.mime,
        }
    }
}

/// What a LOB's bytes look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LobType {
    pub ext: &'static str,
    pub mime: &'static str,
    pub binary: bool,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Guesses a LOB's type from its first bytes, so the file gets a useful extension.
pub fn sniff(bytes: &[u8]) -> LobType {
    let ty = |ext, mime, binary| LobType { ext, mime, binary };
    if bytes.starts_with(b"%PDF-") {
        return ty("pdf", "application/pdf", true);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return ty("png", "image/png", true);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return ty("jpg", "image/jpeg", true);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return ty("gif", "image/gif", true);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return ty("webp", "image/webp", true);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let head = &bytes[..bytes.len().min(1 << 20)];
        return if contains(head, b"word/") {
            ty(
                "docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                true,
            )
        } else if contains(head, b"xl/") {
            ty(
                "xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                true,
            )
        } else if contains(head, b"ppt/") {
            ty(
                "pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                true,
            )
        } else {
            ty("zip", "application/zip", true)
        };
    }
    if bytes.starts_with(&[0x1F, 0x8B]) {
        return ty("gz", "application/gzip", true);
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ty("bin", "application/octet-stream", true);
    };
    if text.contains('\0') {
        return ty("bin", "application/octet-stream", true);
    }
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let lower = head
        .chars()
        .take(1_024)
        .collect::<String>()
        .to_ascii_lowercase();
    if lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg")) {
        return ty("svg", "image/svg+xml", false);
    }
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return ty("html", "text/html", false);
    }
    if lower.starts_with('<') {
        return ty("xml", "application/xml", false);
    }
    if (head.starts_with('{') || head.starts_with('['))
        && serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok()
    {
        return ty("json", "application/json", false);
    }
    ty("txt", "text/plain", false)
}

/// What a result file is about; used for its name, for policy rechecks and for audit.
pub struct SpillMeta<'a> {
    /// Connections whose policy produced the result; the first is the primary one.
    pub connection_ids: Vec<i64>,
    pub tool: McpTool,
    pub format: SpillFormat,
    /// Optional object/table name for the file name.
    pub label: Option<&'a str>,
    /// The producing call's audit target, repeated on later reads.
    pub target: Option<Map<String, Value>>,
}

#[derive(Clone, Debug)]
pub struct SpillEntry {
    pub id: String,
    pub path: PathBuf,
    pub connection_ids: Vec<i64>,
    pub tool: McpTool,
    pub format: SpillFormat,
    pub bytes: u64,
    pub lines: usize,
    pub sha256: String,
    pub target: Option<Map<String, Value>>,
    created: Instant,
}

impl SpillEntry {
    fn expires_in(&self, ttl: Duration) -> Duration {
        ttl.saturating_sub(self.created.elapsed())
    }

    /// The `file` object handed to the agent.
    pub fn handle(&self, ttl: Duration) -> Value {
        json!({
            "id": self.id,
            "path": self.path.to_string_lossy(),
            "format": self.format.name(),
            "mime": self.format.mime(),
            "binary": self.format.binary(),
            "bytes": self.bytes,
            "lines": self.lines,
            "sha256": self.sha256,
            "expires_in_secs": self.expires_in(ttl).as_secs(),
        })
    }

    /// What the audit entry records about the file: never its content.
    pub fn audit_note(&self) -> Value {
        json!({
            "id": self.id,
            "format": self.format.name(),
            "bytes": self.bytes,
            "lines": self.lines,
            "sha256": self.sha256,
        })
    }
}

struct State {
    entries: HashMap<String, SpillEntry>,
    dir_ready: bool,
}

pub struct SpillStore {
    parent: PathBuf,
    dir: PathBuf,
    ttl: Duration,
    max_files: usize,
    max_total_bytes: u64,
    counter: AtomicU64,
    state: Mutex<State>,
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

impl SpillStore {
    /// A store under `<os temp>/schemetry-mcp/`.
    pub fn in_temp_dir() -> Self {
        Self::under(std::env::temp_dir().join("schemetry-mcp"))
    }

    /// A store whose per-process folder is created inside `parent` on first use.
    pub fn under(parent: PathBuf) -> Self {
        let nonce = sha256_hex(format!("{}:{}", std::process::id(), now_nanos()).as_bytes());
        let dir = parent.join(format!("{}-{}", std::process::id(), &nonce[..8]));
        Self {
            parent,
            dir,
            ttl: TTL,
            max_files: MAX_FILES,
            max_total_bytes: MAX_TOTAL_BYTES,
            counter: AtomicU64::new(0),
            state: Mutex::new(State {
                entries: HashMap::new(),
                dir_ready: false,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, ttl: Duration, max_files: usize, max_total: u64) -> Self {
        self.ttl = ttl;
        self.max_files = max_files;
        self.max_total_bytes = max_total;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    fn ensure_dir(&self, state: &mut State) -> Result<(), String> {
        if state.dir_ready && self.dir.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(&self.parent)
            .map_err(|e| format!("Could not create the result folder: {e}"))?;
        sweep_stale_dirs(&self.parent, &self.dir, STALE_DIR_AGE);
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&self.dir)
            .map_err(|e| format!("Could not create the result folder: {e}"))?;
        state.dir_ready = true;
        Ok(())
    }

    fn remove(state: &mut State, id: &str) {
        if let Some(entry) = state.entries.remove(id) {
            let _ = fs::remove_file(&entry.path);
        }
    }

    /// Drops expired files, then the oldest ones until `incoming` more bytes fit.
    fn make_room(&self, state: &mut State, incoming: u64) {
        let expired: Vec<String> = state
            .entries
            .values()
            .filter(|e| e.created.elapsed() >= self.ttl)
            .map(|e| e.id.clone())
            .collect();
        for id in expired {
            Self::remove(state, &id);
        }
        loop {
            let total: u64 = state.entries.values().map(|e| e.bytes).sum();
            let full = state.entries.len() >= self.max_files
                || (!state.entries.is_empty() && total + incoming > self.max_total_bytes);
            if !full {
                break;
            }
            let Some(oldest) = state
                .entries
                .values()
                .min_by_key(|e| e.created)
                .map(|e| e.id.clone())
            else {
                break;
            };
            Self::remove(state, &oldest);
        }
    }

    fn next_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let seed = format!(
            "{}:{n}:{}:{}",
            std::process::id(),
            now_nanos(),
            self.dir.display()
        );
        format!("r_{}", &sha256_hex(seed.as_bytes())[..16])
    }

    /// Writes one result file and registers it.
    pub fn write(&self, meta: SpillMeta<'_>, bytes: &[u8]) -> Result<SpillEntry, String> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "The result is {} bytes, more than the {} MB file limit.",
                bytes.len(),
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.ensure_dir(&mut state)?;
        self.make_room(&mut state, bytes.len() as u64);

        let id = self.next_id();
        let mut name = format!("{id}-{}", meta.tool.name());
        if let Some(label) = meta.label.map(file_label).filter(|l| !l.is_empty()) {
            name.push('-');
            name.push_str(&label);
        }
        name.push('.');
        name.push_str(meta.format.name());
        let path = self.dir.join(name);
        let temp = self.dir.join(format!("{id}.tmp"));

        let written = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(bytes)?;
            file.flush()?;
            drop(file);
            fs::rename(&temp, &path)
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(format!("Could not write the result file: {error}"));
        }

        let entry = SpillEntry {
            id: id.clone(),
            path,
            connection_ids: meta.connection_ids,
            tool: meta.tool,
            format: meta.format,
            bytes: bytes.len() as u64,
            lines: line_count(bytes),
            sha256: sha256_hex(bytes),
            target: meta.target,
            created: Instant::now(),
        };
        state.entries.insert(id, entry.clone());
        Ok(entry)
    }

    /// The file behind `id`, unless it is unknown or expired.
    pub fn get(&self, id: &str) -> Option<SpillEntry> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get(id)?.clone();
        if entry.created.elapsed() >= self.ttl {
            Self::remove(&mut state, id);
            return None;
        }
        Some(entry)
    }
}

impl Drop for SpillStore {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Removes sibling folders (other processes' stores) older than `max_age`.
pub(crate) fn sweep_stale_dirs(parent: &Path, keep: &Path, max_age: Duration) {
    let Ok(children) = fs::read_dir(parent) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        if path == keep || !child.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let old = child
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= max_age);
        if old {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn file_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect()
}

pub fn line_count(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    match bytes.last() {
        None => 0,
        Some(b'\n') => newlines,
        Some(_) => newlines + 1,
    }
}

fn clip_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        line.to_string()
    } else {
        line.chars().take(MAX_LINE_CHARS).collect::<String>() + "…"
    }
}

/// The first lines of a result, at most 40 lines / 4 KB.
pub fn preview(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines().take(PREVIEW_LINES) {
        let line = clip_line(line);
        if out.len() + line.len() + 1 > PREVIEW_BYTES {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutlineItem {
    pub line: usize,
    pub kind: String,
    pub name: String,
}

fn ascii_regex(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .case_insensitive(true)
        .build()
        .expect("valid regex")
}

const NAME: &str = r#"((?:"[^"]+"|[A-Za-z0-9_$#]+)(?:\.(?:"[^"]+"|[A-Za-z0-9_$#]+))?)"#;

/// Where the statements and subprograms of a SQL result start, so an agent can jump
/// straight to a line. Returns the items and whether the list was cut short.
pub fn outline_sql(text: &str) -> (Vec<OutlineItem>, bool) {
    let create = ascii_regex(&format!(
        r"^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:NON)?EDITIONABLE\s+)?(?:(?:GLOBAL\s+)?TEMPORARY\s+|UNIQUE\s+|MATERIALIZED\s+)?(PACKAGE\s+BODY|TYPE\s+BODY|PACKAGE|PROCEDURE|FUNCTION|TRIGGER|TYPE|VIEW|TABLE|INDEX|SEQUENCE|SYNONYM)\s+(?:IF\s+NOT\s+EXISTS\s+)?{NAME}"
    ));
    let alter = ascii_regex(&format!(r"^\s*ALTER\s+(TABLE)\s+{NAME}"));
    let subprogram = ascii_regex(&format!(r"^\s*(PROCEDURE|FUNCTION)\s+{NAME}"));
    let section = ascii_regex(r"^\s*--\s*(target data source|table):\s*(\S.*?)\s*$");

    let mut items = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let bytes = line.as_bytes();
        let found = create
            .captures(bytes)
            .map(|c| (c, false))
            .or_else(|| alter.captures(bytes).map(|c| (c, true)))
            .or_else(|| subprogram.captures(bytes).map(|c| (c, false)))
            .or_else(|| section.captures(bytes).map(|c| (c, false)));
        let Some((caps, is_alter)) = found else {
            continue;
        };
        if items.len() >= MAX_OUTLINE_ITEMS {
            return (items, true);
        }
        let kind = String::from_utf8_lossy(&caps[1])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase();
        let kind = if is_alter {
            format!("ALTER {kind}")
        } else {
            kind
        };
        items.push(OutlineItem {
            line: index + 1,
            kind,
            name: clip_line(&String::from_utf8_lossy(&caps[2])),
        });
    }
    (items, false)
}

/// A grep matcher for agent input: literal unless `regex`, ASCII case folding, and size
/// limits so a pattern can't use much memory. Matching is on bytes, so it never fails on
/// odd encodings.
pub fn build_matcher(pattern: &str, regex: bool, ignore_case: bool) -> Result<Regex, String> {
    if pattern.is_empty() {
        return Err("pattern must not be empty.".into());
    }
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(format!(
            "pattern must be at most {MAX_PATTERN_CHARS} characters."
        ));
    }
    // Unicode mode is off (the build has no Unicode tables), so non-ASCII characters are
    // matched as their UTF-8 bytes.
    let mut source = String::new();
    for c in pattern.chars() {
        if c.is_ascii() {
            if regex {
                source.push(c);
            } else {
                source.push_str(&regex::escape(c.encode_utf8(&mut [0; 4])));
            }
        } else {
            for b in c.encode_utf8(&mut [0; 4]).bytes() {
                source.push_str(&format!(r"\x{b:02X}"));
            }
        }
    }
    RegexBuilder::new(&source)
        .unicode(false)
        .case_insensitive(ignore_case)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| format!("pattern is not a valid regular expression: {e}"))
}

fn open_lines(path: &Path) -> Result<impl Iterator<Item = std::io::Result<String>>, String> {
    let file = fs::File::open(path).map_err(|e| {
        format!("The result file could not be opened (it may have been deleted): {e}")
    })?;
    let mut reader = BufReader::new(file);
    Ok(std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                Some(Ok(String::from_utf8_lossy(&buf).into_owned()))
            }
            Err(e) => Some(Err(e)),
        }
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReadOut {
    /// `<line number>: <text>` per line.
    pub text: String,
    pub start_line: usize,
    /// Last line returned; `start_line - 1` when none was.
    pub end_line: usize,
    pub returned: usize,
    pub more: bool,
}

/// Lines `start..start+max` (1-based), streamed so the whole file is never loaded.
pub fn read_lines(path: &Path, start: usize, max: usize, total: usize) -> Result<ReadOut, String> {
    let start = start.max(1);
    let mut text = String::new();
    let mut end = start - 1;
    let mut returned = 0;
    let mut cut = false;
    for (index, line) in open_lines(path)?.enumerate() {
        let number = index + 1;
        if number < start {
            continue;
        }
        if returned >= max {
            break;
        }
        let line = line.map_err(|e| format!("The result file could not be read: {e}"))?;
        let row = format!("{number}: {}\n", clip_line(&line));
        if !text.is_empty() && text.len() + row.len() > MAX_RESPONSE_BYTES {
            cut = true;
            break;
        }
        text.push_str(&row);
        end = number;
        returned += 1;
    }
    Ok(ReadOut {
        text,
        start_line: start,
        end_line: end,
        returned,
        more: cut || end < total,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct GrepOut {
    /// Matching lines as `<n>:<text>`, context lines as `<n>-<text>`, groups split by `--`.
    pub hunks: String,
    /// Every matching line in the file, including ones not returned.
    pub match_count: usize,
    pub returned: usize,
    pub truncated: bool,
}

/// `grep -n -C <context>` over a result file, streamed.
pub fn grep(
    path: &Path,
    matcher: &Regex,
    context: usize,
    max_matches: usize,
) -> Result<GrepOut, String> {
    let mut out = String::new();
    let mut before: VecDeque<(usize, String)> = VecDeque::new();
    let mut after_left = 0usize;
    let mut last_printed = 0usize;
    let mut match_count = 0usize;
    let mut returned = 0usize;
    let mut truncated = false;
    let mut writing = true;

    /// Appends one line (and a `--` before a gap); false once the response is full.
    fn emit(out: &mut String, number: usize, sep: char, text: &str, last: &mut usize) -> bool {
        let gap = if *last != 0 && number > *last + 1 {
            "--\n"
        } else {
            ""
        };
        let row = format!("{gap}{number}{sep}{}\n", clip_line(text));
        if out.len() + row.len() > MAX_RESPONSE_BYTES {
            return false;
        }
        out.push_str(&row);
        *last = number;
        true
    }

    for (index, line) in open_lines(path)?.enumerate() {
        let number = index + 1;
        let line = line.map_err(|e| format!("The result file could not be read: {e}"))?;
        let hit = matcher.is_match(line.as_bytes());
        if hit {
            match_count += 1;
        }
        if !writing {
            continue;
        }
        if hit {
            if returned >= max_matches {
                truncated = true;
                writing = false;
                continue;
            }
            for (n, text) in before.drain(..) {
                if n > last_printed && !emit(&mut out, n, '-', &text, &mut last_printed) {
                    truncated = true;
                    writing = false;
                    break;
                }
            }
            if !writing {
                continue;
            }
            if !emit(&mut out, number, ':', &line, &mut last_printed) {
                truncated = true;
                writing = false;
                continue;
            }
            returned += 1;
            after_left = context;
        } else if after_left > 0 {
            after_left -= 1;
            if !emit(&mut out, number, '-', &line, &mut last_printed) {
                truncated = true;
                writing = false;
            }
        } else if context > 0 {
            if before.len() == context {
                before.pop_front();
            }
            before.push_back((number, line));
        }
    }
    Ok(GrepOut {
        hunks: out,
        match_count,
        returned,
        truncated,
    })
}

#[cfg(test)]
#[path = "tests/mcp_spill.rs"]
mod tests;
