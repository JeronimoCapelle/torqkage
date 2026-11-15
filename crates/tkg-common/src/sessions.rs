//! Multi-harness conversation session discovery across SQLite (`agy`),
//! Claude Code JSONL transcripts, `OpenCode` JSON files, and
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
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.chars().take(80).collect());
        }
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.chars().take(80).collect());
                }
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

        if let Some(cwd_str) = val.get("cwd").and_then(serde_json::Value::as_str) {
            let cwd_path = Path::new(cwd_str);
            let cwd_canon = cwd_path
                .canonicalize()
                .unwrap_or_else(|_| cwd_path.to_path_buf());
            if cwd_canon != canonical_ws && cwd_path != raw_ws {
                return None;
            }
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
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                summary_preview = Some(s.chars().take(80).collect());
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

    let conversation_id = session_id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_owned()
    });

    let last_modified_time = last_timestamp.unwrap_or_else(|| {
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
    });

    Some(SessionMeta {
        conversation_id,
        agent_name: agent_name.to_owned(),
        workspace_uris: canonical_ws.to_string_lossy().into_owned(),
        last_modified_time,
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
    let canonical = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
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
}
