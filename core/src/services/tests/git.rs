use std::path::Path;
use std::process::Command;

use super::*;

#[test]
fn porcelain_output_maps_to_statuses() {
    assert_eq!(parse_porcelain_status(""), FileStatus::Clean);
    assert_eq!(
        parse_porcelain_status("?? code/a.pck\n"),
        FileStatus::Untracked
    );
    assert_eq!(
        parse_porcelain_status("!! code/a.pck\n"),
        FileStatus::Ignored
    );
    assert_eq!(
        parse_porcelain_status(" M code/a.pck\n"),
        FileStatus::Modified
    );
    assert_eq!(
        parse_porcelain_status("MM code/a.pck\n"),
        FileStatus::Modified
    );
    assert_eq!(
        parse_porcelain_status("M  code/a.pck\n"),
        FileStatus::Staged
    );
    assert_eq!(
        parse_porcelain_status("A  code/a.pck\n"),
        FileStatus::Staged
    );
    assert_eq!(
        parse_porcelain_status("D  code/a.pck\n"),
        FileStatus::Staged
    );
    assert_eq!(
        parse_porcelain_status(" D code/a.pck\n"),
        FileStatus::Modified
    );
    assert_eq!(
        parse_porcelain_status("UU code/a.pck\n"),
        FileStatus::Conflicted
    );
    assert_eq!(
        parse_porcelain_status("AA code/a.pck\n"),
        FileStatus::Conflicted
    );
}

#[test]
fn only_clean_and_unknown_files_have_no_uncommitted_changes() {
    assert!(!FileStatus::Clean.has_uncommitted_changes());
    assert!(!FileStatus::Unknown.has_uncommitted_changes());
    for status in [
        FileStatus::Modified,
        FileStatus::Staged,
        FileStatus::Untracked,
        FileStatus::Ignored,
        FileStatus::Conflicted,
    ] {
        assert!(status.has_uncommitted_changes(), "{status:?}");
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_temp_folder_outside_any_repo_is_unknown() {
    if !git_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // The system temp folder could itself sit inside a repository; then there is nothing to check.
    if repo_status(dir.path()).is_known() {
        return;
    }
    let file = dir.path().join("a.sql");
    std::fs::write(&file, "x").unwrap();
    assert!(matches!(repo_status(dir.path()), RepoStatus::Unknown(_)));
    assert_eq!(file_status(&file), FileStatus::Unknown);
}

#[test]
fn statuses_follow_a_real_repository() {
    if !git_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "-q"]);
    std::fs::write(root.join(".gitignore"), "ignored.sql\n").unwrap();
    std::fs::write(root.join("clean.sql"), "one\n").unwrap();
    std::fs::write(root.join("edited.sql"), "one\n").unwrap();
    std::fs::write(root.join("staged.sql"), "one\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "init"]);

    std::fs::write(root.join("edited.sql"), "two\n").unwrap();
    std::fs::write(root.join("staged.sql"), "two\n").unwrap();
    run_git(root, &["add", "staged.sql"]);
    std::fs::write(root.join("fresh.sql"), "new\n").unwrap();
    std::fs::write(root.join("ignored.sql"), "new\n").unwrap();

    assert_eq!(file_status(&root.join("clean.sql")), FileStatus::Clean);
    assert_eq!(file_status(&root.join("edited.sql")), FileStatus::Modified);
    assert_eq!(file_status(&root.join("staged.sql")), FileStatus::Staged);
    assert_eq!(file_status(&root.join("fresh.sql")), FileStatus::Untracked);
    assert_eq!(file_status(&root.join("ignored.sql")), FileStatus::Ignored);

    match repo_status(root) {
        RepoStatus::Repo { branch, .. } => assert!(branch.is_some()),
        other => panic!("expected a repository, got {other:?}"),
    }
    // A folder that is not created yet is judged by its nearest existing parent.
    assert!(repo_status(&root.join("not").join("yet")).is_known());
}
