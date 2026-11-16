//! Property-based (`proptest`) and snapshot (`insta`) verification suite for
//! `tkg-common`.

use std::path::Path;

use proptest::prelude::*;
use serde_json::json;
use tkg_common::{
    AgentFrontmatter, AgentGuard, BlockCommentDelimiters, CircuitBreakerLedger,
    DEFAULT_PROTECTED_MANIFESTS, HookDecision, PathPolicy, PreToolResponse, SyntaxConfig,
    check_dangerous_files_with_security, check_manifest_protection, extract_candidate_paths,
    harness::{Antigravity, ClaudeCode, OpenCode},
    hook_dialect::{
        ClaudeCodeDialect, CursorDialect, GeminiCliDialect, GithubCopilotDialect, HookDialect,
        ProtoJsonDialect, detect_payload_dialect,
    },
    is_manifest_protected, matches_directory_scope,
    models::extract_compiler_error_signature,
    path_matches_pattern,
    project::parse_project_toml,
    strip_comments_and_normalize_with_syntax, strip_delimited, strip_delimited_and_normalize,
    strip_tests_and_normalize_with_syntax,
    tools::{RawToolsConfig, resolve as resolve_tools},
};

fn rust_syntax() -> SyntaxConfig {
    SyntaxConfig {
        line_comments: vec!["//".to_owned()],
        block_comments: vec![BlockCommentDelimiters::new("/*", "*/")],
        test_attributes: vec!["test".to_owned(), "cfg(test)".to_owned()],
        test_prefixes: Vec::new(),
    }
}

fn python_syntax() -> SyntaxConfig {
    SyntaxConfig {
        line_comments: vec!["#".to_owned()],
        block_comments: vec![
            BlockCommentDelimiters::new("\"\"\"", "\"\"\""),
            BlockCommentDelimiters::new("'''", "'''"),
        ],
        test_attributes: vec!["@pytest".to_owned(), "@unittest".to_owned()],
        test_prefixes: vec!["test_".to_owned(), "Test".to_owned()],
    }
}

