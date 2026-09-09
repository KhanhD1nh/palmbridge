//! MCP JSON-RPC over stdio (newline-delimited) and Streamable HTTP POST /mcp.
//! No extra crates: ChatGPT tunnel-client speaks stdio; Inspector can use HTTP.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use xai_grok_tools::bridge::ToolBridge;

use crate::host;
use crate::plugin;
use crate::security;
use crate::ui;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "Graft";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
// Keep reconnecting ChatGPT clients from exhausting session capacity. LSP
// remains opt-in per session, so MCP handshakes must not be globally throttled.
const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_HTTP_SESSIONS: usize = 256;

fn negotiate_protocol(requested: Option<&str>) -> &'static str {
    match requested {
        Some(PROTOCOL_VERSION) | None => PROTOCOL_VERSION,
        Some(_) => PROTOCOL_VERSION,
    }
}


struct SessionState {
    workspace: RwLock<PathBuf>,
    cached: Mutex<Option<(PathBuf, ToolBridge)>>,
}

struct SessionEntry {
    state: Arc<SessionState>,
    last_used: Instant,
}

pub struct McpHost {
    fallback_cwd: PathBuf,
    default: Arc<SessionState>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
    call_seq: AtomicU64,
}

impl McpHost {
    pub fn new(fallback_cwd: PathBuf) -> Arc<Self> {
        let workspace = host::resolve_workspace(&fallback_cwd);
        Arc::new(Self {
            fallback_cwd,
            default: Arc::new(SessionState {
                workspace: RwLock::new(workspace),
                cached: Mutex::new(None),
            }),
            sessions: Mutex::new(HashMap::new()),
            call_seq: AtomicU64::new(1),
        })
    }

    pub async fn debug_list(&self) -> Result<Value, String> {
        self.tools_list(&self.default)
            .await
            .map_err(|(_, message, _)| message)
    }

