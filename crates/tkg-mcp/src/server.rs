//! The Model Context Protocol (MCP) server daemon handling JSON-RPC 2.0
//! requests over stdio.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    actions,
    models::{AgentGuard, ToolDefinition},
    protocol::{JsonRpcRequest, JsonRpcResponse},
    schema::McpTool,
};

/// State machine managing the MCP server daemon and dispatching JSON-RPC
/// requests.
#[derive(Debug, Clone)]
pub(crate) struct McpServer {
    tools: Vec<ToolDefinition>,
    mcp_tools: Vec<McpTool>,
    workspace_root: PathBuf,
    agent: Option<String>,
    agent_guards: BTreeMap<String, AgentGuard>,
    detected_harness: Arc<RwLock<Option<String>>>,
}

impl McpServer {
    /// Constructs a new `McpServer` instance, pre-generating MCP tool schemas
    /// for fast responses. If `agent_name` is provided, tools are filtered to
    /// only those authorized for that agent.
    #[must_use]
    pub(crate) fn new(
        tools: Vec<ToolDefinition>,
        workspace_root: PathBuf,
        agent_name: Option<String>,
        agent_guards: BTreeMap<String, AgentGuard>,
    ) -> Self {
        let (tools, agent) = match agent_name {
            Some(name) => {
                let allowed = crate::agent_scope::resolve_allowed_tools(&workspace_root, &name);
                let total = tools.len();
                let filtered: Vec<_> = tools
                    .into_iter()
                    .filter(|t| allowed.contains("*") || allowed.contains(&t.name))
                    .collect();
                tracing::info!(
                    agent = %name,
                    exposed = filtered.len(),
                    total,
                    "[tkg-mcp] Scoped toolset to active agent"
                );
                (filtered, Some(name))
            },
            None => (tools, None),
        };

        let project = tkg_common::ProjectConfig::detect_or_load(&workspace_root);
        let mut enriched_tools = tools;
        for t in &mut enriched_tools {
            if t.is_documentation_tool(&project) && !project.docs.paths.is_empty() {
                t.description = format!(
                    "Searches project documentation and specifications across [{}] for grep-ready specs, opcodes, and ABI definitions",
                    project.docs.paths.join(", ")
                );
            }
        }

        let mcp_tools = enriched_tools
            .iter()
            .map(McpTool::from_definition)
            .collect();

        let initial_harness = std::env::var("TKG_HARNESS")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| tkg_common::harness::resolve(&v).id().to_owned())
            .or_else(tkg_common::harness::detect_parent_process_harness);

