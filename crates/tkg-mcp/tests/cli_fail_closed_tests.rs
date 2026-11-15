//! CLI fail-closed configuration integration tests for `tkg-mcp`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::tests_outside_test_module,
    reason = "Integration test assertions"
)]

use std::process::{Command, Stdio};

#[test]
fn test_tkg_mcp_aborts_on_corrupt_tools_toml_with_line_diagnostics() {
    let temp_dir = std::env::temp_dir().join(format!(
        "tkg_mcp_corrupt_cfg_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    let corrupt_cfg = temp_dir.join("tools.toml");
    std::fs::write(
        &corrupt_cfg,
        "[tools.demo]\ndescription = \"ok\"\ncorrupt = [[\n",
    )
    .expect("write corrupt tools.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_tkg-mcp"))
        .arg("--workspace")
        .arg(&temp_dir)
        .arg("--config")
        .arg(&corrupt_cfg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn tkg-mcp");

    assert!(
        !output.status.success(),
        "tkg-mcp must abort with non-zero exit code on corrupt tools.toml"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("at line 3, column"),
        "expected line/col diagnostic in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("3 | corrupt = [["),
        "expected source snippet in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains('^'),
        "expected caret pointer in stderr, got: {stderr}"
    );

    drop(std::fs::remove_dir_all(&temp_dir));
}