    pub async fn debug_call(&self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .tools_call(&self.default, json!({ "name": name, "arguments": arguments }))
            .await
            .map_err(|(_, message, _)| message)?;
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            Err(text)
        } else {
            Ok(text)
        }
    }

    fn workspace(&self, session: &SessionState) -> PathBuf {
        session.workspace
            .read()
            .map(|path| path.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    async fn bridge(&self, session: &SessionState) -> Result<ToolBridge, String> {
        let cwd = self.workspace(session);
        let mut cache = session.cached.lock().await;
        if let Some((path, bridge)) = cache.as_ref()
            && path == &cwd
        {
            return Ok(bridge.clone());
        }
        let bridge = host::build_bridge(cwd.clone()).await?;
        *cache = Some((cwd, bridge.clone()));
        Ok(bridge)
    }

    fn workspace_info_result(&self, session: &SessionState) -> Value {
        let cwd = self.workspace(session);
        let mut lines = vec![format!("workspace: {}", cwd.display())];
        let recent: Vec<String> = host::read_recent()
            .into_iter()
            .filter(|p| p != &cwd)
            .map(|p| p.display().to_string())
            .collect();
        if recent.is_empty() {
            lines.push("recent: (none)".into());
        } else {
            lines.push("recent:".into());
            for p in &recent {
                lines.push(format!("  {p}"));
            }
        }
        lines.push(
            "Switch from chat with set_workspace({path}). Short names resolve under ~/Dev.".into(),
        );
        json!({
            "content": [{ "type": "text", "text": lines.join("\n") }],
            "structuredContent": {
                "workspace": cwd.display().to_string(),
                "recent": recent,
            },
            "isError": false
        })
    }

    async fn switch_workspace(&self, session: &SessionState, raw: &str) -> Result<PathBuf, String> {
        let path = host::resolve_project(raw)?;
        let cwd = dunce::canonicalize(&path).map_err(|e| format!("canonicalize: {e}"))?;
        match session.workspace.write() {
            Ok(mut workspace) => *workspace = cwd.clone(),
            Err(poisoned) => *poisoned.into_inner() = cwd.clone(),
        }
        host::remember_workspace(&cwd);
        let mut cache = session.cached.lock().await;
        *cache = None;
        Ok(cwd)
    }

    fn fresh_session_state(&self) -> Arc<SessionState> {
        Arc::new(SessionState {
            // The persisted pin is a default for *new* sessions. Re-resolve it
            // here so `graft use` takes effect without disrupting sessions
            workspace: RwLock::new(host::resolve_workspace(&self.fallback_cwd)),
            cached: Mutex::new(None),
        })
    }

    async fn begin_http_session(&self) -> Result<(String, Arc<SessionState>), String> {
        let id = security::new_session_id()?;
        let session = self.fresh_session_state();
        let now = Instant::now();
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, entry| now.duration_since(entry.last_used) < SESSION_TTL);
        if sessions.len() >= MAX_HTTP_SESSIONS {
            return Err("too many active MCP HTTP sessions".into());
        }
        sessions.insert(
            id.clone(),
            SessionEntry {
                state: Arc::clone(&session),
                last_used: now,
            },
        );
        Ok((id, session))
    }

    async fn http_session(&self, id: &str) -> Option<Arc<SessionState>> {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, entry| now.duration_since(entry.last_used) < SESSION_TTL);
        let entry = sessions.get_mut(id)?;
        entry.last_used = now;
        Some(Arc::clone(&entry.state))
    }

    async fn end_http_session(&self, id: &str) -> bool {
        self.sessions.lock().await.remove(id).is_some()
    }

    pub async fn serve_stdio(self: Arc<Self>) -> Result<(), String> {
        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| format!("stdin: {e}"))?
        {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let msg: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    let err = rpc_error(Value::Null, -32700, format!("parse error: {e}"));
                    write_line(&mut stdout, &err).await?;
                    continue;
                }
            };
            if let Some(resp) = self.handle_rpc(&self.default, msg).await {
                write_line(&mut stdout, &resp).await?;
            }
        }
        Ok(())
    }

    pub async fn serve_http(self: Arc<Self>, addr: SocketAddr) -> Result<(), String> {
        let warm = Arc::clone(&self);
        tokio::spawn(async move {
            if let Err(e) = warm.bridge(&warm.default).await {
                eprintln!("warmup: {e}");
            }
        });
        #[cfg(unix)]
        {
            let sock = host::mcp_socket();
            if let Some(parent) = sock.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(&sock);
            let uds = UnixListener::bind(&sock)
                .map_err(|e| format!("bind {}: {e}", sock.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
            }
            eprintln!("MCP uds  {}", sock.display());
            let host_u = Arc::clone(&self);
            tokio::spawn(async move {
                loop {
                    match uds.accept().await {
                        Ok((stream, _)) => {
                            let host = Arc::clone(&host_u);
                            tokio::spawn(async move {
                                let (r, w) = stream.into_split();
                                if let Err(e) =
                                    handle_connection(BufReader::new(r), w, host, false).await
                                {
                                    eprintln!("uds: {e}");
                                }
                            });
                        }
                        Err(e) => eprintln!("uds accept: {e}"),
                    }
                }
            });
        }
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind {addr}: {e}"))?;
        eprintln!("Graft UI  http://{addr}/");
        eprintln!("MCP       http://{addr}/mcp");
        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|e| format!("accept: {e}"))?;
            let _ = stream.set_nodelay(true); // tool-call latency beats batching
            let host = Arc::clone(&self);
            tokio::spawn(async move {
                let (r, w) = stream.into_split();
                if let Err(e) = handle_connection(BufReader::new(r), w, host, false).await {
                    eprintln!("http: {e}");
                }
            });
        }
    }

    async fn handle_rpc(&self, session: &SessionState, msg: Value) -> Option<Value> {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned()?;
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let result = match method {
            "initialize" => Ok(self.initialize(session, params.clone())),
            "ping" => Ok(json!({})),
            "server/discover" => Ok(json!({
                "supportedVersions": ["2026-07-28"],
                "capabilities": plugin::initialize_capabilities(),
                "instructions": plugin::initialize_instructions(
                    &self.workspace(session).display().to_string()
                ),
                "_meta": {
                    "io.modelcontextprotocol/serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION,
                    }
                },
                "ttlMs": 0,
                "cacheScope": "private",
                "resultType": "complete",
            })),
            "tools/list" => self.tools_list(session).await,
            "tools/call" => self.tools_call(session, params.clone()).await,
            "skills/list" => Ok(plugin::skills_list()),
            "skills/get" => plugin::skills_get(&params),
            "resources/list" => Ok(plugin::resources_list()),
            "resources/read" => plugin::resources_read(&params),
            other => Err((
                -32601,
                format!("method not found: {other}"),
                Value::Null,
            )),
        };

        Some(match result {
            Ok(mut value) => {
                if params
                    .pointer("/_meta/io.modelcontextprotocol~1protocolVersion")
                    .and_then(Value::as_str)
                    .is_some_and(|version| version >= "2026-07-28")
                {
                    value["resultType"] = Value::String("complete".into());
                }
                json!({"jsonrpc": "2.0", "id": id, "result": value})
            }
            Err((code, message, data)) => rpc_error_with_data(id, code, message, data),
        })
    }

    fn initialize(&self, session: &SessionState, params: Value) -> Value {
        let negotiated_version = negotiate_protocol(
            params.get("protocolVersion").and_then(Value::as_str),
        );
        json!({
            "protocolVersion": negotiated_version,
            "capabilities": plugin::initialize_capabilities(),
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            },
            "instructions": plugin::initialize_instructions(
                &self.workspace(session).display().to_string()
            ),
        })
    }

    async fn tools_list(&self, session: &SessionState) -> Result<Value, (i64, String, Value)> {
        let mut tools = vec![
            plugin::tool_descriptor(
                "workspace_info",
                "Use this when you need the active local workspace root and recently used folders. Call before other tools if the workspace might have changed.",
                json!({ "type": "object", "properties": {} }),
            ),
            plugin::tool_descriptor(
                "set_workspace",
                "Use this when the user wants another repo, including while they are not at the machine. Switches the current Graft server session without changing the persisted default. Accepts an absolute path, ~/path, or a short name resolved under ~/Dev (e.g. bunko).",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory to pin: absolute, ~/…, or folder name under ~/Dev"
                        }
                    },
                    "required": ["path"]
                }),
            ),
        ];
        let defs = self
            .bridge(session)
            .await
            .map_err(|e| (-32603, e, Value::Null))?
            .tool_definitions()
            .await;
        tools.extend(defs.into_iter().map(|d| {
            let name = d.function.name;
            let description = d.function.description.unwrap_or_default();
            plugin::tool_descriptor(&name, &description, d.function.parameters)
        }));
        tools.push(crate::browser::tool_definition());
        Ok(json!({ "tools": tools }))
    }

    async fn tools_call(&self, session: &SessionState, params: Value) -> Result<Value, (i64, String, Value)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "tools/call requires name".into(), Value::Null))?;
        if name == "workspace_info" {
            return Ok(self.workspace_info_result(session));
        }
        if name == "set_workspace" {
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or((-32602, "set_workspace requires path".into(), Value::Null))?;
            return match self.switch_workspace(session, path).await {
                Ok(cwd) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("workspace switched for this Graft server session: {}\nThe persisted default workspace was not changed.", cwd.display())
                    }],
                    "structuredContent": {
                        "workspace": cwd.display().to_string()
                    },
                    "isError": false
                })),
                Err(e) => Ok(json!({
                    "content": [{ "type": "text", "text": e }],
                    "isError": true
                })),
            };
        }
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        if name == "glob" {
            let cwd = self.workspace(session);
            return match crate::native_glob::run(&arguments, &cwd) {
                Ok(text) => Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                })),
                Err(error) => Ok(json!({
                    "content": [{ "type": "text", "text": error }],
                    "isError": true
                })),
            };
        }
        if name == "browser" {
            let cwd = self.workspace(session);
            return match crate::browser::run(&arguments, &cwd).await {
                Ok(text) => Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                })),
                Err(error) => Ok(json!({
                    "content": [{ "type": "text", "text": error }],
                    "isError": true
                })),
            };
        }
        let call_id = format!(
            "mcp-{}",
            self.call_seq.fetch_add(1, Ordering::Relaxed)
        );
        let bridge = self
            .bridge(session)
            .await
            .map_err(|e| (-32603, e, Value::Null))?;
        match bridge.call(name, arguments, &call_id).await {
            Ok(result) => Ok(json!({
                "content": [{ "type": "text", "text": result.prompt_text }],
                "isError": false
            })),
            Err(e) => Ok(json!({
                "content": [{ "type": "text", "text": e.to_string() }],
                "isError": true
            })),
        }
    }
}


