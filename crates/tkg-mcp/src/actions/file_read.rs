//! `file_read` action handler with path confinement and safe truncation.

use std::{collections::BTreeMap, path::Path};

use tkg_common::ProjectConfig;

use crate::{
    guards::{
        check_dangerous_files_with_security, check_enforce_boundaries, resolve_safe_path,
        resolve_scoped_path_str, truncate_output,
    },
    models::{AgentGuard, ToolDefinition},
};

/// Executes a bounded file read action.
///
/// # Errors
/// Returns an error message if the path violates security guards, does not
/// exist, or cannot be read.
pub(crate) fn execute(
    tool: &ToolDefinition,
    workspace_root: &Path,
    arguments: &serde_json::Value,
    agent_name: Option<&str>,
    agent_guards: &BTreeMap<String, AgentGuard>,
) -> Result<String, String> {
    let raw_path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Missing required parameter 'path'".to_owned())?;

    let resolved_path = resolve_scoped_path_str(
        workspace_root,
        raw_path,
        Some(tool),
        agent_name.map(|name| (name, agent_guards)),
    );

    if tool.ban_dangerous_files() {
        let project = ProjectConfig::detect_or_load(workspace_root);
        check_dangerous_files_with_security(Path::new(&resolved_path), Some(&project.security))?;
    }

    let safe_path = resolve_safe_path(
        workspace_root,
        &resolved_path,
        &tool.allowed_directories,
        tool.enforce_workspace_bounds(),
    )?;

    check_enforce_boundaries(
        workspace_root,
        &safe_path,
        tool.enforce_dir.as_deref(),
        tool.enforce_file_target.as_deref(),
    )?;

    if safe_path.is_dir() {
        return Err(format!(
            "Cannot read '{}': path is a directory, not a regular file",
            safe_path.display()
        ));
    }

    let raw_bytes = std::fs::read(&safe_path)
        .map_err(|err| format!("Failed to read file '{}': {err}", safe_path.display()))?;

    // Gracefully handle files that cannot be decoded as valid UTF-8 text or
    // contain null bytes
    if raw_bytes.iter().take(1024).any(|&b| b == 0) {
        return Ok(format!(
            "File '{}' contains non-UTF-8 or binary data ({} bytes) and cannot be displayed as plain text.",
            safe_path.display(),
            raw_bytes.len()
        ));
    }

    let Ok(raw_content) = String::from_utf8(raw_bytes) else {
        return Ok(format!(
            "File '{}' contains non-UTF-8 or binary data and cannot be displayed as plain text.",
            safe_path.display()
        ));
    };

    let start_line = arguments
        .get("start_line")
        .and_then(serde_json::Value::as_u64);
    let end_line = arguments
        .get("end_line")
        .and_then(serde_json::Value::as_u64);

    if let (Some(start), Some(end)) = (start_line, end_line)
        && start > end
    {
        return Err(format!(
            "Invalid line range: start_line ({start}) cannot be greater than end_line ({end})"
        ));
    }

    let content_to_process = if start_line.is_some() || end_line.is_some() {
        slice_lines(&raw_content, start_line, end_line)
    } else {
        raw_content
    };

    let result = truncate_output(
        &content_to_process,
        tool.truncate_lines,
        tool.truncate_bytes,
        tool.emit_truncation_notice(),
    );

    Ok(result)
}

fn slice_lines(text: &str, start_opt: Option<u64>, end_opt: Option<u64>) -> String {
    let start_raw = start_opt.unwrap_or(1);
    let start = usize::try_from(start_raw).unwrap_or(1).saturating_sub(1);
    let end = end_opt.map_or(usize::MAX, |e| usize::try_from(e).unwrap_or(usize::MAX));

    let mut selected = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_num = idx + 1;
        if line_num > start && line_num <= end {
            selected.push(line);
        }
    }

    selected.join("\n")
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "Unit test assertions")]
mod tests {
    use super::*;

