//! Integration tests for multi-workspace sweep operations (`--all`).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct MultiRepoEnv {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo_a: PathBuf,
    repo_b: PathBuf,
    shared_src: PathBuf,
}

impl MultiRepoEnv {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let repo_a = root.join("repo_a");
        let repo_b = root.join("repo_b");
        let shared = root.join("shared");

        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        fs::create_dir_all(&shared).unwrap();

        let shared_src = shared.join("common.txt");
        fs::write(&shared_src, "version 1 canonical\n").unwrap();

        fs::write(
            repo_a.join("catasto.ctmt"),
            "\"common.txt\": { source: \"../shared/common.txt\" }\n",
        )
        .unwrap();

        fs::write(
            repo_b.join("catasto.ctmt"),
            "\"common.txt\": { source: \"../shared/common.txt\" }\n",
        )
        .unwrap();

        Self {
            _tmp: tmp,
            root,
            repo_a,
            repo_b,
            shared_src,
        }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_catasto"))
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn sync_all_and_check_all_flow() {
    let env = MultiRepoEnv::new();

    // 1. Sync all workspaces
    let out = env.run(&["sync", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[1/2]"));
    assert!(stdout.contains("[2/2]"));
    assert!(stdout.contains("Summary: 2 succeeded, 0 failed (2 total)"));

    // Files and lockfiles should exist in both repos
    assert!(env.repo_a.join("common.txt").is_file());
    assert!(env.repo_b.join("common.txt").is_file());
    assert!(env.repo_a.join("catasto.lock").is_file());
    assert!(env.repo_b.join("catasto.lock").is_file());

    // 2. Check all (clean)
    let out = env.run(&["check", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[1/2]"));
    assert!(stdout.contains("... ok"));
    assert!(stdout.contains("[2/2]"));
    assert!(stdout.contains("Summary: 2 passed, 0 failed (2 total)"));

    // 3. Check all with --json
    let out = env.run(&["check", "--all", "--json", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 2);
    assert_eq!(json["summary"]["passed"], 2);
    assert_eq!(json["summary"]["failed"], 0);
    assert_eq!(json["workspaces"].as_array().unwrap().len(), 2);

    // 4. Diff all (clean)
    let out = env.run(&["diff", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Summary: 2 clean, 0 drifted, 0 failed (2 total)"));

    // 5. List all
    let out = env.run(&["list", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[1/2]"));
    assert!(stdout.contains("common.txt"));
    assert!(stdout.contains("[2/2]"));

    // 6. List all with --json
    let out = env.run(&["list", "--all", "--json", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 2);
    assert_eq!(json["workspaces"].as_array().unwrap().len(), 2);
}

#[test]
fn drift_detection_across_workspaces() {
    let env = MultiRepoEnv::new();

    // Sync initial state
    let out = env.run(&["sync", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));

    // Introduce drift in repo_b only
    fs::write(env.repo_b.join("common.txt"), "drifted modification\n").unwrap();

    // Check all: should fail with exit code 1
    let out = env.run(&["check", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("... ok"));
    assert!(stdout.contains("... failed (1 violation)"));
    assert!(stdout.contains("Summary: 1 passed, 1 failed (2 total)"));

    // Check all with --json: should exit code 1 and record violation
    let out = env.run(&["check", "--all", "--json", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 2);
    assert_eq!(json["summary"]["passed"], 1);
    assert_eq!(json["summary"]["failed"], 1);

    let workspaces = json["workspaces"].as_array().unwrap();
    let drifted_ws = workspaces.iter().find(|ws| ws["ok"] == false).unwrap();
    assert_eq!(drifted_ws["violations"][0]["path"], "common.txt");
    assert_eq!(drifted_ws["violations"][0]["kind"], "modified");

    // Diff all: should detect diff and exit code 1
    let out = env.run(&["diff", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("diff detected"));
    assert!(stdout.contains("Summary: 1 clean, 1 drifted, 0 failed (2 total)"));

    // Sync all: restores drifted file to match source
    let out = env.run(&["sync", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));

    // After sync, check all passes
    let out = env.run(&["check", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));

    // Now update source and test lock --all
    fs::write(&env.shared_src, "version 2 updated\n").unwrap();
    fs::write(env.repo_a.join("common.txt"), "version 2 updated\n").unwrap();
    fs::write(env.repo_b.join("common.txt"), "version 2 updated\n").unwrap();

    let out = env.run(&["lock", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Summary: 2 succeeded, 0 failed (2 total)"));

    let out = env.run(&["check", "--all", env.root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn all_covers_a_root_manifest_and_a_workspace_inside_another() {
    let env = MultiRepoEnv::new();

    // The root is a workspace of its own, and `repo_a` holds one more.
    fs::write(
        env.root.join("catasto.ctmt"),
        "\"root.txt\": { source: \"shared/common.txt\" }\n",
    )
    .unwrap();
    let inner = env.repo_a.join("inner");
    fs::create_dir_all(&inner).unwrap();
    fs::write(
        inner.join("catasto.ctmt"),
        "\"inner.txt\": { source: \"../../shared/common.txt\" }\n",
    )
    .unwrap();

    let root = env.root.to_str().unwrap();
    let out = env.run(&["sync", "--all", root]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[1/4]"));
    assert!(stdout.contains("[4/4]"));
    assert!(stdout.contains("Summary: 4 succeeded, 0 failed (4 total)"));

    // Each workspace wrote its own file and its own lock, and nothing leaked into its parent.
    assert!(env.root.join("root.txt").is_file());
    assert!(env.root.join("catasto.lock").is_file());
    assert!(env.repo_a.join("common.txt").is_file());
    assert!(inner.join("inner.txt").is_file());
    assert!(inner.join("catasto.lock").is_file());
    assert!(!env.repo_a.join("inner.txt").exists());
    let lock_a = fs::read_to_string(env.repo_a.join("catasto.lock")).unwrap();
    assert!(!lock_a.contains("inner.txt"));

    let out = env.run(&["check", "--all", "--json", root]);
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 4);
    assert_eq!(json["summary"]["passed"], 4);
}

#[test]
fn single_workspace_check_json() {
    let env = MultiRepoEnv::new();

    // Sync repo_a
    let out = env.run(&["sync", env.repo_a.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));

    // Check repo_a with --json
    let out = env.run(&["check", "--json", env.repo_a.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert!(json["violations"].as_array().unwrap().is_empty());

    // Drift repo_a
    fs::write(env.repo_a.join("common.txt"), "drift").unwrap();
    let out = env.run(&["check", "--json", env.repo_a.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["violations"][0]["kind"], "modified");
}

#[test]
fn empty_discovery_handles_gracefully() {
    let tmp = tempfile::tempdir().unwrap();
    let empty_root = tmp.path();

    // Human readable check
    let out = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .args(["check", "--all", empty_root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("catasto: no workspaces discovered under"));

    // JSON check
    let out = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .args(["check", "--all", "--json", empty_root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 0);
    assert!(json["workspaces"].as_array().unwrap().is_empty());
}
