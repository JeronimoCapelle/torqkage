#![no_main]

use libfuzzer_sys::fuzz_target;
use tkg_common::{
    extract_candidate_paths,
    hook_dialect::{
        ClaudeCodeDialect, CursorDialect, GeminiCliDialect, GithubCopilotDialect, HookDialect,
        ProtoJsonDialect, detect_payload_dialect,
    },
    is_manifest_protected, path_matches_pattern,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let _ = detect_payload_dialect(input);

        let dialects: [&dyn HookDialect; 5] = [
            &ProtoJsonDialect,
            &ClaudeCodeDialect,
            &GeminiCliDialect,
            &CursorDialect,
            &GithubCopilotDialect,
        ];
        for dialect in dialects {
            let _ = dialect.parse_pre_tool_use(input);
            let _ = dialect.parse_pre_invocation(input);
            let _ = dialect.parse_stop(input);
            let _ = dialect.parse_session_start(input);
        }

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(input) {
            for candidate in extract_candidate_paths(&val) {
                let _ = is_manifest_protected(&candidate, &[]);
            }
        }
        let _ = is_manifest_protected(input, &[]);
        let _ = path_matches_pattern(input, "src/**/*.rs");
    }
});
