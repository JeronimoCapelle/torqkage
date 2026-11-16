//! Manifest mutation guard blocking unauthorized build manifest modifications.
//!
//! Neutralizes the host IDE procedural-macro / `build.rs` execution sandbox
//! escape vector.

use std::path::Path;

pub(crate) use tkg_common::manifest_guard::{
    extract_candidate_paths, is_default_fallback_mutation_tool, is_manifest_protected,
};
use tkg_common::{PreToolResponse, PreToolUseInput};

use crate::config::HooksConfig;

/// Evaluates whether a tool invocation attempts to modify a protected build
/// manifest.
///
/// If a protected manifest (e.g. `Cargo.toml`, `build.rs`, `CMakeLists.txt`) is
/// targeted, returns a `PreToolResponse::ask` to require explicit operator
/// confirmation before modification.
#[must_use]
pub(crate) fn evaluate_manifest_guard(
    input: &PreToolUseInput,
    root: &Path,
    config: &HooksConfig,
) -> Option<PreToolResponse> {
    if !config.manifest_guard.enabled {
        return None;
    }

    let tc = input.tool_call.as_ref()?;
    let tool_name = tc.name.as_str();

    let is_mutation_tool = if config.manifest_guard.protected_tools.is_empty() {
        crate::policy_gate::is_mutating_tool(tool_name, root)
            || is_default_fallback_mutation_tool(tool_name)
    } else {
        config
            .manifest_guard
            .protected_tools
            .iter()
            .any(|t| t == tool_name)
    };
    if !is_mutation_tool {
        return None;
    }

    let candidates = extract_candidate_paths(&tc.args);
    for target_path in &candidates {
        if is_manifest_protected(target_path, &config.manifest_guard.protected_patterns) {
            return Some(PreToolResponse::ask(format!(
                "🛑 MANIFEST MUTATION GUARD: Tool '{tool_name}' attempted to modify protected build manifest '{target_path}'.\n\
                 Modifying build manifests can trigger un-sandboxed host IDE proc-macro or build-script execution.\n\
                 Confirm manual override to allow modifying this manifest?"
            )));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tkg_common::{HookDecision, ToolCallPayload};

    use super::*;

    #[test]
    fn test_manifest_guard_blocks_cargo_toml() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "file_write".to_owned(),
                args: json!({
                    "path": "Cargo.toml",
                    "content": "[dependencies]\nevil-proc-macro = \"1.0\""
                }),
            }),
            ..Default::default()
        };

        let config = HooksConfig::default();
        let resp = evaluate_manifest_guard(&input, Path::new("."), &config);
        assert!(resp.is_some());
        let res = resp.unwrap();
        assert_eq!(res.decision, HookDecision::Ask);
        assert!(res.reason.unwrap().contains("MANIFEST MUTATION GUARD"));
    }

    #[test]
    fn test_manifest_guard_allows_regular_source() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "file_write".to_owned(),
                args: json!({
                    "path": "src/main.rs",
                    "content": "fn main() {}"
                }),
            }),
            ..Default::default()
        };

        let config = HooksConfig::default();
        let resp = evaluate_manifest_guard(&input, Path::new("."), &config);
        assert!(resp.is_none());
    }

    #[test]
    fn test_manifest_guard_allows_read_only_tool() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "file_read".to_owned(),
                args: json!({
                    "path": "Cargo.toml"
                }),
            }),
            ..Default::default()
        };

        let config = HooksConfig::default();
        let resp = evaluate_manifest_guard(&input, Path::new("."), &config);
        assert!(resp.is_none());
    }

    #[test]
    fn test_manifest_guard_disabled() {
        let input = PreToolUseInput {
            tool_call: Some(ToolCallPayload {
                name: "file_write".to_owned(),
                args: json!({
                    "path": "Cargo.toml"
                }),
            }),
            ..Default::default()
        };

        let mut config = HooksConfig::default();
        config.manifest_guard.enabled = false;
        let resp = evaluate_manifest_guard(&input, Path::new("."), &config);
        assert!(resp.is_none());
    }

    mod prop_tests {
        use proptest::prelude::*;

        use super::*;

        proptest! {
        #[test]
        fn prop_fallback_mutators_targeting_protected_manifests_always_ask(
            tool_prefix in prop_oneof![
                Just("write"),
                Just("edit"),
                Just("modify"),
                Just("patch"),
                Just("create"),
                Just("replace"),
            ],
            tool_suffix in "[a-z0-9_]{0,12}",
            manifest_idx in 0usize..tkg_common::DEFAULT_PROTECTED_MANIFESTS.len(),
            subdir in prop::option::of("[a-z]{2,8}"),
        ) {
            let tool_name = if tool_suffix.is_empty() {
                tool_prefix.to_owned()
            } else {
                format!("{tool_prefix}_{tool_suffix}")
            };
            let manifest = tkg_common::DEFAULT_PROTECTED_MANIFESTS
                .get(manifest_idx)
                .copied()
                .unwrap_or("Cargo.toml");
            let target = subdir.map_or_else(
                || manifest.to_owned(),
                |dir| format!("{dir}/{manifest}"),
            );
            let input = PreToolUseInput {
                tool_call: Some(ToolCallPayload {
                    name: tool_name,
                    args: json!({ "nested": { "file_path": target } }),
                }),
                ..Default::default()
            };
            let config = HooksConfig::default();
            let resp = evaluate_manifest_guard(&input, Path::new("."), &config);
            prop_assert!(resp.is_some());
            if let Some(r) = resp {
                prop_assert_eq!(r.decision, HookDecision::Ask);
            }
        }
        }
    }
}