proptest! {
    #[test]
    fn prop_strip_comments_never_panics_and_is_idempotent(input in "\\PC*") {
        let rs_syntax = rust_syntax();
        let py_syntax = python_syntax();

        let once_rs = strip_comments_and_normalize_with_syntax(&input, &rs_syntax);
        let twice_rs = strip_comments_and_normalize_with_syntax(&once_rs, &rs_syntax);
        prop_assert_eq!(once_rs, twice_rs);

        let once_py = strip_comments_and_normalize_with_syntax(&input, &py_syntax);
        let twice_py = strip_comments_and_normalize_with_syntax(&once_py, &py_syntax);
        prop_assert_eq!(once_py, twice_py);
    }

    #[test]
    fn prop_strip_tests_never_panics_and_is_idempotent(input in "\\PC*") {
        let rs_syntax = rust_syntax();
        let py_syntax = python_syntax();

        let once_rs = strip_tests_and_normalize_with_syntax(&input, &rs_syntax, None, None);
        let twice_rs = strip_tests_and_normalize_with_syntax(&once_rs, &rs_syntax, None, None);
        prop_assert_eq!(once_rs, twice_rs);

        let once_py = strip_tests_and_normalize_with_syntax(&input, &py_syntax, None, None);
        let twice_py = strip_tests_and_normalize_with_syntax(&once_py, &py_syntax, None, None);
        prop_assert_eq!(once_py, twice_py);
    }

    #[test]
    fn prop_strip_delimited_is_idempotent(
        before in "[a-zA-Z0-9_ ]{0,40}",
        middle in "\\PC{0,60}",
        after in "[a-zA-Z0-9_ ]{0,40}",
    ) {
        let start = "<<START>>";
        let end = "<<END>>";
        let combined = format!("{before}{start}{middle}{end}{after}");
        let stripped = strip_delimited(&combined, start, end);
        let stripped_again = strip_delimited(&stripped, start, end);
        prop_assert_eq!(&stripped, &stripped_again);
        prop_assert!(!stripped.contains(start));
    }

    #[test]
    fn prop_frontmatter_roundtrip_preserves_fields(
        name in "[a-z][a-z0-9_-]{1,20}",
        description in "[a-zA-Z0-9 .,_-]{1,50}",
        effort in prop_oneof![Just("low".to_owned()), Just("medium".to_owned()), Just("high".to_owned())],
        main_agent in any::<bool>(),
        subagent in any::<bool>(),
        directives in "[a-zA-Z0-9 #._-]{1,80}",
    ) {
        let fm = AgentFrontmatter {
            name,
            description,
            effort,
            permission_mode: "default".to_owned(),
            command_execution_policy: "turbo".to_owned(),
            main_agent,
            subagent,
            inherit_mcp: false,
            allowed_subagents: vec!["helper".to_owned()],
            pre_launch_checks: vec!["workspace_clean".to_owned()],
            allowed_dirs: vec!["src".to_owned()],
            disallowed_dirs: vec!["docs".to_owned()],
            allowed_files: vec!["Cargo.toml".to_owned()],
            disallowed_files: vec!["deny.toml".to_owned()],
            path_policies: vec![PathPolicy::comments_only("src/**/*.rs")],
            native_tools: vec!["read_file".to_owned()],
            mcp_tools: vec!["modify_code".to_owned()],
            directives: format!("{directives}\n"),
        };

        let serialized = fm.serialize_for_harness(&Antigravity);
        let parsed = AgentFrontmatter::parse(&serialized).expect("valid serialized frontmatter");

        prop_assert_eq!(parsed.name, fm.name);
        prop_assert_eq!(parsed.description, fm.description);
        prop_assert_eq!(parsed.effort, fm.effort);
        prop_assert_eq!(parsed.main_agent, fm.main_agent);
        prop_assert_eq!(parsed.subagent, fm.subagent);
        prop_assert_eq!(parsed.allowed_dirs, fm.allowed_dirs);
        prop_assert_eq!(parsed.disallowed_dirs, fm.disallowed_dirs);
        prop_assert_eq!(parsed.path_policies, fm.path_policies);
        prop_assert_eq!(parsed.mcp_tools, fm.mcp_tools);
    }

    #[test]
    fn prop_arbitrary_inputs_never_panic_parsers(raw in "\\PC*") {
        _ = AgentFrontmatter::parse(&raw);
        _ = extract_compiler_error_signature(&raw, None);
        _ = check_dangerous_files_with_security(Path::new(&raw), None);
        _ = strip_delimited_and_normalize(&raw, "/*", "*/");
        _ = parse_project_toml(&raw);
        let raw_tools = RawToolsConfig::from_toml_str(&raw);
        _ = resolve_tools(&raw_tools);
        _ = path_matches_pattern(&raw, "src/**/*.rs");
    }

    #[test]
    fn prop_manifest_guard_catches_all_protected_variants(
        prefix in prop::collection::vec("[a-z0-9_]{1,8}", 0..4),
        manifest_idx in 0usize..DEFAULT_PROTECTED_MANIFESTS.len(),
        flip_case in any::<bool>(),
        key_name in prop_oneof![Just("path"), Just("file_path"), Just("target_file")],
    ) {
        let base = DEFAULT_PROTECTED_MANIFESTS.get(manifest_idx).copied().unwrap_or("Cargo.toml");
        let target_name = if flip_case {
            base.to_ascii_uppercase()
        } else {
            base.to_owned()
        };
        let rel = if prefix.is_empty() {
            target_name
        } else {
            format!("{}/{}", prefix.join("/"), target_name)
        };
        let payload = json!({ key_name: rel, "nested": [{ "path": "src/lib.rs" }] });
        let extracted = extract_candidate_paths(&payload);
        prop_assert!(extracted.iter().any(|p| p == &rel));
        prop_assert!(is_manifest_protected(&rel, &[]));

        let restrictive_guard = AgentGuard {
            allowed_files: vec!["src/*.rs".to_owned()],
            ..Default::default()
        };
        prop_assert!(
            check_manifest_protection(Path::new(&rel), Some("coder"), Some(&restrictive_guard))
                .is_err()
        );
    }

    #[test]
    fn prop_directory_scope_rejects_traversal_and_accepts_subpaths(
        allowed_dir in "[a-z]{2,8}",
        sub in "[a-z0-9_]{1,8}",
        file in "[a-z0-9_]{1,8}\\.rs",
    ) {
        let inside_rel = format!("{allowed_dir}/{sub}/{file}");
        let other_rel = format!("other_{inside_rel}");
        prop_assert!(matches_directory_scope(&inside_rel, &allowed_dir));
        prop_assert!(!matches_directory_scope(&other_rel, &allowed_dir));

        let ws = Path::new("/tmp/tkg_prop_scope_ws");
        let guard = AgentGuard {
            allowed_dirs: vec![allowed_dir],
            ..Default::default()
        };
        let inside = ws.join(&inside_rel);
        prop_assert!(tkg_common::check_agent_guard_direct(ws, &inside, Some("coder"), Some(&guard)).is_ok());

        let outside = ws.join("forbidden").join(&file);
        prop_assert!(tkg_common::check_agent_guard_direct(ws, &outside, Some("coder"), Some(&guard)).is_err());
    }

    #[test]
    fn prop_circuit_breaker_arithmetic_invariants(
        max_strikes in 0usize..=10,
        strikes in 0usize..=25,
    ) {
        let mut ledger = CircuitBreakerLedger::new("prop-agent").with_max_strikes(max_strikes);
        for _ in 0..strikes {
            ledger.record_failure_in_memory(Some("prop-agent"), Some("test"), Some("ERR"), None);
        }
        prop_assert_eq!(ledger.consecutive_failures, strikes);
        if strikes > 0 {
            prop_assert_eq!(ledger.tripped, strikes >= max_strikes);
        } else {
            prop_assert!(!ledger.tripped);
        }
    }

    #[test]
    fn prop_hook_dialects_never_panic_and_format_decisions(
        raw in "\\PC{0,256}",
        reason in "[a-zA-Z0-9 ._-]{0,60}",
        decision_idx in 0usize..4,
    ) {
        _ = detect_payload_dialect(&raw);
        let decision = match decision_idx {
            0 => HookDecision::Allow,
            1 => HookDecision::Ask,
            2 => HookDecision::Deny,
            _ => HookDecision::ForceAsk,
        };
        let resp = match decision {
            HookDecision::Allow => PreToolResponse::allow(),
            HookDecision::Ask | HookDecision::ForceAsk => PreToolResponse::ask(reason),
            HookDecision::Deny => PreToolResponse::deny(reason),
        };
        let dialects: [&dyn HookDialect; 5] = [
            &ProtoJsonDialect,
            &ClaudeCodeDialect,
            &GeminiCliDialect,
            &CursorDialect,
            &GithubCopilotDialect,
        ];
        for dialect in dialects {
            _ = dialect.parse_pre_tool_use(&raw);
            _ = dialect.parse_stop(&raw);
            _ = dialect.parse_session_start(&raw);
            let formatted = dialect.format_pre_tool_use(&resp);
            prop_assert!(formatted.is_ok());
        }
    }

    #[test]
    fn prop_comments_only_allows_arbitrary_comment_and_blocks_code_mutation(
        val_a in 1u32..500,
        delta in 1u32..500,
        comment_text in "[a-zA-Z0-9 _.:;?!-]{1,60}",
        evil_payload in "\\PC{1,60}",
    ) {
        let policy = PathPolicy::comments_only("src");
        let val_b = val_a.saturating_add(delta);
        let original = format!("pub fn value() -> u32 {{\n    {val_a}\n}}\n");
        let comment_only = format!(
            "/// {comment_text}\npub fn value() -> u32 {{\n    // {comment_text}\n    {val_a}\n}}\n"
        );
        let code_tampered = format!(
            "/// {comment_text}\npub fn value() -> u32 {{\n    {val_b} // {evil_payload}\n}}\n"
        );
        // Comment-delimiter escape attempt (`*/ tampered_code /*`) must never pass comments_only.
        let safe_evil = evil_payload.replace("/*", "").replace("*/", "");
        let delimiter_smuggle = format!(
            "/* {safe_evil} */\n// ok\npub fn value() -> u32 {{\n    {val_a} + /* {safe_evil} */ {val_b}\n}}\n"
        );
        prop_assert!(policy.verify(&original, &comment_only).is_ok());
        prop_assert!(policy.verify(&original, &code_tampered).is_err());
        prop_assert!(policy.verify(&original, &delimiter_smuggle).is_err());
    }

    #[test]
    fn prop_delimited_allows_arbitrary_inner_edit_and_rejects_outside_edit(
        header in "[a-zA-Z0-9_]{3,16}",
        footer in "[a-zA-Z0-9_]{3,16}",
        inner_after in "[a-zA-Z0-9 _=;]{1,40}",
    ) {
        let policy = PathPolicy::delimited("src/gen.rs", "// BEGIN AUTOGEN", "// END AUTOGEN");
        let original = format!(
            "let h = \"{header}\";\n// BEGIN AUTOGEN\nlet x = 1;\n// END AUTOGEN\nlet f = \"{footer}\";\n"
        );
        let valid = format!(
            "let h = \"{header}\";\n// BEGIN AUTOGEN\n{inner_after}\n// END AUTOGEN\nlet f = \"{footer}\";\n"
        );
        let outside_tamper = format!(
            "let h = \"{header}_tamper\";\n// BEGIN AUTOGEN\n{inner_after}\n// END AUTOGEN\nlet f = \"{footer}\";\n"
        );
        prop_assert!(policy.verify(&original, &valid).is_ok());
        prop_assert!(policy.verify(&original, &outside_tamper).is_err());
    }

    #[test]
    fn prop_tests_only_allows_arbitrary_test_add_and_blocks_prod_mutation(
        val_a in 1u32..500,
        delta in 1u32..500,
        test_name in "[a-z][a-z0-9_]{2,15}",
    ) {
        let policy = PathPolicy::tests_only("src");
        let rs_syntax = rust_syntax();
        let val_b = val_a.saturating_add(delta);
        let original = format!("pub fn compute() -> u32 {{\n    {val_a}\n}}\n");
        let valid_test = format!(
            "pub fn compute() -> u32 {{\n    {val_a}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn {test_name}() {{\n        assert_eq!(super::compute(), {val_a});\n    }}\n}}\n"
        );
        let tampered_prod = format!(
            "pub fn compute() -> u32 {{\n    {val_b}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn {test_name}() {{}}\n}}\n"
        );
        let rs_path = Path::new("src/lib.rs");
        prop_assert!(policy.verify_with_syntax(rs_path, &original, &valid_test, &rs_syntax).is_ok());
        prop_assert!(policy.verify_with_syntax(rs_path, &original, &tampered_prod, &rs_syntax).is_err());

        let py_syntax = python_syntax();
        let py_orig = format!("def compute():\n    return {val_a}\n");
        let py_valid = format!("def compute():\n    return {val_a}\n\ndef test_{test_name}():\n    assert compute() == {val_a}\n");
        let py_tamper = format!("def compute():\n    return {val_b}\n\ndef test_{test_name}():\n    assert True\n");
        let py_path = Path::new("src/app.py");
        prop_assert!(policy.verify_with_syntax(py_path, &py_orig, &py_valid, &py_syntax).is_ok());
        prop_assert!(policy.verify_with_syntax(py_path, &py_orig, &py_tamper, &py_syntax).is_err());
    }
}

