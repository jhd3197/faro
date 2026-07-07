//! Built-in Agent chat — calls Anthropic's API with the Faro bridge tools so
//! the user can ask natural-language questions about their servers.
//!
//! This is intentionally simple: one request/response pair with optional tool
//! calls. It reuses the same `faro_*` primitives exposed over MCP/REST.

use crate::bridge::{
    exec_on, op_context, op_glob, op_list_dir, op_read_file, op_read_file_batch,
    op_run_command, op_search, op_server_info, op_tail, BridgeState,
};
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL: &str = "claude-3-5-sonnet-20241022";
const MAX_TOKENS: u32 = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub api_key: String,
    pub session_id: Option<String>,
    pub prompt: String,
    pub history: Vec<HistoryTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub content: String,
}

/// Main entry point from the Tauri command.
pub async fn agent_chat(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    req: ChatRequest,
) -> Result<ChatResponse> {
    if req.api_key.trim().is_empty() {
        bail!("Anthropic API key is required. Add it in Settings → Agent.");
    }

    let mut messages = Vec::new();
    messages.push(json!({
        "role": "user",
        "content": build_system_prompt(app, state).await
    }));
    for h in &req.history {
        messages.push(json!({ "role": h.role, "content": h.content }));
    }
    messages.push(json!({ "role": "user", "content": req.prompt }));

    let client = reqwest::Client::new();
    let tools = anthropic_tools();

    // Simple loop: allow up to 5 tool-use turns before giving up.
    let mut tool_turns = 0;
    loop {
        if tool_turns > 5 {
            return Ok(ChatResponse {
                content: "I made several tool calls but didn't reach a final answer. Try rephrasing your request.".into(),
            });
        }
        tool_turns += 1;

        let body = json!({
            "model": MODEL,
            "max_tokens": MAX_TOKENS,
            "system": "You are Faro's built-in agent. Help the user with their own servers and files through Faro. Prefer read-only inspection first, explain what you're checking, and avoid destructive operations unless the user clearly asked.",
            "messages": messages,
            "tools": tools,
        });

        let res = client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &req.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!("Anthropic API error: {text}");
        }

        let payload: Value = res.json().await?;
        let content = payload
            .get("content")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Collect any text and any tool_use requests.
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_uses: Vec<(String, String, Value)> = Vec::new(); // (id, name, input)
        for block in &content {
            if let Some(t) = block.get("type").and_then(|v| v.as_str()) {
                if t == "text" {
                    if let Some(txt) = block.get("text").and_then(|v| v.as_str()) {
                        text_parts.push(txt.to_string());
                    }
                } else if t == "tool_use" {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    tool_uses.push((id, name, input));
                }
            }
        }

        // If there are no tool calls, return the text response.
        if tool_uses.is_empty() {
            return Ok(ChatResponse {
                content: text_parts.join("\n"),
            });
        }

        // Append assistant's message (text + tool_use blocks).
        messages.push(json!({
            "role": "assistant",
            "content": content
        }));

        // Execute each requested tool and append tool_result blocks.
        for (id, name, input) in tool_uses {
            let result = run_tool(app, state, &req.session_id, &name, &input).await;
            let result_content = match result {
                Ok(v) => json!({ "type": "text", "text": v.to_string() }),
                Err(e) => json!({ "type": "text", "text": format!("Error: {e}") }),
            };
            messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": result_content
                }]
            }));
        }
    }
}

async fn build_system_prompt(app: &AppHandle, state: &Arc<BridgeState>) -> String {
    let ctx = op_context(app, state).await;
    let (_, ctx_body) = ctx;
    let running = ctx_body
        .get("bridgeRunning")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !running {
        return "Faro's Agent Bridge is currently off. Tell the user to open Faro → Agent Bridge and turn it on.".into();
    }
    format!(
        "Here is the current Faro context so you know what's available. Respond to the user's request using the tools when needed.\n\n{}",
        serde_json::to_string_pretty(&ctx_body).unwrap_or_default()
    )
}