        Self {
            tools: enriched_tools,
            mcp_tools,
            workspace_root,
            agent,
            agent_guards,
            detected_harness: Arc::new(RwLock::new(initial_harness)),
        }
    }

    fn caller_harness(&self) -> Box<dyn tkg_common::harness::Harness> {
        if let Ok(guard) = self.detected_harness.read()
            && let Some(ref id) = *guard
        {
            return tkg_common::harness::resolve(id);
        }
        tkg_common::harness::runner_for(&self.workspace_root).harness()
    }

    /// Handles a single incoming JSON-RPC request and produces a response (if
    /// not a notification).
    #[must_use]
    pub(crate) fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let Some(id) = req.id else {
            handle_notification(&req);
            return None;
        };

        match req.method.as_str() {
            "initialize" => {
                if let Some(client_name) = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("clientInfo"))
                    .and_then(|ci| ci.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    let resolved = tkg_common::harness::resolve_client_name(client_name);
                    let id = resolved.id().to_owned();
                    tracing::info!(
                        harness = %id,
                        client = client_name,
                        native_hooks = resolved.supports_hooks(),
                        "[tkg-mcp] Identified MCP caller harness"
                    );
                    if let Ok(mut guard) = self.detected_harness.write() {
                        *guard = Some(id);
                    }
                }

                let result = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        },
                        "resources": {
                            "subscribe": false,
                            "listChanged": false
                        },
                        "prompts": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "tkg-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                Some(JsonRpcResponse::success(id, result))
            },

            "tools/list" => {
                let result = json!({
                    "tools": self.mcp_tools
                });
                Some(JsonRpcResponse::success(id, result))
            },

            "resources/list" => Some(Self::handle_resources_list(id)),

            "resources/read" => Some(self.handle_resources_read(id, req.params.as_ref())),

            "prompts/list" => Some(Self::handle_prompts_list(id)),

            "prompts/get" => Some(Self::handle_prompts_get(id, req.params.as_ref())),

            "ping" => Some(JsonRpcResponse::success(id, json!({}))),

            "tools/call" => {
                let tool_name = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");

                if tool_name.is_empty() {
                    return Some(JsonRpcResponse::invalid_params(
                        id,
                        "Missing required parameter 'name' in tools/call",
                    ));
                }

                let Some(tool) = self.tools.iter().find(|t| t.name == tool_name) else {
                    let agent_hint = self
                        .agent
                        .as_ref()
                        .map_or_else(String::new, |a| format!(" (unauthorized for agent '{a}')"));
                    return Some(JsonRpcResponse::error(
                        id,
                        -32602,
                        format!("Tool '{tool_name}' is not recognized by tkg-mcp{agent_hint}"),
                    ));
                };

                let empty_args = json!({});
                let arguments = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("arguments"))
                    .unwrap_or(&empty_args);

                let in_band_seatbelt = !self.caller_harness().supports_hooks();
                let (content_text, is_error) = match actions::dispatch_action(
                    tool,
                    &self.workspace_root,
                    arguments,
                    self.agent.as_deref(),
                    &self.agent_guards,
                    &self.tools,
                    in_band_seatbelt,
                ) {
                    Ok((text, err)) => (text, err),
                    Err(err_msg) => (err_msg, true),
                };

                let result = json!({
                    "content": [
                        {
                            "type": "text",
                            "text": content_text
                        }
                    ],
                    "isError": is_error
                });

                Some(JsonRpcResponse::success(id, result))
            },

            unknown => Some(JsonRpcResponse::method_not_found(id, unknown)),
        }
    }

    fn handle_resources_list(id: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "resources": [
                    {
                        "uri": "tkg://rules",
                        "name": "TorqKage Rules & Boundaries",
                        "description": "Merged project invariants, agent path boundaries, and tool sandbox policies",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "tkg://status",
                        "name": "TorqKage Live Status",
                        "description": "Live circuit breaker strike count, active agent scope, and sandbox availability",
                        "mimeType": "application/json"
                    }
                ]
            }),
        )
    }

    fn handle_resources_read(
        &self,
        id: serde_json::Value,
        params: Option<&serde_json::Value>,
    ) -> JsonRpcResponse {
        let uri = params
            .and_then(|p| p.get("uri"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        if uri.is_empty() {
            return JsonRpcResponse::invalid_params(
                id,
                "Missing required parameter 'uri' in resources/read",
            );
        }

        let payload = match uri {
            "tkg://rules" => self.build_rules_resource(),
            "tkg://status" => self.build_status_resource(),
            other => {
                return JsonRpcResponse::invalid_params(
                    id,
                    format!("Unknown resource URI '{other}'"),
                );
            },
        };

        let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
        JsonRpcResponse::success(
            id,
            json!({
                "contents": [
                    {
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": text
                    }
                ]
            }),
        )
    }

    fn build_rules_resource(&self) -> serde_json::Value {
        let project = tkg_common::ProjectConfig::detect_or_load(&self.workspace_root);
        let tools_summary: Vec<_> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "category": t.category,
                    "mutating": t.safety.mutating,
                    "tracks_circuit_breaker": t.safety.tracks_circuit_breaker,
                    "require_permission_gate": t.safety.require_permission_gate,
                    "enforce_sandbox": t.enforce_sandbox(),
                    "enforce_workspace_bounds": t.sandbox.enforce_workspace_bounds,
                    "ban_dangerous_files": t.sandbox.ban_dangerous_files,
                    "require_gates": t.preconditions.require_gates,
                })
            })
            .collect();

        json!({
            "workspace_root": self.workspace_root,
            "active_agent": self.agent,
            "agent_guards": self.agent_guards,
            "docs_paths": project.docs.paths,
            "exposed_tools": tools_summary,
        })
    }

    fn build_status_resource(&self) -> serde_json::Value {
        let ledger = tkg_common::CircuitBreakerLedger::load(&self.workspace_root);
        let active_agent = self
            .agent
            .clone()
            .or_else(|| tkg_common::resolve_active_agent(&self.workspace_root));

        let caller = self.caller_harness();

        json!({
            "workspace_root": self.workspace_root,
            "active_agent": active_agent,
            "detected_harness": caller.id(),
            "in_band_seatbelt": !caller.supports_hooks(),
            "exposed_tools_count": self.tools.len(),
            "inside_container": tkg_common::is_inside_container(),
            "bwrap_available": which::which("bwrap").is_ok(),
            "circuit_breaker": ledger,
        })
    }

    fn handle_prompts_list(id: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "prompts": [
                    {
                        "name": "tkg-refactor",
                        "description": "Standardized systems refactoring workflow under TorqKage guards",
                        "arguments": [
                            {
                                "name": "target",
                                "description": "Module, file, or symbol to refactor",
                                "required": true
                            }
                        ]
                    },
                    {
                        "name": "tkg-audit",
                        "description": "Verification, security boundary, and quality gate audit checklist",
                        "arguments": [
                            {
                                "name": "scope",
                                "description": "Subsystem or diff scope to audit",
                                "required": false
                            }
                        ]
                    }
                ]
            }),
        )
    }

    fn handle_prompts_get(
        id: serde_json::Value,
        params: Option<&serde_json::Value>,
    ) -> JsonRpcResponse {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        if name.is_empty() {
            return JsonRpcResponse::invalid_params(
                id,
                "Missing required parameter 'name' in prompts/get",
            );
        }

        let args = params.and_then(|p| p.get("arguments"));
        let (description, text) = match name {
            "tkg-refactor" => {
                let target = args
                    .and_then(|a| a.get("target"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("the target module");
                (
                    "Standardized systems refactoring workflow under TorqKage guards",
                    format!(
                        "Refactor `{target}` following TorqKage invariants:\n\
                         1. Read `tkg://rules` and inspect existing callers before editing.\n\
                         2. Extract shared helpers rather than duplicating logic across variants.\n\
                         3. Keep functions focused with shallow nesting and preserve unrelated docs/comments.\n\
                         4. Run compilation, lint, and test gates after edits; never bypass a tripped circuit breaker."
                    ),
                )
            },
            "tkg-audit" => {
                let scope = args
                    .and_then(|a| a.get("scope"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("the workspace");
                (
                    "Verification, security boundary, and quality gate audit checklist",
                    format!(
                        "Audit `{scope}` against TorqKage quality and security gates:\n\
                         1. Inspect `tkg://status` and `tkg://rules` for active guards and breaker state.\n\
                         2. Verify fail-closed config/sandbox behavior and workspace path confinement.\n\
                         3. Confirm zero swallowed errors on fallible I/O or VCS calls.\n\
                         4. Verify formatting, clippy (`-D warnings`), tests, and documentation links."
                    ),
                )
            },
            other => {
                return JsonRpcResponse::invalid_params(id, format!("Unknown prompt '{other}'"));
            },
        };

        JsonRpcResponse::success(
            id,
            json!({
                "description": description,
                "messages": [
                    {
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": text
                        }
                    }
                ]
            }),
        )
    }

    /// Runs the long-running async stdio communication loop.
    ///
    /// # Errors
    /// Returns an I/O error if reading from `stdin` or writing to `stdout`
    /// fails.
    pub(crate) async fn run_stdio(&self) -> std::io::Result<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();

        tracing::info!("[tkg-mcp] Listening for JSON-RPC 2.0 messages on stdio...");

        while let Some(line) = reader.next_line().await? {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let parse_result = serde_json::from_str::<JsonRpcRequest>(trimmed);

            match parse_result {
                Ok(request) => {
                    if let Some(response) = self.handle_request(request) {
                        let mut response_bytes = serde_json::to_vec(&response)?;
                        response_bytes.push(b'\n');
                        stdout.write_all(&response_bytes).await?;
                        stdout.flush().await?;
                    }
                },
                Err(err) => {
                    tracing::warn!("[tkg-mcp] JSON-RPC parse error: {err}");
                    let error_resp = JsonRpcResponse::error(
                        serde_json::Value::Null,
                        -32700,
                        format!("Parse error: {err}"),
                    );
                    let mut response_bytes = serde_json::to_vec(&error_resp)?;
                    response_bytes.push(b'\n');
                    stdout.write_all(&response_bytes).await?;
                    stdout.flush().await?;
                },
            }
        }

        tracing::info!("[tkg-mcp] Stdio stream closed, exiting daemon gracefully.");
        Ok(())
    }
}

fn handle_notification(req: &JsonRpcRequest) {
    if req.method == "notifications/initialized" {
        tracing::info!("[tkg-mcp] Handshake complete: MCP client successfully connected");
    } else if req.method == "notifications/cancelled" {
        tracing::debug!("[tkg-mcp] Received operation cancellation notification from client");
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "Unit test assertions")]
mod tests {
    use super::*;
    use crate::models::ToolAction;

    fn sample_server() -> McpServer {
        let tools = vec![
            ToolDefinition {
                name: "read_project_file".to_owned(),
                description: "Read project file".to_owned(),
                action: ToolAction::FileRead,
                ..Default::default()
            },
            ToolDefinition {
                name: "run_check".to_owned(),
                description: "Run cargo check".to_owned(),
                action: ToolAction::StaticCommand,
                command: vec!["cargo".to_owned(), "check".to_owned()],
                ..Default::default()
            },
        ];

        McpServer::new(tools, std::env::temp_dir(), None, BTreeMap::new())
    }

    #[test]
    fn test_initialize_handshake() {
        let server = sample_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(1)),
            method: "initialize".to_owned(),
            params: Some(json!({})),
        };

        let resp = server.handle_request(req).expect("response expected");
        assert_eq!(resp.id, json!(1));
        let res = resp.result.expect("result expected");
        assert_eq!(res["protocolVersion"], "2024-11-05");
        assert_eq!(res["serverInfo"]["name"], "tkg-mcp");
    }

    #[test]
    fn test_tools_list() {
        let server = sample_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(2)),
            method: "tools/list".to_owned(),
            params: None,
        };

        let resp = server.handle_request(req).expect("response expected");
        let res = resp.result.expect("result expected");
        let tools = res["tools"].as_array().expect("array expected");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "read_project_file");
        assert_eq!(tools[1]["name"], "run_check");
    }

    #[test]
    fn test_ping() {
        let server = sample_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!("ping-1")),
            method: "ping".to_owned(),
            params: None,
        };

        let resp = server.handle_request(req).expect("response expected");
        assert_eq!(resp.id, json!("ping-1"));
        assert_eq!(resp.result, Some(json!({})));
    }

    #[test]
    fn test_tools_call_file_read() {
        let temp_dir = std::env::temp_dir().join(format!("tkg_test_read_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("create dir");
        let test_file = temp_dir.join("test.txt");
        std::fs::write(&test_file, "Hello from file_read action test!").expect("write test file");

        let tools = vec![ToolDefinition {
            name: "read_project_file".to_owned(),
            action: ToolAction::FileRead,
            ..Default::default()
        }];

        let server = McpServer::new(tools, temp_dir.clone(), None, BTreeMap::new());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(3)),
            method: "tools/call".to_owned(),
            params: Some(
                json!({ "name": "read_project_file", "arguments": { "path": "test.txt" } }),
            ),
        };

        let resp = server.handle_request(req).expect("response expected");
        let res = resp.result.expect("result expected");
        assert_eq!(res["isError"], false);
        let content = res["content"].as_array().expect("content array");
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("Hello from file_read")
        );

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_tools_call_write_modify_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("tkg_test_wm_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("create dir");

        let tools = vec![
            ToolDefinition {
                name: "write_file".to_owned(),
                action: ToolAction::FileWrite,
                ..Default::default()
            },
            ToolDefinition {
                name: "modify_code".to_owned(),
                action: ToolAction::MpatchEdit,
                ..Default::default()
            },
        ];

        let server = McpServer::new(tools, temp_dir.clone(), None, BTreeMap::new());

        // 1. Write file
        let write_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!("w-1")),
            method: "tools/call".to_owned(),
            params: Some(json!({
                "name": "write_file",
                "arguments": {
                    "path": "test.rs",
                    "content": "fn calculate() -> i32 {\n    10\n}\n"
                }
            })),
        };

        let write_resp = server.handle_request(write_req).expect("write resp");
        assert_eq!(write_resp.result.unwrap()["isError"], false);

        // 2. Modify file with Aider block
        let modify_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!("m-1")),
            method: "tools/call".to_owned(),
            params: Some(json!({
                "name": "modify_code",
                "arguments": {
                    "path": "test.rs",
                    "patch": "<<<<<<< SEARCH\n    10\n=======\n    42\n>>>>>>> REPLACE"
                }
            })),
        };

        let mod_resp = server.handle_request(modify_req).expect("mod resp");
        assert_eq!(mod_resp.result.unwrap()["isError"], false);

        let final_content = std::fs::read_to_string(temp_dir.join("test.rs")).unwrap();
        assert!(final_content.contains("42"));

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_agent_scoping_unauthorized_cannot_call_tool() {
        let tools = vec![
            ToolDefinition {
                name: "modify_code".to_owned(),
                description: "Modify source code".to_owned(),
                action: ToolAction::MpatchEdit,
                ..Default::default()
            },
            ToolDefinition {
                name: "eval_symbolic_math".to_owned(),
                description: "Symbolic math".to_owned(),
                action: ToolAction::ParameterizedCommand,
                ..Default::default()
            },
        ];

        let temp_dir = std::env::temp_dir().join(format!("tkg_test_scope_{}", std::process::id()));
        let agents_dir = temp_dir.join("plugins/torqkage/agents");
        std::fs::create_dir_all(&agents_dir).expect("create dir");
        std::fs::write(
            agents_dir.join("restricted_agent.md"),
            "---\nname: restricted_agent\nmcp_tools:\n  - modify_code\n---\n",
        )
        .expect("write mock restricted_agent.md");

        // Scope to restricted_agent
        let server = McpServer::new(
            tools,
            temp_dir.clone(),
            Some("restricted_agent".to_owned()),
            BTreeMap::new(),
        );

        // 1. tools/list should ONLY have modify_code, NOT eval_symbolic_math
        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(1)),
            method: "tools/list".to_owned(),
            params: None,
        };
        let list_resp = server.handle_request(list_req).expect("list resp");
        let tool_list = list_resp.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(tool_list.len(), 1);
        assert_eq!(tool_list[0]["name"], "modify_code");

        // 2. tools/call on eval_symbolic_math should fail with error
        let call_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(2)),
            method: "tools/call".to_owned(),
            params: Some(json!({
                "name": "eval_symbolic_math",
                "arguments": { "expression": "2+2" }
            })),
        };
        let call_resp = server.handle_request(call_req).expect("call resp");
        assert!(call_resp.error.is_some());
        let err = call_resp.error.unwrap();
        assert!(
            err.message
                .contains("unauthorized for agent 'restricted_agent'")
        );

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_agent_scoping_wildcard_grants_all_tools() {
        let tools = vec![
            ToolDefinition {
                name: "modify_code".to_owned(),
                description: "Modify source code".to_owned(),
                action: ToolAction::MpatchEdit,
                ..Default::default()
            },
            ToolDefinition {
                name: "eval_symbolic_math".to_owned(),
                description: "Symbolic math".to_owned(),
                action: ToolAction::ParameterizedCommand,
                ..Default::default()
            },
        ];

        let temp_dir =
            std::env::temp_dir().join(format!("tkg_test_scope_wildcard_{}", std::process::id()));
        let agents_dir = temp_dir.join("plugins/torqkage/agents");
        std::fs::create_dir_all(&agents_dir).expect("create dir");
        std::fs::write(
            agents_dir.join("wildcard_agent.md"),
            "---\nname: wildcard_agent\nmcp_tools:\n  - \"*\"\n---\n",
        )
        .expect("write mock wildcard_agent.md");

        let server = McpServer::new(
            tools,
            temp_dir.clone(),
            Some("wildcard_agent".to_owned()),
            BTreeMap::new(),
        );

        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(1)),
            method: "tools/list".to_owned(),
            params: None,
        };
        let list_resp = server.handle_request(list_req).expect("list resp");
        let tool_list = list_resp.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(tool_list.len(), 2);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resources_list_and_read() {
        let server = sample_server();

        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(10)),
            method: "resources/list".to_owned(),
            params: None,
        };
        let list_resp = server.handle_request(list_req).expect("list resp");
        let resources = list_resp.result.unwrap()["resources"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0]["uri"], "tkg://rules");
        assert_eq!(resources[1]["uri"], "tkg://status");

        let read_rules = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(11)),
                method: "resources/read".to_owned(),
                params: Some(json!({ "uri": "tkg://rules" })),
            })
            .expect("rules resp");
        let rules_text = read_rules.result.unwrap()["contents"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        let rules_json: serde_json::Value = serde_json::from_str(&rules_text).unwrap();
        assert!(rules_json["exposed_tools"].is_array());

        let read_status = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(12)),
                method: "resources/read".to_owned(),
                params: Some(json!({ "uri": "tkg://status" })),
            })
            .expect("status resp");
        let status_text = read_status.result.unwrap()["contents"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        let status_json: serde_json::Value = serde_json::from_str(&status_text).unwrap();
        assert_eq!(status_json["exposed_tools_count"], 2);
        assert!(status_json["circuit_breaker"]["max_strikes"].is_number());
    }

    #[test]
    fn test_prompts_list_and_get() {
        let server = sample_server();

        let list_resp = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(20)),
                method: "prompts/list".to_owned(),
                params: None,
            })
            .expect("prompts list");
        let prompts = list_resp.result.unwrap()["prompts"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0]["name"], "tkg-refactor");
        assert_eq!(prompts[1]["name"], "tkg-audit");

        let get_resp = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(21)),
                method: "prompts/get".to_owned(),
                params: Some(json!({
                    "name": "tkg-refactor",
                    "arguments": { "target": "crates/tkg-mcp/src/server.rs" }
                })),
            })
            .expect("prompt get");
        let msg_text = get_resp.result.unwrap()["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(msg_text.contains("crates/tkg-mcp/src/server.rs"));
    }

    #[test]
    fn test_initialize_client_info_updates_status_seatbelt() {
        let server = sample_server();
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(30)),
            method: "initialize".to_owned(),
            params: Some(json!({
                "clientInfo": { "name": "Windsurf Cascade", "version": "1.0.0" }
            })),
        };
        let init_resp = server.handle_request(init_req).expect("init response");
        assert!(init_resp.error.is_none());

        let status_req = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(31)),
            method: "resources/read".to_owned(),
            params: Some(json!({ "uri": "tkg://status" })),
        };
        let status_resp = server.handle_request(status_req).expect("status response");
        let status_text = status_resp.result.unwrap()["contents"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        let status_json: serde_json::Value = serde_json::from_str(&status_text).unwrap();
        assert_eq!(status_json["detected_harness"], "windsurf");
        assert_eq!(status_json["in_band_seatbelt"], true);

        let cur_init = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(32)),
            method: "initialize".to_owned(),
            params: Some(json!({
                "clientInfo": { "name": "Cursor", "version": "0.50.0" }
            })),
        };
        assert!(server.handle_request(cur_init).unwrap().error.is_none());
        let cur_status = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(33)),
                method: "resources/read".to_owned(),
                params: Some(json!({ "uri": "tkg://status" })),
            })
            .unwrap();
        let cur_text = cur_status.result.unwrap()["contents"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        let cur_json: serde_json::Value = serde_json::from_str(&cur_text).unwrap();
        assert_eq!(cur_json["detected_harness"], "cursor");
        assert_eq!(cur_json["in_band_seatbelt"], false);
    }

    #[test]
    fn test_in_band_circuit_breaker_halts_mutating_calls() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg_inband_cb_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let tools = vec![
            ToolDefinition {
                name: "failing_check".to_owned(),
                description: "Simulated failing check".to_owned(),
                action: ToolAction::StaticCommand,
                command: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "echo 'error[E0308]: mismatched types' >&2; exit 1".to_owned(),
                ],
                ..Default::default()
            },
            ToolDefinition {
                name: "write_project_file".to_owned(),
                description: "Write project file".to_owned(),
                action: ToolAction::FileWrite,
                safety: tkg_common::tools::ToolSafetyPolicy {
                    mutating: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        let server = McpServer::new(tools, temp_dir.clone(), None, BTreeMap::new());
        let init_resp = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(1)),
                method: "initialize".to_owned(),
                params: Some(json!({ "clientInfo": { "name": "windsurf" } })),
            })
            .expect("init response");
        assert!(init_resp.error.is_none());

        for call_id in [2, 3] {
            let resp = server
                .handle_request(JsonRpcRequest {
                    jsonrpc: "2.0".to_owned(),
                    id: Some(json!(call_id)),
                    method: "tools/call".to_owned(),
                    params: Some(json!({ "name": "failing_check", "arguments": {} })),
                })
                .expect("failing_check response");
            assert_eq!(resp.result.unwrap()["isError"], true);
        }

        let blocked_resp = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(4)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "write_project_file",
                    "arguments": { "path": "foo.txt", "content": "bar" }
                })),
            })
            .expect("blocked response");
        let result = blocked_resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let msg = result["content"][0]["text"].as_str().unwrap();
        assert!(msg.contains("CIRCUIT BREAKER TRIPPED"));

        _ = std::fs::remove_dir_all(&temp_dir);
    }
}