#[test]
fn test_snapshot_frontmatter_dialects() {
    let fm = AgentFrontmatter {
        name: "test-author".to_owned(),
        description: "Design Verification & Formal Test Engineer".to_owned(),
        effort: "high".to_owned(),
        permission_mode: "default".to_owned(),
        command_execution_policy: "turbo".to_owned(),
        main_agent: true,
        subagent: true,
        inherit_mcp: false,
        allowed_subagents: vec!["mini".to_owned()],
        pre_launch_checks: vec!["workspace_clean".to_owned()],
        allowed_dirs: vec!["tests".to_owned(), "src".to_owned()],
        disallowed_dirs: vec!["docs".to_owned()],
        allowed_files: Vec::new(),
        disallowed_files: vec!["Cargo.toml".to_owned()],
        path_policies: vec![PathPolicy::tests_only("src/**")],
        native_tools: vec!["read_file".to_owned()],
        mcp_tools: vec!["cargo_nextest".to_owned(), "modify_code".to_owned()],
        directives: "# Verification Directives\nWrite deterministic property tests.\n".to_owned(),
    };

    insta::assert_snapshot!(
        "frontmatter_antigravity",
        fm.serialize_for_harness(&Antigravity)
    );
    insta::assert_snapshot!("frontmatter_opencode", fm.serialize_for_harness(&OpenCode));
    insta::assert_snapshot!(
        "frontmatter_claudecode",
        fm.serialize_for_harness(&ClaudeCode)
    );
}
