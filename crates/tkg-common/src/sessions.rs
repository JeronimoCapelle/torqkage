//! Multi-harness conversation session discovery across `SQLite` (`agy`),
//! Claude Code JSONL transcripts, `OpenAI` Codex CLI rollouts, Google Gemini
//! CLI chats, Cursor transcripts, `OpenCode` JSON files, and
//! `.tkg/state/sessions`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Compact summary of a past conversation session for a specific agent in a
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionMeta {
    pub conversation_id: String,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub workspace_uris: String,
    #[serde(default)]
    pub last_modified_time: String,
    #[serde(default)]
    pub step_count: u64,
    #[serde(default)]
    pub preview: String,
}

impl SessionMeta {
    /// Formats a short prefix of the UUID conversation ID.
    #[must_use]
    pub fn short_id(&self) -> &str {
        self.conversation_id
            .get(..8)
            .unwrap_or(&self.conversation_id)
    }
}

/// Discovers JSON session metadata files stored under `dirs`, sorted newest
/// first.
#[must_use]
pub fn discover_json_sessions_in_dirs(dirs: &[PathBuf], agent_name: &str) -> Vec<SessionMeta> {
    let mut sessions = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(sess) = parse_json_session(&val, &path, agent_name)
            {
                sessions.push(sess);
            }
        }
    }

    sessions.sort_by(|a, b| b.last_modified_time.cmp(&a.last_modified_time));
    sessions
}

/// Discovers `OpenCode` session files across workspace and user data
/// directories.
#[must_use]
pub fn discover_opencode_sessions(agent_name: &str, workspace_root: &Path) -> Vec<SessionMeta> {
    let mut candidate_dirs = vec![
        workspace_root.join(".opencode").join("sessions"),
        workspace_root.join(".opencode"),
    ];

    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        candidate_dirs.push(home_path.join(".local/share/opencode/sessions"));
        candidate_dirs.push(home_path.join(".opencode/sessions"));
    }

    discover_json_sessions_in_dirs(&candidate_dirs, agent_name)
}

/// Discovers generic `TorqKage` session JSON snapshots under
/// `.tkg/state/sessions`.
#[must_use]
pub fn discover_generic_state_sessions(
    agent_name: &str,
    workspace_root: &Path,
) -> Vec<SessionMeta> {
    let dirs = [workspace_root.join(".tkg/state/sessions")];
    discover_json_sessions_in_dirs(&dirs, agent_name)
}

fn file_mtime_string(path: &Path) -> String {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .map_or_else(
            || "1970-01-01T00:00:00Z".to_owned(),
            |t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .to_string()
            },
        )
}

fn file_stem_or_unknown(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_owned()
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn workspace_matches_path(candidate: &str, canonical_ws: &Path, raw_ws: &Path) -> bool {
    let candidate_path = Path::new(candidate);
    let candidate_canon = canonicalize_or_self(candidate_path);
    candidate_canon == canonical_ws || candidate_path == raw_ws
}

fn truncate_preview(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(80).collect())
    }
}

fn collect_files_with_ext(root: &Path, ext: &str, max_depth: usize) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() && depth < max_depth {
                stack.push((path, depth + 1));
            } else if ft.is_file() && path.extension().is_some_and(|e| e == ext) {
                results.push(path);
            }
        }
    }
    results
}

fn encode_claude_project_slug(path: &Path, normalize_punct: bool) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '?') || (normalize_punct && matches!(c, '.' | '_')) {
                '-'
            } else {
                c
            }
        })
        .collect()
}

fn extract_claude_message_text(val: &serde_json::Value) -> Option<String> {
    let content = val.get("message").and_then(|m| m.get("content"))?;
    if let Some(s) = content.as_str() {
        return truncate_preview(s);
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if let Some(text) = item.get("text").and_then(serde_json::Value::as_str)
                && let Some(preview) = truncate_preview(text)
            {
                return Some(preview);
            }
        }
    }
    None
}