fn anthropic_tools() -> Vec<Value> {
    let session_param = json!({
        "type": "string",
        "description": "Session id or name. Optional when only one session is available."
    });
    vec![
        json!({
            "name": "faro_context",
            "description": "Get the agent's overall context in Faro: bridge state, policy, enabled connections, saved commands, and active session.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "faro_server_info",
            "description": "Get context about a connected server: protocol, host, port, username, default path, and remote context (home, cwd, shell, OS) for SSH.",
            "input_schema": {
                "type": "object",
                "properties": { "session": session_param.clone() },
                "required": ["session"]
            }
        }),
        json!({
            "name": "faro_exec",
            "description": "Run a diagnostic/status command on an SSH server or a paired Faro Agent machine. Prefer read-only commands. Set dryRun=true to preview approval. `sudo` is supported when the user enabled it — write `sudo <cmd>` normally (Faro answers the password prompt); never add `-S` or pipe in a password.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "dryRun": { "type": "boolean" },
                    "timeoutMs": { "type": "integer", "description": "Optional timeout in ms (default 60000, clamped to 1000–900000)." },
                    "session": session_param.clone()
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "faro_list_dir",
            "description": "List a directory on a connected server.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "session": session_param.clone()
                }
            }
        }),
        json!({
            "name": "faro_read_file",
            "description": "Read a text file from a connected SSH/SFTP server (capped at 256 KiB).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "session": session_param.clone()
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "faro_read_files_batch",
            "description": "Read several text files from a connected SSH/SFTP server in one call.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "session": session_param.clone()
                },
                "required": ["paths"]
            }
        }),
        json!({
            "name": "faro_search",
            "description": "Find files/directories whose names contain a substring, recursively.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" },
                    "session": session_param.clone()
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "faro_glob",
            "description": "Find files matching a glob pattern (e.g. '*.log'), recursively. SSH only.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "session": session_param.clone()
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "faro_tail",
            "description": "Stream the tail of a log file for up to 30 seconds. SSH only.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "lines": { "type": "integer" },
                    "session": session_param.clone()
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "faro_run_command",
            "description": "Run one of the user's pre-approved saved commands by name. Prefer over faro_exec when a saved command fits.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "session": session_param.clone()
                },
                "required": ["name"]
            }
        }),
    ]
}

/// Resolve a session argument to a concrete id, falling back to the active
/// session or the only available session.
async fn resolve_session_arg(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    req_session: &Option<String>,
) -> Result<String> {
    if let Some(s) = req_session {
        if !s.is_empty() {
            // Try to resolve via the bridge's enabled-session resolver.
            let enabled: Vec<String> = state.enabled.lock().await.iter().cloned().collect();
            let manager = app.state::<AppState>().sessions.clone();
            for id in enabled {
                if &id == s {
                    return Ok(id);
                }
                if let Some(sess) = manager.get(&id).await {
                    if sess.profile().name.eq_ignore_ascii_case(s) {
                        return Ok(id);
                    }
                }
            }
            bail!("no enabled session matches '{s}'");
        }
    }

    // Fall back to active session.
    if let Some(active) = state.active_session_id.lock().await.clone() {
        return Ok(active);
    }

    // Last resort: if there's exactly one enabled session, use it.
    let enabled: Vec<String> = state.enabled.lock().await.iter().cloned().collect();
    if enabled.len() == 1 {
        return Ok(enabled[0].clone());
    }

    bail!("please specify which connection to use (session id or name)")
}

async fn run_tool(
    app: &AppHandle,
    state: &Arc<BridgeState>,
    req_session: &Option<String>,
    name: &str,
    input: &Value,
) -> Result<Value> {
    let session_id = resolve_session_arg(app, state, req_session).await?;
    match name {
        "faro_context" => {
            let (_, body) = op_context(app, state).await;
            Ok(body)
        }
        "faro_server_info" => {
            let (_, body) = op_server_info(app, state, &session_id).await;
            Ok(body)
        }
        "faro_exec" => {
            let command = input
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("command is required"))?;
            let dry_run = input.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false);
            let timeout_ms = input.get("timeoutMs").and_then(|v| v.as_u64());
            let (status, body) =
                exec_on(app, state, &session_id, command, dry_run, timeout_ms).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("exec failed")
                ))
            }
        }
        "faro_list_dir" => {
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let (status, body) = op_list_dir(app, state, &session_id, path).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("list_dir failed")
                ))
            }
        }
        "faro_read_file" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("path is required"))?;
            let (status, body) = op_read_file(app, state, &session_id, path).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("read_file failed")
                ))
            }
        }
        "faro_read_files_batch" => {
            let paths = input
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let (status, body) = op_read_file_batch(app, state, &session_id, paths).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("read_files_batch failed")
                ))
            }
        }
        "faro_search" => {
            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("query is required"))?;
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let (status, body) = op_search(app, state, &session_id, path, query).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("search failed")
                ))
            }
        }
        "faro_glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("pattern is required"))?;
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let (status, body) = op_glob(app, state, &session_id, path, pattern).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("glob failed")
                ))
            }
        }
        "faro_tail" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("path is required"))?;
            let lines = input.get("lines").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let (status, body) = op_tail(app, state, &session_id, path, lines).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error").and_then(|v| v.as_str()).unwrap_or("tail failed")
                ))
            }
        }
        "faro_run_command" => {
            let name = input
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("name is required"))?;
            let (status, body) = op_run_command(app, state, &session_id, name, false, None).await;
            if status == 200 {
                Ok(body)
            } else {
                Err(anyhow!(
                    "{}",
                    body.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("run_command failed")
                ))
            }
        }
        other => Err(anyhow!("unknown tool: {other}")),
    }
}
