//! Thin wrappers around the `git` CLI, used to avoid clobbering uncommitted work.
//!
//! Everything degrades to "unknown" (never an error) when git is missing, the path is not
//! inside a repository, or git itself fails — callers decide what unknown means.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What is known about the repository a folder lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoStatus {
    /// Git is unavailable or the folder is not inside a work tree.
    Unknown(String),
    Repo {
        root: PathBuf,
        /// `None` for a detached HEAD.
        branch: Option<String>,
    },
}

impl RepoStatus {
    pub fn is_known(&self) -> bool {
        matches!(self, RepoStatus::Repo { .. })
    }
}

/// The git state of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// Git could not tell (no git, not a repository, command failure).
    Unknown,
    /// Tracked and identical to HEAD.
    Clean,
    /// Tracked with unstaged changes (possibly also staged).
    Modified,
    /// Tracked with staged, uncommitted changes only.
    Staged,
    Untracked,
    /// Ignored by git, so history does not hold it either.
    Ignored,
    /// Unresolved merge conflict.
    Conflicted,
}

impl FileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FileStatus::Unknown => "unknown",
            FileStatus::Clean => "clean",
            FileStatus::Modified => "modified",
            FileStatus::Staged => "staged",
            FileStatus::Untracked => "untracked",
            FileStatus::Ignored => "ignored",
            FileStatus::Conflicted => "conflicted",
        }
    }

    /// The current content exists nowhere in git history, so overwriting it loses data.
    pub fn has_uncommitted_changes(self) -> bool {
        !matches!(self, FileStatus::Unknown | FileStatus::Clean)
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command
        .output()
        .map_err(|error| format!("git is not available: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The nearest folder at or above `dir` that exists, so a not-yet-created target can be asked about.
fn existing_ancestor(dir: &Path) -> Option<&Path> {
    dir.ancestors().find(|ancestor| ancestor.is_dir())
}

/// Repository root and current branch for `dir`.
pub fn repo_status(dir: &Path) -> RepoStatus {
    let Some(dir) = existing_ancestor(dir) else {
        return RepoStatus::Unknown("folder does not exist".to_string());
    };
    let root = match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(out) if !out.trim().is_empty() => PathBuf::from(out.trim()),
        Ok(_) => return RepoStatus::Unknown("not a git repository".to_string()),
        Err(error) => return RepoStatus::Unknown(error),
    };
    let branch = git(dir, &["branch", "--show-current"])
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|name| !name.is_empty());
    RepoStatus::Repo { root, branch }
}

/// Maps `git status --porcelain` output for a single path to a [`FileStatus`].
pub fn parse_porcelain_status(output: &str) -> FileStatus {
    for line in output.lines() {
        let mut chars = line.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        return match (x, y) {
            ('?', '?') => FileStatus::Untracked,
            ('!', '!') => FileStatus::Ignored,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => FileStatus::Conflicted,
            (_, ' ') => FileStatus::Staged,
            _ => FileStatus::Modified,
        };
    }
    FileStatus::Clean
}

/// Git status of `path`; [`FileStatus::Unknown`] when it cannot be determined.
pub fn file_status(path: &Path) -> FileStatus {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return FileStatus::Unknown;
    };
    let Some(name) = name.to_str() else {
        return FileStatus::Unknown;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    match git(dir, &["status", "--porcelain", "--ignored", "--", name]) {
        Ok(out) => parse_porcelain_status(&out),
        Err(_) => FileStatus::Unknown,
    }
}

#[cfg(test)]
#[path = "tests/git.rs"]
mod tests;