fn parse_claude_jsonl_session(
    path: &Path,
    agent_name: &str,
    canonical_ws: &Path,
    raw_ws: &Path,
) -> Option<SessionMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut session_id: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    let mut declared_agent: Option<String> = None;
    let mut summary_preview: Option<String> = None;
    let mut user_preview: Option<String> = None;
    let mut step_count: u64 = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if let Some(cwd_str) = val.get("cwd").and_then(serde_json::Value::as_str)
            && !workspace_matches_path(cwd_str, canonical_ws, raw_ws)
        {
            return None;
        }

        if let Some(sid) = val
            .get("sessionId")
            .or_else(|| val.get("session_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            session_id = Some(sid.to_owned());
        }

        if let Some(da) = val
            .get("agent")
            .or_else(|| val.get("agent_name"))
            .or_else(|| val.get("agentName"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            declared_agent = Some(da.to_owned());
        }

        if let Some(ts) = val
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            last_timestamp = Some(ts.to_owned());
        }

        let entry_type = val.get("type").and_then(serde_json::Value::as_str);
        if entry_type == Some("summary") {
            if let Some(s) = val
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .and_then(truncate_preview)
            {
                summary_preview = Some(s);
            }
        } else if matches!(entry_type, Some("user" | "assistant")) {
            step_count += 1;
            if entry_type == Some("user") && user_preview.is_none() {
                user_preview = extract_claude_message_text(&val);
            }
        }
    }

    if step_count == 0 {
        return None;
    }

    if let Some(da) = declared_agent
        && !da.eq_ignore_ascii_case(agent_name)
    {
        return None;
    }

    Some(SessionMeta {
        conversation_id: session_id.unwrap_or_else(|| file_stem_or_unknown(path)),
        agent_name: agent_name.to_owned(),
        workspace_uris: canonical_ws.to_string_lossy().into_owned(),
        last_modified_time: last_timestamp.unwrap_or_else(|| file_mtime_string(path)),
        step_count,
        preview: summary_preview.or(user_preview).unwrap_or_default(),
    })
}

/// Discovers Claude Code JSONL transcript sessions under `projects_dir`
/// matching `workspace_root`, sorted newest first.
#[must_use]
pub fn discover_claude_sessions_in(
    projects_dir: &Path,
    agent_name: &str,
    workspace_root: &Path,
) -> Vec<SessionMeta> {
    if !projects_dir.is_dir() {
        return Vec::new();
    }
    let canonical = canonicalize_or_self(workspace_root);
    let mut candidate_dirs = Vec::new();
    for base in [&canonical, workspace_root] {
        for normalize_punct in [false, true] {
            let dir = projects_dir.join(encode_claude_project_slug(base, normalize_punct));
            if dir.is_dir() && !candidate_dirs.contains(&dir) {
                candidate_dirs.push(dir);
            }
        }
    }

    let mut sessions = Vec::new();
    for dir in candidate_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(sess) =
                    parse_claude_jsonl_session(&path, agent_name, &canonical, workspace_root)
            {
                sessions.push(sess);
            }
        }
    }

    sessions.sort_by(|a, b| b.last_modified_time.cmp(&a.last_modified_time));
    sessions
}

fn parse_codex_rollout_session(
    path: &Path,
    agent_name: &str,
    canonical_ws: &Path,
    raw_ws: &Path,
) -> Option<SessionMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut session_id: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    let mut preview: Option<String> = None;
    let mut matched_workspace = false;
    let mut step_count: u64 = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = val
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            last_timestamp = Some(ts.to_owned());
        }

        let entry_type = val.get("type").and_then(serde_json::Value::as_str);
        let payload = val.get("payload");
        if entry_type == Some("session_meta")
            && let Some(p) = payload
        {
            if let Some(cwd_str) = p.get("cwd").and_then(serde_json::Value::as_str) {
                if !workspace_matches_path(cwd_str, canonical_ws, raw_ws) {
                    return None;
                }
                matched_workspace = true;
            }
            if let Some(id) = p
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                session_id = Some(id.to_owned());
            }
        } else if matches!(entry_type, Some("event_msg" | "response_item")) {
            step_count += 1;
            if preview.is_none()
                && let Some(p) = payload
            {
                if p.get("type").and_then(serde_json::Value::as_str) == Some("user_message")
                    && let Some(msg) = p.get("message").and_then(serde_json::Value::as_str)
                {
                    preview = truncate_preview(msg);
                } else if p.get("role").and_then(serde_json::Value::as_str) == Some("user")
                    && let Some(arr) = p.get("content").and_then(serde_json::Value::as_array)
                {
                    for item in arr {
                        if let Some(text) = item.get("text").and_then(serde_json::Value::as_str)
                            && let Some(p_text) = truncate_preview(text)
                        {
                            preview = Some(p_text);
                            break;
                        }
                    }
                }
            }
        }
    }

    if !matched_workspace || step_count == 0 {
        return None;
    }

    Some(SessionMeta {
        conversation_id: session_id.unwrap_or_else(|| file_stem_or_unknown(path)),
        agent_name: agent_name.to_owned(),
        workspace_uris: canonical_ws.to_string_lossy().into_owned(),
        last_modified_time: last_timestamp.unwrap_or_else(|| file_mtime_string(path)),
        step_count,
        preview: preview.unwrap_or_default(),
    })
}

