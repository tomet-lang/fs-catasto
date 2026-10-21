//! Integration tests for `catasto`.

use std::process::Command;

#[test]
fn help_describes_catasto() {
    let output = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("catasto"), "{help}");
    assert!(help.contains("check"), "{help}");
}

#[test]
fn check_current_directory_exits_0() {
    let output = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .args(["check", "."])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
}
