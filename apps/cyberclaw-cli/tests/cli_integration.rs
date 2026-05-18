//! CLI 集成测试

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("cyberclaw"));
}

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.arg("--version");
    cmd.assert().success();
}

#[test]
fn test_status_command() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.arg("status");
    cmd.assert().success();
}

#[test]
fn test_package_list_command() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["package", "list"]);
    cmd.assert().success();
}

#[test]
fn test_package_list_json() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["package", "list", "--format", "json"]);
    cmd.assert().success();
}

#[test]
fn test_agent_list_command() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["agent", "list"]);
    cmd.assert().success();
}

#[test]
fn test_agent_list_json() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["agent", "list", "--format", "json"]);
    cmd.assert().success();
}

// `connector list` hits a JWT-protected endpoint; the test harness has no
// stored credential, so this test fails 401 in CI. The same functional surface
// is exercised by `scripts/testing/smoke-business.sh` (HTTP smoke with login).
// Re-enable once we have a test helper that injects a JWT into the CLI keystore.
#[test]
#[ignore = "requires authenticated CLI session — covered by smoke-business.sh"]
fn test_connector_list_command() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["connector", "list"]);
    cmd.assert().success();
}

#[test]
fn test_task_list_command() {
    let mut cmd = Command::cargo_bin("cyberclaw-cli").unwrap();
    cmd.args(["task", "list"]);
    cmd.assert().success();
}