    #[test]
    fn test_slice_lines() {
        let content = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let sliced = slice_lines(content, Some(2), Some(4));
        assert_eq!(sliced, "Line 2\nLine 3\nLine 4");
    }

    #[test]
    fn test_file_read_inverted_bounds_error() {
        let temp = std::env::temp_dir().join(format!("tkg_read_bounds_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).expect("create dir");
        let file_path = temp.join("test.txt");
        std::fs::write(&file_path, "Line 1\nLine 2\n").expect("write test file");

        let tool = ToolDefinition {
            name: "file_read".to_owned(),
            ..Default::default()
        };
        let guards = BTreeMap::new();
        let args = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "start_line": 10,
            "end_line": 5
        });

        let res = execute(&tool, &temp, &args, None, &guards);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Invalid line range"));
    }

    #[test]
    fn test_file_read_scoped_path_resolution() {
        let temp = std::env::temp_dir().join(format!("tkg_read_scoped_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("src/auth")).expect("create src/auth");
        std::fs::create_dir_all(temp.join("docs/specs")).expect("create docs/specs");
        std::fs::write(temp.join("src/auth/hasher.rs"), "pub fn hash() {}").expect("write hasher");
        std::fs::write(temp.join("Cargo.toml"), "[package]\nname = \"demo\"\n")
            .expect("write Cargo.toml");
        std::fs::write(temp.join("docs/specs/rv32.md"), "# RV32 Spec").expect("write spec");

        let mut guards = BTreeMap::new();
        guards.insert(
            "worker_agent".to_owned(),
            AgentGuard {
                allowed_dirs: vec!["src".to_owned()],
                ..Default::default()
            },
        );

        let default_tool = ToolDefinition {
            name: "read_file".to_owned(),
            allowed_directories: vec!["src".to_owned()],
            ..Default::default()
        };

        // 1. Worker reads scoped relative path "auth/hasher.rs" -> resolves to
        //    "src/auth/hasher.rs"
        let args = serde_json::json!({ "path": "auth/hasher.rs" });
        let res = execute(&default_tool, &temp, &args, Some("worker_agent"), &guards)
            .expect("read auth/hasher.rs");
        assert!(res.contains("pub fn hash() {}"));

        // 2. Read root Cargo.toml (with tool allowing workspace root)
        let root_tool = ToolDefinition {
            name: "read_file".to_owned(),
            ..Default::default()
        };
        let root_args = serde_json::json!({ "path": "Cargo.toml" });
        let res_root = execute(&root_tool, &temp, &root_args, Some("worker_agent"), &guards)
            .expect("read Cargo.toml");
        assert!(res_root.contains("name = \"demo\""));

        // 3. Tool with enforce_dir "docs" reading "specs/rv32.md" -> resolves
        //    to "docs/specs/rv32.md"
        let docs_tool = ToolDefinition {
            name: "read_doc".to_owned(),
            enforce_dir: Some("docs".to_owned()),
            ..Default::default()
        };
        let doc_args = serde_json::json!({ "path": "specs/rv32.md" });
        let res_doc = execute(&docs_tool, &temp, &doc_args, None, &guards).expect("read doc");
        assert!(res_doc.contains("# RV32 Spec"));

        // 4. Reading a directory produces clean actionable error without
        //    failing with raw OS error 21
        let dir_path = temp.join("sub_dir");
        std::fs::create_dir_all(&dir_path).unwrap();
        let dir_args = serde_json::json!({ "path": "sub_dir" });
        let dir_err = execute(&root_tool, &temp, &dir_args, None, &guards).unwrap_err();
        assert!(dir_err.contains("is a directory, not a regular file"));

        // 5. Binary/non-UTF8 file returns clean notice without unhandled panic
        //    or recommending tools
        let bin_path = temp.join("output.bin");
        std::fs::write(&bin_path, [0x7f, b'E', b'L', b'F', 0x00, 0x02, 0x01, 0x00]).unwrap();
        let bin_args = serde_json::json!({ "path": "output.bin" });
        let bin_res = execute(&root_tool, &temp, &bin_args, None, &guards).expect("read binary");
        assert!(bin_res.contains(
            "contains non-UTF-8 or binary data (8 bytes) and cannot be displayed as plain text"
        ));

        _ = std::fs::remove_dir_all(&temp);
    }

    mod prop_tests {
        use proptest::prelude::*;
        use serde_json::json;

        use super::*;

        struct TempDirGuard(std::path::PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                _ = std::fs::remove_dir_all(&self.0);
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(48))]

            #[test]
            fn prop_file_read_line_slicing_and_inverted_bounds(
                lines in proptest::collection::vec("[a-zA-Z0-9 _-]{1,24}", 1..20),
                start in 1u64..25,
                end in 1u64..25,
            ) {
                let dir_id = format!(
                    "tkg_read_prop_{}_{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_nanos()
                );
                let temp = std::env::temp_dir().join(dir_id);
                std::fs::create_dir_all(&temp).expect("create temp");
                let _cleanup = TempDirGuard(temp.clone());

                let body = lines.join("\n");
                let file_path = temp.join("sample.txt");
                std::fs::write(&file_path, &body).expect("write sample");

                let tool = ToolDefinition {
                    name: "file_read".to_owned(),
                    ..Default::default()
                };
                let guards = BTreeMap::new();
                let args = json!({
                    "path": "sample.txt",
                    "start_line": start,
                    "end_line": end,
                });

                let res = execute(&tool, &temp, &args, None, &guards);
                if start > end {
                    let err = res.expect_err("inverted bounds must error");
                    prop_assert!(err.contains("Invalid line range"));
                } else {
                    let out = res.expect("valid range must succeed");
                    let len = lines.len();
                    let lo = usize::try_from(start).unwrap_or(1).saturating_sub(1).min(len);
                    let hi = usize::try_from(end).unwrap_or(len).min(len);
                    let expected = if lo < hi {
                        lines[lo..hi].join("\n")
                    } else {
                        String::new()
                    };
                    prop_assert_eq!(out, expected);
                }
            }

            #[test]
            fn prop_file_read_rejects_arbitrary_paths_outside_allowed_dir(
                dir_prefix in prop_oneof![
                    Just("src_extra".to_owned()),
                    Just("src2".to_owned()),
                    Just("tests".to_owned()),
                    Just("..".to_owned()),
                    Just("src/../tests".to_owned()),
                    "[a-rt-z]{1,6}[0-9]{1,3}",
                ],
                file_name in "[a-zA-Z0-9_]{1,12}\\.rs",
                use_abs in proptest::bool::ANY,
            ) {
                let dir_id = format!(
                    "tkg_read_confine_{}_{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_nanos()
                );
                let temp = std::env::temp_dir().join(dir_id);
                std::fs::create_dir_all(&temp).expect("create temp");
                let _cleanup = TempDirGuard(temp.clone());

                let tool = ToolDefinition {
                    name: "read_file".to_owned(),
                    allowed_directories: vec!["src".to_owned()],
                    sandbox: crate::models::ToolSandboxPolicy {
                        enforce_workspace_bounds: true,
                        ban_dangerous_files: true,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let guards = BTreeMap::new();
                let target = if use_abs {
                    format!("/{dir_prefix}/{file_name}")
                } else {
                    if !dir_prefix.contains("..") {
                        let outside_file = temp.join(&dir_prefix).join(&file_name);
                        std::fs::create_dir_all(outside_file.parent().expect("parent"))
                            .expect("create outside dir");
                        std::fs::write(&outside_file, "SECRET_OUTSIDE_SRC")
                            .expect("seed outside file");
                    }
                    format!("{dir_prefix}/{file_name}")
                };
                let args = json!({ "path": target });
                let res = execute(&tool, &temp, &args, None, &guards);
                prop_assert!(res.is_err());
            }
        }
    }
}