fn rpc_error(id: Value, code: i64, message: String) -> Value {
    rpc_error_with_data(id, code, message, Value::Null)
}

fn rpc_error_with_data(id: Value, code: i64, message: String, data: Value) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if !data.is_null() {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

async fn write_line(stdout: &mut tokio::io::Stdout, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    line.push('\n');
    stdout
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("stdout: {e}"))?;
    stdout.flush().await.map_err(|e| format!("flush: {e}"))?;
    Ok(())
}

async fn handle_connection<R, W>(
    mut reader: BufReader<R>,
    mut writer: W,
    host: Arc<McpHost>,
    _require_mcp_token: bool,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let mut header_buf = Vec::new();
        loop {
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Ok(());
            }
            header_buf.extend_from_slice(line.as_bytes());
            if line == "\r\n" || line == "\n" {
                break;
            }
            if header_buf.len() > 64 * 1024 {
                write_http(&mut writer, 431, "text/plain", b"headers too large", false)
                    .await?;
                return Ok(());
            }
        }
        let header_text = String::from_utf8_lossy(&header_buf);
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("/");
        let version = parts.next().unwrap_or("HTTP/1.1");

        let mut content_length = 0usize;
        let mut accept = String::new();
        let mut connection = String::new();
        let mut content_type = String::new();
        let mut origin = String::new();
        let mut host_header = String::new();
        let mut mcp_session_id = String::new();
        for line in lines {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("accept") {
                accept = v.to_string();
            } else if k.eq_ignore_ascii_case("connection") {
                connection = v.to_string();
            } else if k.eq_ignore_ascii_case("content-type") {
                content_type = v.to_string();
            } else if k.eq_ignore_ascii_case("origin") {
                origin = v.to_string();
            } else if k.eq_ignore_ascii_case("host") {
                host_header = v.to_string();
            } else if k.eq_ignore_ascii_case("mcp-session-id") {
                mcp_session_id = v.to_string();
            }
        }
        let keep = if connection.eq_ignore_ascii_case("close") {
            false
        } else if connection.eq_ignore_ascii_case("keep-alive") {
            true
        } else {
            version.eq_ignore_ascii_case("HTTP/1.1")
        };

        if content_length > 8 * 1024 * 1024 {
            write_http(&mut writer, 413, "text/plain", b"body too large", false).await?;
            return Ok(());
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader
                .read_exact(&mut body)
                .await
                .map_err(|e| format!("body: {e}"))?;
        }

        let path_only = path.split('?').next().unwrap_or(path);
        if !host_header.is_empty() && !security::is_loopback_host(&host_header) {
            write_http(&mut writer, 403, "text/plain", b"forbidden host", false).await?;
            return Ok(());
        }
        if method == "GET" && (path_only == "/health" || path_only == "/healthz") {
            write_http(&mut writer, 200, "text/plain", b"ok", keep).await?;
            if !keep {
                return Ok(());
            }
            continue;
        }
        if path_only == "/.well-known/oauth-protected-resource"
            || path_only == "/.well-known/oauth-protected-resource/mcp"
        {
            write_http(
                &mut writer,
                200,
                "application/json",
                br#"{"resource":"http://127.0.0.1:8787/mcp"}"#,
                keep,
            )
            .await?;
            if !keep {
                return Ok(());
            }
            continue;
        }
        if path_only.contains("/.well-known/")
            || (method == "GET" && path_only == "/" && !accept.to_lowercase().contains("text/html"))
        {
            write_http(
                &mut writer,
                404,
                "application/json",
                br#"{"error":"not_found"}"#,
                keep,
            )
            .await?;
            if !keep {
                return Ok(());
            }
            continue;
        }
        if method == "POST" && path_only.starts_with("/api/") {
            let json_body = content_type
                .split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
            if !json_body
                || (!origin.is_empty()
                    && !security::is_same_loopback_origin(&origin, &host_header))
            {
                write_http(&mut writer, 403, "application/json", br#"{"error":"forbidden"}"#, keep)
                    .await?;
                if !keep {
                    return Ok(());
                }
                continue;
            }
        }
        if let Some((status, ctype, payload)) = ui::route(method, path_only, &body) {
            write_http(&mut writer, status, ctype, &payload, keep).await?;
            if !keep {
                return Ok(());
            }
            continue;
        }
        if !matches!(method, "POST" | "DELETE") || path_only != "/mcp" {
            write_http(&mut writer, 404, "text/plain", b"not found", keep).await?;
            if !keep {
                return Ok(());
            }
            continue;
        }
        if method == "DELETE" {
            let status = if mcp_session_id.is_empty() {
                400
            } else if host.end_http_session(&mcp_session_id).await {
                204
            } else {
                404
            };
            write_http(&mut writer, status, "text/plain", b"", keep).await?;
            if !keep {
                return Ok(());
            }
            continue;
        }

        let json_body = content_type
            .split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
        if !json_body {
            write_http(
                &mut writer,
                415,
                "application/json",
                br#"{"error":"content_type_must_be_application_json"}"#,
                keep,
            )
            .await?;
            if !keep {
                return Ok(());
            }
            continue;
        }

        let msg = match serde_json::from_slice::<Value>(&body) {
            Ok(msg) => msg,
            Err(e) => {
                let resp = rpc_error(Value::Null, -32700, format!("parse error: {e}"));
                let payload = serde_json::to_vec(&resp).map_err(|e| e.to_string())?;
                write_http(&mut writer, 400, "application/json", &payload, keep).await?;
                if !keep {
                    return Ok(());
                }
                continue;
            }
        };
        let method = msg.get("method").and_then(Value::as_str);
        let (session, new_session_id) = if method == Some("initialize") {
            let (id, session) = host.begin_http_session().await?;
            (session, Some(id))
        } else if method == Some("server/discover") {
            // tunnel-client discovers before initialize. Do not attach a
            // session header here: it treats discovery as stateless.
            (Arc::clone(&host.default), None)
        } else if mcp_session_id.is_empty()
            && msg
                .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
                .and_then(Value::as_str)
                .is_some_and(|version| version >= "2026-07-28")
        {
            // MCP 2026-07-28 is stateless: each request includes the protocol
            // version and client capabilities in params._meta.
            (Arc::clone(&host.default), None)
        } else if !mcp_session_id.is_empty() {
            let Some(session) = host.http_session(&mcp_session_id).await else {
                write_http(&mut writer, 404, "text/plain", b"unknown MCP session", keep).await?;
                if !keep {
                    return Ok(());
                }
                continue;
            };
            (session, None)
        } else {
            write_http(
                &mut writer,
                400,
                "application/json",
                br#"{"error":"missing_mcp_session_id"}"#,
                keep,
            )
            .await?;
            if !keep {
                return Ok(());
            }
            continue;
        };

        let Some(resp) = host.handle_rpc(&session, msg).await else {
            write_http_with_headers(
                &mut writer,
                202,
                "application/json",
                b"",
                keep,
                &[],
            )
            .await?;
            if !keep {
                return Ok(());
            }
            continue;
        };
        let payload = serde_json::to_vec(&resp).map_err(|e| e.to_string())?;
        let response_headers = new_session_id
            .as_deref()
            .map(|id| vec![("Mcp-Session-Id", id)])
            .unwrap_or_default();
        if accept.contains("text/event-stream") && !accept.contains("application/json") {
            let mut sse = Vec::from("event: message\ndata: ");
            sse.extend_from_slice(&payload);
            sse.extend_from_slice(b"\n\n");
            write_http_with_headers(
                &mut writer,
                200,
                "text/event-stream",
                &sse,
                keep,
                &response_headers,
            )
            .await?;
        } else {
            write_http_with_headers(
                &mut writer,
                200,
                "application/json",
                &payload,
                keep,
                &response_headers,
            )
            .await?;
        }
        if !keep {
            return Ok(());
        }
    }
}