/// Discovers `OpenAI` Codex CLI rollout JSONL sessions under `sessions_dir`
/// (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`), sorted newest first.
#[must_use]
pub fn discover_codex_sessions_in(
    sessions_dir: &Path,
    agent_name: &str,
    workspace_root: &Path,
) -> Vec<SessionMeta> {
    if !sessions_dir.is_dir() {
        return Vec::new();
    }
    let canonical = canonicalize_or_self(workspace_root);
    let mut sessions: Vec<_> = collect_files_with_ext(sessions_dir, "jsonl", 5)
        .into_iter()
        .filter_map(|p| parse_codex_rollout_session(&p, agent_name, &canonical, workspace_root))
        .collect();
    sessions.sort_by(|a, b| b.last_modified_time.cmp(&a.last_modified_time));
    sessions
}

fn format_timestamp_value(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    } else if let Some(n) = val.as_u64() {
        return Some(n.to_string());
    }
    None
}

fn parse_cursor_jsonl_session(
    path: &Path,
    agent_name: &str,
    canonical_ws: &Path,
) -> Option<SessionMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut step_count: u64 = 0;
    let mut last_timestamp: Option<String> = None;
    let mut preview: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = val.get("timestamp").and_then(format_timestamp_value) {
            last_timestamp = Some(ts);
        }

        let role = val.get("role").and_then(serde_json::Value::as_str);
        if matches!(role, Some("user" | "assistant")) {
            step_count += 1;
            if role == Some("user") && preview.is_none() {
                preview = extract_claude_message_text(&val).or_else(|| {
                    val.get("text")
                        .and_then(serde_json::Value::as_str)
                        .and_then(truncate_preview)
                });
            }
        }
    }

    if step_count == 0 {
        return None;
    }

    Some(SessionMeta {
        conversation_id: file_stem_or_unknown(path),
        agent_name: agent_name.to_owned(),
        workspace_uris: canonical_ws.to_string_lossy().into_owned(),
        last_modified_time: last_timestamp.unwrap_or_else(|| file_mtime_string(path)),
        step_count,
        preview: preview.unwrap_or_default(),
    })
}

/// Discovers Cursor JSONL agent transcripts under `projects_dir`
/// (`~/.cursor/projects/<encoded-workspace>/agent-transcripts/*.jsonl`),
/// sorted newest first.
#[must_use]
pub fn discover_cursor_sessions_in(
    projects_dir: &Path,
    agent_name: &str,
    workspace_root: &Path,
) -> Vec<SessionMeta> {
    if !projects_dir.is_dir() {
        return Vec::new();
    }
    let canonical = canonicalize_or_self(workspace_root);
    let mut candidate_dirs = Vec::new();
    for base in [&canonical, workspace_root] {
        for normalize_punct in [false, true] {
            let raw_slug = encode_claude_project_slug(base, normalize_punct);
            for slug in [raw_slug.trim_start_matches('-').to_owned(), raw_slug] {
                let dir = projects_dir.join(slug).join("agent-transcripts");
                if dir.is_dir() && !candidate_dirs.contains(&dir) {
                    candidate_dirs.push(dir);
                }
            }
        }
    }

    let mut sessions = Vec::new();
    for dir in candidate_dirs {
        for path in collect_files_with_ext(&dir, "jsonl", 2) {
            if let Some(sess) = parse_cursor_jsonl_session(&path, agent_name, &canonical) {
                sessions.push(sess);
            }
        }
    }
    sessions.sort_by(|a, b| b.last_modified_time.cmp(&a.last_modified_time));
    sessions
}

fn parse_gemini_chat_session(
    path: &Path,
    agent_name: &str,
    canonical_ws: &Path,
    raw_ws: &Path,
    dir_project_root: Option<&str>,
) -> Option<SessionMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;

    let root_hint = val
        .get("cwd")
        .or_else(|| val.get("projectRoot"))
        .and_then(serde_json::Value::as_str)
        .or(dir_project_root);
    let root_str = root_hint?;
    if !workspace_matches_path(root_str, canonical_ws, raw_ws) {
        return None;
    }

    let conversation_id = val
        .get("sessionId")
        .or_else(|| val.get("session_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| file_stem_or_unknown(path), ToOwned::to_owned);

    let messages = val.get("messages").and_then(serde_json::Value::as_array)?;
    let step_count = u64::try_from(messages.len()).unwrap_or(0);
    if step_count == 0 {
        return None;
    }

    let preview = messages
        .iter()
        .find(|m| {
            matches!(
                m.get("type")
                    .or_else(|| m.get("role"))
                    .and_then(serde_json::Value::as_str),
                Some("user")
            )
        })
        .and_then(|m| {
            m.get("content")
                .and_then(serde_json::Value::as_str)
                .and_then(truncate_preview)
                .or_else(|| {
                    m.get("parts")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|parts| {
                            parts.iter().find_map(|p| {
                                p.get("text")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(truncate_preview)
                            })
                        })
                })
        })
        .unwrap_or_default();

    let last_modified_time = val
        .get("lastUpdated")
        .or_else(|| val.get("updatedAt"))
        .or_else(|| val.get("startTime"))
        .or_else(|| val.get("timestamp"))
        .and_then(format_timestamp_value)
        .unwrap_or_else(|| file_mtime_string(path));

    Some(SessionMeta {
        conversation_id,
        agent_name: agent_name.to_owned(),
        workspace_uris: canonical_ws.to_string_lossy().into_owned(),
        last_modified_time,
        step_count,
        preview,
    })
}

/// Discovers Google Gemini CLI JSON sessions under `tmp_dir`
/// (`~/.gemini/tmp/<project_hash>/chats/session-*.json`), sorted newest first.
#[must_use]
pub fn discover_gemini_sessions_in(
    tmp_dir: &Path,
    agent_name: &str,
    workspace_root: &Path,
) -> Vec<SessionMeta> {
    if !tmp_dir.is_dir() {
        return Vec::new();
    }
    let canonical = canonicalize_or_self(workspace_root);
    let mut sessions = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(tmp_dir) else {
        return Vec::new();
    };
    for entry in project_dirs.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let project_root_hint = std::fs::read_to_string(project_dir.join(".project_root"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let chats_dir = project_dir.join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        for chat_path in collect_files_with_ext(&chats_dir, "json", 1) {
            if let Some(sess) = parse_gemini_chat_session(
                &chat_path,
                agent_name,
                &canonical,
                workspace_root,
                project_root_hint.as_deref(),
            ) {
                sessions.push(sess);
            }
        }
    }
    sessions.sort_by(|a, b| b.last_modified_time.cmp(&a.last_modified_time));
    sessions
}

fn parse_json_session(
    val: &serde_json::Value,
    path: &Path,
    agent_name: &str,
) -> Option<SessionMeta> {
    let declared_agent = val
        .get("agent")
        .or_else(|| val.get("agent_name"))
        .or_else(|| val.get("agentName"))
        .and_then(serde_json::Value::as_str);

    if let Some(da) = declared_agent
        && !da.trim().is_empty()
        && !da.eq_ignore_ascii_case(agent_name)
    {
        return None;
    }

    let conversation_id = val
        .get("id")
        .or_else(|| val.get("session_id"))
        .or_else(|| val.get("sessionId"))
        .or_else(|| val.get("conversation_id"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_owned()
            },
            ToOwned::to_owned,
        );

    let last_modified_time = val
        .get("updated_at")
        .or_else(|| val.get("updatedAt"))
        .or_else(|| val.get("last_modified_time"))
        .or_else(|| val.get("timestamp"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || {
                path.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map_or_else(
                        || "1970-01-01T00:00:00Z".to_owned(),
                        |t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                                .to_string()
                        },
                    )
            },
            ToOwned::to_owned,
        );

    let step_count = val
        .get("step_count")
        .or_else(|| val.get("stepCount"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            let arr = val
                .get("messages")
                .or_else(|| val.get("turns"))
                .or_else(|| val.get("steps"))
                .and_then(serde_json::Value::as_array)?;
            u64::try_from(arr.len()).ok()
        })
        .unwrap_or(1);

    if step_count == 0 {
        return None;
    }

    let preview = val
        .get("preview")
        .or_else(|| val.get("title"))
        .or_else(|| val.get("summary"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();

    Some(SessionMeta {
        conversation_id,
        agent_name: agent_name.to_owned(),
        workspace_uris: String::new(),
        last_modified_time,
        step_count,
        preview,
    })
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use std::{
        collections::HashMap,
        fs::File,
        io::{BufRead, BufReader},
        path::{Path, PathBuf},
    };

    use serde::Deserialize;

    use super::SessionMeta;

    #[derive(Deserialize)]
    struct HistoryEntry {
        #[serde(rename = "conversationId")]
        conversation_id: Option<String>,
        workspace: Option<PathBuf>,
    }

    fn load_history_workspaces(history_path: &Path) -> HashMap<String, PathBuf> {
        let mut map = HashMap::new();
        let Ok(file) = File::open(history_path) else {
            return map;
        };
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(entry) = serde_json::from_str::<HistoryEntry>(&line)
                && let (Some(cid), Some(ws)) = (entry.conversation_id, entry.workspace)
            {
                map.insert(cid, ws);
            }
        }
        map
    }

    fn workspace_matches(ws_a: &Path, ws_b: &Path) -> bool {
        if ws_a == ws_b {
            return true;
        }
        let canon_a = ws_a.canonicalize().unwrap_or_else(|_| ws_a.to_path_buf());
        let canon_b = ws_b.canonicalize().unwrap_or_else(|_| ws_b.to_path_buf());
        canon_a == canon_b || canon_a.starts_with(&canon_b) || canon_b.starts_with(&canon_a)
    }

    fn session_matches_workspace(
        conn: &rusqlite::Connection,
        row: &SessionMeta,
        workspace_root: &Path,
        ws_canonical: &Path,
        history_map: &HashMap<String, PathBuf>,
    ) -> bool {
        if !row.workspace_uris.is_empty() {
            let ws_str = ws_canonical.to_string_lossy();
            if row.workspace_uris.contains(&*ws_str) {
                return true;
            }
            let root_str = workspace_root.to_string_lossy();
            if row.workspace_uris.contains(&*root_str) {
                return true;
            }
        }

        if let Some(hist_ws) = history_map.get(&row.conversation_id)
            && workspace_matches(hist_ws, ws_canonical)
        {
            return true;
        }

        if let Ok(mut fts_stmt) = conn.prepare_cached(
            "SELECT workspace FROM conversations_fts WHERE conversation_id = ?1 LIMIT 1;",
        ) {
            let fts_ws: Option<String> = fts_stmt
                .query_row(rusqlite::params![row.conversation_id], |r| r.get(0))
                .ok();
            if let Some(ws) = fts_ws
                && !ws.is_empty()
            {
                let ws_path = Path::new(&ws);
                if workspace_matches(ws_path, ws_canonical) {
                    return true;
                }
                if let Some(ws_name) = ws_canonical.file_name().and_then(|n| n.to_str())
                    && ws == ws_name
                {
                    return true;
                }
            }
        }

        false
    }

    /// Queries Antigravity's `conversation_summaries.db` and `history.jsonl`
    /// for resumable sessions matching `agent_name` and `workspace_root`.
    #[must_use]
    pub fn query_session_from_sources(
        db_path: &Path,
        convs_dir: &Path,
        history_path: &Path,
        agent_name: &str,
        workspace_root: &Path,
    ) -> Vec<SessionMeta> {
        if !db_path.is_file() {
            return Vec::new();
        }

        let ws_canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let history_map = load_history_workspaces(history_path);

        let Ok(conn) = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return Vec::new();
        };

        let Ok(mut stmt) = conn.prepare(
            "SELECT conversation_id, agent_name, workspace_uris, last_modified_time, step_count, preview \
             FROM conversation_summaries \
             WHERE LOWER(agent_name) = LOWER(?1) AND step_count > 0 \
             ORDER BY last_modified_time DESC LIMIT 20;",
        ) else {
            return Vec::new();
        };

        let Ok(rows) = stmt.query_map(rusqlite::params![agent_name], |row| {
            Ok(SessionMeta {
                conversation_id: row.get(0)?,
                agent_name: row.get(1)?,
                workspace_uris: row.get(2)?,
                last_modified_time: row.get(3)?,
                step_count: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                preview: row.get(5)?,
            })
        }) else {
            return Vec::new();
        };

        let mut matches = Vec::new();
        for row in rows.flatten() {
            let conv_file = convs_dir.join(format!("{}.db", row.conversation_id));
            if !conv_file.is_file() {
                continue;
            }
            if session_matches_workspace(&conn, &row, workspace_root, &ws_canonical, &history_map) {
                matches.push(row);
            }
        }
        matches
    }
}

#[cfg(feature = "sqlite")]
pub use sqlite::query_session_from_sources;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_id() {
        let summary = SessionMeta {
            conversation_id: "6030b881-9107-4730-a943-fe383688402a".to_owned(),
            agent_name: "mock_agent".to_owned(),
            workspace_uris: "[]".to_owned(),
            last_modified_time: "2026-11-02".to_owned(),
            step_count: 10,
            preview: "Implement feature".to_owned(),
        };
        assert_eq!(summary.short_id(), "6030b881");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_rusqlite_session_discovery_via_workspace_uris() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-session-uris-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("workspace_a");
        let gemini_dir = temp_dir.join(".gemini/antigravity-cli");
        let convs_dir = gemini_dir.join("conversations");
        std::fs::create_dir_all(&convs_dir).unwrap();
        std::fs::create_dir_all(&mock_ws).unwrap();

        let db_path = gemini_dir.join("conversation_summaries.db");
        let history_path = gemini_dir.join("history.jsonl");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE conversation_summaries (
                conversation_id TEXT PRIMARY KEY,
                agent_name TEXT,
                workspace_uris TEXT,
                last_modified_time TEXT,
                step_count INTEGER,
                preview TEXT
            )",
            [],
        )
        .unwrap();

        let uris_json = format!("[\"{}\"]", mock_ws.display());
        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "test-uuid-1234",
                "mock_agent",
                uris_json,
                "2026-11-07T00:00:00Z",
                5,
                "Testing session"
            ],
        )
        .unwrap();
        std::fs::write(convs_dir.join("test-uuid-1234.db"), b"").unwrap();

        let res =
            query_session_from_sources(&db_path, &convs_dir, &history_path, "mock_agent", &mock_ws);
        assert_eq!(res.len(), 1);
        let first = res.first().unwrap();
        assert_eq!(first.conversation_id, "test-uuid-1234");
        assert_eq!(first.step_count, 5);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_rusqlite_session_discovery_via_history_jsonl() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-session-hist-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("cli_project");
        let other_ws = temp_dir.join("other_project");
        let gemini_dir = temp_dir.join(".gemini/antigravity-cli");
        let convs_dir = gemini_dir.join("conversations");
        std::fs::create_dir_all(&convs_dir).unwrap();
        std::fs::create_dir_all(&mock_ws).unwrap();
        std::fs::create_dir_all(&other_ws).unwrap();

        let db_path = gemini_dir.join("conversation_summaries.db");
        let history_path = gemini_dir.join("history.jsonl");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE conversation_summaries (
                conversation_id TEXT PRIMARY KEY,
                agent_name TEXT,
                workspace_uris TEXT,
                last_modified_time TEXT,
                step_count INTEGER,
                preview TEXT
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "cli-uuid-5678",
                "copilot",
                "",
                "2026-11-07T23:00:00Z",
                47,
                "Setup Test And Capabilities"
            ],
        )
        .unwrap();

        let hist_entry = serde_json::json!({
            "conversationId": "cli-uuid-5678",
            "workspace": mock_ws.display().to_string(),
            "display": "Test prompt"
        });
        std::fs::write(&history_path, format!("{hist_entry}\n")).unwrap();
        std::fs::write(convs_dir.join("cli-uuid-5678.db"), b"").unwrap();

        let res =
            query_session_from_sources(&db_path, &convs_dir, &history_path, "copilot", &mock_ws);
        assert_eq!(res.len(), 1);
        let first = res.first().unwrap();
        assert_eq!(first.conversation_id, "cli-uuid-5678");
        assert_eq!(first.step_count, 47);

        let res_none =
            query_session_from_sources(&db_path, &convs_dir, &history_path, "copilot", &other_ws);
        assert_eq!(res_none.len(), 0);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_rusqlite_session_discovery_ignores_zero_steps() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-session-zero-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("zero_project");
        let gemini_dir = temp_dir.join(".gemini/antigravity-cli");
        let convs_dir = gemini_dir.join("conversations");
        std::fs::create_dir_all(&convs_dir).unwrap();
        std::fs::create_dir_all(&mock_ws).unwrap();

        let db_path = gemini_dir.join("conversation_summaries.db");
        let history_path = gemini_dir.join("history.jsonl");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE conversation_summaries (
                conversation_id TEXT PRIMARY KEY,
                agent_name TEXT,
                workspace_uris TEXT,
                last_modified_time TEXT,
                step_count INTEGER,
                preview TEXT
            )",
            [],
        )
        .unwrap();

        let uris_json = format!("[\"{}\"]", mock_ws.display());
        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "zero-uuid-9999",
                "copilot",
                uris_json,
                "2026-11-07T23:00:00Z",
                0,
                ""
            ],
        )
        .unwrap();
        std::fs::write(convs_dir.join("zero-uuid-9999.db"), b"").unwrap();

        let res =
            query_session_from_sources(&db_path, &convs_dir, &history_path, "copilot", &mock_ws);
        assert_eq!(res.len(), 0);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_rusqlite_session_discovery_subdirectory_matching() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-session-sub-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("root_project");
        let sub_ws = mock_ws.join("crates").join("my_crate");
        let gemini_dir = temp_dir.join(".gemini/antigravity-cli");
        let convs_dir = gemini_dir.join("conversations");
        std::fs::create_dir_all(&convs_dir).unwrap();
        std::fs::create_dir_all(&sub_ws).unwrap();

        let db_path = gemini_dir.join("conversation_summaries.db");
        let history_path = gemini_dir.join("history.jsonl");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE conversation_summaries (
                conversation_id TEXT PRIMARY KEY,
                agent_name TEXT,
                workspace_uris TEXT,
                last_modified_time TEXT,
                step_count INTEGER,
                preview TEXT
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO conversation_summaries VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "sub-uuid-7777",
                "coder",
                "",
                "2026-11-07T23:30:00Z",
                12,
                "Subdirectory turn"
            ],
        )
        .unwrap();

        let hist_entry = serde_json::json!({
            "conversationId": "sub-uuid-7777",
            "workspace": sub_ws.display().to_string(),
            "display": "Subdirectory prompt"
        });
        std::fs::write(&history_path, format!("{hist_entry}\n")).unwrap();
        std::fs::write(convs_dir.join("sub-uuid-7777.db"), b"").unwrap();

        let res =
            query_session_from_sources(&db_path, &convs_dir, &history_path, "coder", &mock_ws);
        assert_eq!(res.len(), 1);
        let first = res.first().unwrap();
        assert_eq!(first.conversation_id, "sub-uuid-7777");
        assert_eq!(first.step_count, 12);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_opencode_session_discovery() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-opencode-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("opencode_workspace");
        let opencode_sessions = mock_ws.join(".opencode").join("sessions");
        std::fs::create_dir_all(&opencode_sessions).unwrap();

        let session_data = serde_json::json!({
            "id": "opencode-ses-1234",
            "agent": "coder",
            "updatedAt": "2026-11-08T18:00:00Z",
            "title": "Fix buffer overflow in parser",
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"}
            ]
        });

        std::fs::write(
            opencode_sessions.join("ses_1234.json"),
            session_data.to_string(),
        )
        .unwrap();

        let res = discover_opencode_sessions("coder", &mock_ws);
        assert_eq!(res.len(), 1);
        let first = res.first().unwrap();
        assert_eq!(first.conversation_id, "opencode-ses-1234");
        assert_eq!(first.step_count, 2);

        let diff_res = discover_opencode_sessions("reviewer", &mock_ws);
        assert_eq!(diff_res.len(), 0);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_generic_state_session_discovery() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-generic-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("generic_workspace");
        let state_sessions = mock_ws.join(".tkg").join("state").join("sessions");
        std::fs::create_dir_all(&state_sessions).unwrap();

        let session_data = serde_json::json!({
            "conversation_id": "gen-session-999",
            "agent_name": "planner",
            "last_modified_time": "2026-11-08T19:00:00Z",
            "step_count": 5,
            "preview": "Architecture roadmap review"
        });

        std::fs::write(
            state_sessions.join("gen_999.json"),
            session_data.to_string(),
        )
        .unwrap();

        let res = discover_generic_state_sessions("planner", &mock_ws);
        assert_eq!(res.len(), 1);
        let first = res.first().unwrap();
        assert_eq!(first.conversation_id, "gen-session-999");
        assert_eq!(first.step_count, 5);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_claude_session_discovery() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tkg-common-test-claude-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mock_ws = temp_dir.join("ws.project");
        let projects_dir = temp_dir.join("projects");
        std::fs::create_dir_all(&mock_ws).unwrap();
        let slug = encode_claude_project_slug(&mock_ws.canonicalize().unwrap(), true);
        let project_sessions = projects_dir.join(slug);
        std::fs::create_dir_all(&project_sessions).unwrap();

        let lines = [
            serde_json::json!({
                "type": "user",
                "sessionId": "claude-ses-42",
                "cwd": mock_ws,
                "timestamp": "2026-11-15T10:00:00Z",
                "message": { "role": "user", "content": "Refactor harness detection" }
            })
            .to_string(),
            serde_json::json!({
                "type": "assistant",
                "sessionId": "claude-ses-42",
                "cwd": mock_ws,
                "timestamp": "2026-11-15T10:05:00Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "Done" }] }
            })
            .to_string(),
            serde_json::json!({
                "type": "summary",
                "summary": "Harness detection refactored"
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(project_sessions.join("claude-ses-42.jsonl"), lines).unwrap();

        let sessions = discover_claude_sessions_in(&projects_dir, "coder", &mock_ws);
        assert_eq!(sessions.len(), 1);
        let first = sessions.first().unwrap();
        assert_eq!(first.conversation_id, "claude-ses-42");
        assert_eq!(first.step_count, 2);
        assert_eq!(first.preview, "Harness detection refactored");

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_encode_claude_project_slug_windows_safe() {
        let verbatim = Path::new(r"\\?\C:\Users\runner\ws.project");
        let slug = encode_claude_project_slug(verbatim, true);
        assert!(!slug.contains(':'));
        assert!(!slug.contains('?'));
        assert_eq!(slug, "C--Users-runner-ws-project");
    }

    #[test]
    fn test_discover_codex_sessions_in_filters_workspace() {
        let temp_dir = std::env::temp_dir().join(format!("tkg_codex_ses_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_dir);
        let mock_ws = temp_dir.join("repo_a");
        let other_ws = temp_dir.join("repo_b");
        std::fs::create_dir_all(&mock_ws).unwrap();
        std::fs::create_dir_all(&other_ws).unwrap();

        let day_dir = temp_dir.join("sessions/2026/11/15");
        std::fs::create_dir_all(&day_dir).unwrap();

        let match_lines = [
            serde_json::json!({
                "timestamp": "2026-11-15T12:00:00Z",
                "type": "session_meta",
                "payload": { "id": "codex-001", "cwd": mock_ws }
            })
            .to_string(),
            serde_json::json!({
                "timestamp": "2026-11-15T12:01:00Z",
                "type": "event_msg",
                "payload": { "type": "user_message", "message": "Implement Codex integration" }
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(
            day_dir.join("rollout-2026-11-15-codex-001.jsonl"),
            match_lines,
        )
        .unwrap();

        let other_lines = serde_json::json!({
            "timestamp": "2026-11-15T12:02:00Z",
            "type": "session_meta",
            "payload": { "id": "codex-002", "cwd": other_ws }
        })
        .to_string();
        std::fs::write(day_dir.join("rollout-other.jsonl"), other_lines).unwrap();

        let sessions = discover_codex_sessions_in(&temp_dir.join("sessions"), "coder", &mock_ws);
        assert_eq!(sessions.len(), 1);
        let first = sessions.first().unwrap();
        assert_eq!(first.conversation_id, "codex-001");
        assert_eq!(first.preview, "Implement Codex integration");
        assert_eq!(first.step_count, 1);

        _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_cursor_and_gemini_sessions_in() {
        let temp_dir = std::env::temp_dir().join(format!("tkg_cur_gem_ses_{}", std::process::id()));
        _ = std::fs::remove_dir_all(&temp_dir);
        let mock_ws = temp_dir.join("ws");
        std::fs::create_dir_all(&mock_ws).unwrap();

        let projects_dir = temp_dir.join("cursor_projects");
        let raw_slug = encode_claude_project_slug(&mock_ws.canonicalize().unwrap(), true);
        let cursor_dir = projects_dir
            .join(raw_slug.trim_start_matches('-'))
            .join("agent-transcripts");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        let cursor_lines = [
            serde_json::json!({
                "role": "user",
                "message": { "content": "Upgrade Cursor support" },
                "timestamp": 1_747_000_000_u64
            })
            .to_string(),
            serde_json::json!({
                "role": "assistant",
                "message": { "content": "Done" },
                "timestamp": 1_747_000_010_u64
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(cursor_dir.join("cur-uuid-9.jsonl"), cursor_lines).unwrap();

        let cursor_sessions = discover_cursor_sessions_in(&projects_dir, "coder", &mock_ws);
        assert_eq!(cursor_sessions.len(), 1);
        let c0 = cursor_sessions.first().unwrap();
        assert_eq!(c0.conversation_id, "cur-uuid-9");
        assert_eq!(c0.step_count, 2);
        assert_eq!(c0.preview, "Upgrade Cursor support");

        let gem_tmp = temp_dir.join("gemini_tmp");
        let gem_chats = gem_tmp.join("hash123/chats");
        std::fs::create_dir_all(&gem_chats).unwrap();
        std::fs::write(
            gem_tmp.join("hash123/.project_root"),
            mock_ws.to_string_lossy().as_bytes(),
        )
        .unwrap();
        std::fs::write(
            gem_chats.join("session-2026-11-15.json"),
            serde_json::json!({
                "sessionId": "gem-uuid-7",
                "lastUpdated": "2026-11-15T14:00:00Z",
                "messages": [
                    { "type": "user", "content": "Add Gemini hooks" },
                    { "type": "gemini", "content": "Adding hooks" }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let gem_sessions = discover_gemini_sessions_in(&gem_tmp, "coder", &mock_ws);
        assert_eq!(gem_sessions.len(), 1);
        let g0 = gem_sessions.first().unwrap();
        assert_eq!(g0.conversation_id, "gem-uuid-7");
        assert_eq!(g0.step_count, 2);
        assert_eq!(g0.preview, "Add Gemini hooks");

        _ = std::fs::remove_dir_all(&temp_dir);
    }
}