async fn write_http<W: AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    content_type: &str,
    body: &[u8],
    keep_alive: bool,
) -> Result<(), String> {
    write_http_with_headers(writer, status, content_type, body, keep_alive, &[]).await
}

async fn write_http_with_headers<W: AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    content_type: &str,
    body: &[u8],
    keep_alive: bool,
    extra_headers: &[(&str, &str)],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let conn = if keep_alive {
        "keep-alive"
    } else {
        "close"
    };
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: {conn}\r\n\r\n",
        body.len()
    );
    if !extra_headers.is_empty() {
        let suffix = "\r\n";
        header.truncate(header.len() - suffix.len());
        for (name, value) in extra_headers {
            header.push_str(name);
            header.push_str(": ");
            header.push_str(value);
            header.push_str("\r\n");
        }
        header.push_str("\r\n");
    }
    let mut buf = Vec::with_capacity(header.len() + body.len());
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(body);
    writer
        .write_all(&buf)
        .await
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::{negotiate_protocol, PROTOCOL_VERSION};

    #[test]
    fn protocol_negotiation_never_echoes_an_unsupported_version() {
        assert_eq!(negotiate_protocol(Some(PROTOCOL_VERSION)), PROTOCOL_VERSION);
        assert_eq!(negotiate_protocol(Some("2026-07-28")), PROTOCOL_VERSION);
        assert_eq!(negotiate_protocol(None), PROTOCOL_VERSION);
    }
}
