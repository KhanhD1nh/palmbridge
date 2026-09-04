use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::sleep;
use tokio_tungstenite::{WebSocketStream, connect_async, tungstenite::Message};

const DEFAULT_PORT: u16 = 9222;
const DEFAULT_WIDTH: u32 = 1440;
const DEFAULT_HEIGHT: u32 = 900;
const DEFAULT_WAIT_MS: u64 = 600;

pub fn tool_definition() -> Value {
    json!({
        "name": "browser",
        "description": "Inspect and debug a real Chromium page. Operations: start (persistent debug browser), inspect (DOM + computed styles, optionally sampled over time), eval (run JavaScript), screenshot, stop. If no debug browser is running, inspect/eval/screenshot launch an ephemeral headless browser. For authenticated localhost apps, run operation=start once and sign in to the persistent Palmbridge browser profile.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["start", "inspect", "eval", "screenshot", "stop"]
                },
                "url": { "type": "string", "description": "Page URL. Usually a localhost dev/preview URL." },
                "selector": { "type": "string", "description": "CSS selector for inspect." },
                "xpath": { "type": "string", "description": "XPath for inspect; used instead of selector when provided." },
                "expression": { "type": "string", "description": "JavaScript expression for eval." },
                "script": { "type": "string", "description": "Optional JavaScript run before inspect/screenshot, e.g. click a theme toggle." },
                "sample_delays_ms": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 0, "maximum": 10000 },
                    "maxItems": 20,
                    "description": "For inspect, sample computed styles at these absolute delays after script, e.g. [0,50,100,150,250]."
                },
                "wait_ms": { "type": "integer", "minimum": 0, "maximum": 10000, "default": 600 },
                "width": { "type": "integer", "minimum": 240, "maximum": 7680, "default": 1440 },
                "height": { "type": "integer", "minimum": 240, "maximum": 4320, "default": 900 },
                "port": { "type": "integer", "minimum": 1024, "maximum": 65535, "default": 9222 },
                "headless": { "type": "boolean", "default": false, "description": "Only used by operation=start." },
                "user_data_dir": { "type": "string", "description": "Optional Chromium user-data directory for operation=start." },
                "output_path": { "type": "string", "description": "Optional PNG path for screenshot." }
            },
            "required": ["operation"],
            "additionalProperties": false
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "openWorldHint": true
        }
    })
}

pub async fn run(arguments: &Value, cwd: &Path) -> Result<String, String> {
    let operation = arguments
        .get("operation")
        .and_then(Value::as_str)
        .ok_or("browser requires operation")?;
    match operation {
        "start" => start_persistent(arguments).await,
        "stop" => stop_persistent(),
        "inspect" | "eval" | "screenshot" => run_page_operation(operation, arguments, cwd).await,
        other => Err(format!("unsupported browser operation: {other}")),
    }
}

async fn start_persistent(arguments: &Value) -> Result<String, String> {
    let port = arg_u16(arguments, "port", DEFAULT_PORT);
    if cdp_ready(port).await {
        return Ok(format!("Palmbridge browser is already listening on http://127.0.0.1:{port}."));
    }
    let browser = find_browser().ok_or_else(browser_not_found_message)?;
    let profile = arguments
        .get("user_data_dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::host::config_dir().join("browser-profile"));
    fs::create_dir_all(&profile).map_err(|e| format!("create browser profile: {e}"))?;
    let width = arg_u32(arguments, "width", DEFAULT_WIDTH);
    let height = arg_u32(arguments, "height", DEFAULT_HEIGHT);
    let headless = arguments.get("headless").and_then(Value::as_bool).unwrap_or(false);
    let url = arguments.get("url").and_then(Value::as_str).unwrap_or("about:blank");

    let mut child = spawn_browser(&browser, port, &profile, width, height, headless, url)?;
    let pid = child.id();
    if let Err(error) = wait_for_cdp(port, Duration::from_secs(8)).await {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    write_browser_state(pid, port)?;
    std::mem::forget(child);
    Ok(format!(
        "Started Palmbridge browser (pid {pid}) on debug port {port}.\nProfile: {}\nURL: {url}\nIf the app requires authentication, sign in once in this browser; later browser inspect/eval calls can reuse the session.",
        profile.display()
    ))
}

fn stop_persistent() -> Result<String, String> {
    let pid_path = browser_pid_path();
    let pid = fs::read_to_string(&pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if let Some(pid) = pid {
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(not(windows))]
        {
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = fs::remove_file(pid_path);
        let _ = fs::remove_file(browser_port_path());
        return Ok(format!("Stopped Palmbridge browser process {pid}."));
    }
    Ok("No Palmbridge browser pid is recorded.".into())
}

async fn run_page_operation(operation: &str, arguments: &Value, cwd: &Path) -> Result<String, String> {
    let requested_port = arg_u16(arguments, "port", recorded_port().unwrap_or(DEFAULT_PORT));
    let width = arg_u32(arguments, "width", DEFAULT_WIDTH);
    let height = arg_u32(arguments, "height", DEFAULT_HEIGHT);
    let wait_ms = arg_u64(arguments, "wait_ms", DEFAULT_WAIT_MS).min(10_000);
    let url = arguments.get("url").and_then(Value::as_str).unwrap_or("about:blank");

    let mut ephemeral = None;
    let (port, create_target) = if cdp_ready(requested_port).await {
        (requested_port, true)
    } else {
        let port = free_port()?;
        let browser = find_browser().ok_or_else(browser_not_found_message)?;
        let profile = std::env::temp_dir().join(format!(
            "palmbridge-browser-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&profile).map_err(|e| format!("create temp browser profile: {e}"))?;
        let child = spawn_browser(&browser, port, &profile, width, height, true, url)?;
        ephemeral = Some(EphemeralBrowser { child, profile });
        wait_for_cdp(port, Duration::from_secs(8)).await?;
        (port, false)
    };

    let target = if create_target {
        create_page_target(port, url).await.or_else(|_| async_error_placeholder())?
    } else {
        wait_for_page_target(port, Duration::from_secs(5)).await?
    };
    let ws_url = target
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or("Chromium target did not expose webSocketDebuggerUrl")?;
    let (mut ws, _) = connect_async(ws_url)
        .await
        .map_err(|e| format!("connect Chrome DevTools websocket: {e}"))?;
    let mut next_id = 1u64;

    cdp_call(&mut ws, &mut next_id, "Runtime.enable", json!({})).await?;
    cdp_call(&mut ws, &mut next_id, "Page.enable", json!({})).await?;
    sleep(Duration::from_millis(wait_ms)).await;

    if let Some(script) = arguments.get("script").and_then(Value::as_str) {
        evaluate(&mut ws, &mut next_id, script).await?;
    }

    let output = match operation {
        "eval" => {
            let expression = arguments
                .get("expression")
                .and_then(Value::as_str)
                .ok_or("browser eval requires expression")?;
            let value = evaluate(&mut ws, &mut next_id, expression).await?;
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        }
        "inspect" => {
            let selector = arguments.get("selector").and_then(Value::as_str);
            let xpath = arguments.get("xpath").and_then(Value::as_str);
            let expression = inspect_expression(selector, xpath);
            let mut delays = arguments
                .get("sample_delays_ms")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).take(20).collect::<Vec<_>>())
                .unwrap_or_else(|| vec![0]);
            if delays.is_empty() {
                delays.push(0);
            }
            delays.sort_unstable();
            delays.dedup();
            let started = Instant::now();
            let mut samples = Vec::with_capacity(delays.len());
            for delay in delays {
                let target_delay = Duration::from_millis(delay.min(10_000));
                if started.elapsed() < target_delay {
                    sleep(target_delay - started.elapsed()).await;
                }
                let value = evaluate(&mut ws, &mut next_id, &expression).await?;
                samples.push(json!({ "elapsedMs": started.elapsed().as_millis(), "inspection": value }));
            }
            serde_json::to_string_pretty(&json!({
                "url": url,
                "debugPort": port,
                "samples": samples
            }))
            .map_err(|e| e.to_string())?
        }
        "screenshot" => {
            let result = cdp_call(
                &mut ws,
                &mut next_id,
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": false }),
            )
            .await?;
            let data = result.get("data").and_then(Value::as_str).ok_or("screenshot returned no data")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| format!("decode screenshot: {e}"))?;
            let output_path = match arguments.get("output_path").and_then(Value::as_str) {
                Some(path) => workspace_output_path(cwd, path)?,
                None => std::env::temp_dir().join(format!("palmbridge-browser-{}.png", now_millis())),
            };
            fs::write(&output_path, bytes).map_err(|e| format!("write screenshot: {e}"))?;
            format!("Screenshot: {}", output_path.display())
        }
        _ => unreachable!(),
    };

    let _ = ws.close(None).await;
    drop(ephemeral);
    Ok(output)
}

fn workspace_output_path(cwd: &Path, output: &str) -> Result<PathBuf, String> {
    let relative = Path::new(output);
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err("browser screenshot output_path must be a non-empty path within the workspace".into());
    }
    if relative.components().any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) {
        return Err("browser screenshot output_path must not escape the workspace".into());
    }
    let workspace = dunce::canonicalize(cwd)
        .map_err(|e| format!("canonicalize workspace {}: {e}", cwd.display()))?;
    let output_path = workspace.join(relative);
    let resolved = if output_path.exists() {
        dunce::canonicalize(&output_path)
    } else {
        let parent = output_path.parent().ok_or("browser screenshot output_path has no parent")?;
        dunce::canonicalize(parent)
    }
    .map_err(|e| format!("canonicalize screenshot output path: {e}"))?;
    if !resolved.starts_with(&workspace) {
        return Err("browser screenshot output_path escapes the workspace".into());
    }
    Ok(output_path)
}

// Helper used only to keep the fallback expression in run_page_operation readable.
fn async_error_placeholder() -> Result<Value, String> {
    Err("failed to create a new Chromium page target".into())
}

async fn evaluate<S>(
    ws: &mut WebSocketStream<S>,
    next_id: &mut u64,
    expression: &str,
) -> Result<Value, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let result = cdp_call(
        ws,
        next_id,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true
        }),
    )
    .await?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(format!("JavaScript exception: {details}"));
    }
    Ok(result
        .get("result")
        .and_then(|v| v.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

async fn cdp_call<S>(
    ws: &mut WebSocketStream<S>,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let id = *next_id;
    *next_id += 1;
    let payload = json!({ "id": id, "method": method, "params": params }).to_string();
    ws.send(Message::Text(payload.into()))
        .await
        .map_err(|e| format!("send CDP {method}: {e}"))?;

    while let Some(message) = ws.next().await {
        let message = message.map_err(|e| format!("read CDP {method}: {e}"))?;
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(text.as_ref()).map_err(|e| format!("parse CDP response: {e}"))?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(format!("CDP {method} failed: {error}"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    Err(format!("CDP connection closed while waiting for {method}"))
}

fn inspect_expression(selector: Option<&str>, xpath: Option<&str>) -> String {
    let selector = serde_json::to_string(selector.unwrap_or("body")).unwrap();
    let xpath = xpath.map(|s| serde_json::to_string(s).unwrap());
    let lookup = if let Some(xpath) = xpath {
        format!("document.evaluate({xpath}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue")
    } else {
        format!("document.querySelector({selector})")
    };
    format!(
        r#"(() => {{
  const el = {lookup};
  const viewport = {{ width: innerWidth, height: innerHeight, devicePixelRatio }};
  if (!el) return {{ found: false, viewport, rootClass: document.documentElement.className, bodyClass: document.body?.className ?? '' }};
  const s = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  const pick = (node) => {{
    if (!node || node.nodeType !== 1) return null;
    const cs = getComputedStyle(node); const br = node.getBoundingClientRect();
    return {{ tag: node.tagName.toLowerCase(), id: node.id || null, className: String(node.className || ''), display: cs.display,
      backgroundColor: cs.backgroundColor, color: cs.color, fontSize: cs.fontSize,
      rect: {{ x: br.x, y: br.y, width: br.width, height: br.height }} }};
  }};
  const styles = {{
    display: s.display, position: s.position, width: s.width, height: s.height,
    minWidth: s.minWidth, maxWidth: s.maxWidth, minHeight: s.minHeight, maxHeight: s.maxHeight,
    margin: s.margin, padding: s.padding, boxSizing: s.boxSizing,
    color: s.color, backgroundColor: s.backgroundColor,
    fontSize: s.fontSize, fontFamily: s.fontFamily, fontWeight: s.fontWeight,
    lineHeight: s.lineHeight, letterSpacing: s.letterSpacing,
    border: s.border, borderTop: s.borderTop, borderRight: s.borderRight,
    borderBottom: s.borderBottom, borderLeft: s.borderLeft, borderRadius: s.borderRadius,
    overflow: s.overflow, overflowX: s.overflowX, overflowY: s.overflowY,
    flex: s.flex, flexDirection: s.flexDirection, flexWrap: s.flexWrap,
    alignItems: s.alignItems, justifyContent: s.justifyContent, gap: s.gap,
    gridTemplateColumns: s.gridTemplateColumns, gridTemplateRows: s.gridTemplateRows,
    transform: s.transform, opacity: s.opacity, zIndex: s.zIndex,
    transition: s.transition, animation: s.animation
  }};
  const ancestors = []; let p = el.parentElement;
  for (let i = 0; p && i < 4; i++, p = p.parentElement) ancestors.push(pick(p));
  return {{ found: true, viewport, rootClass: document.documentElement.className,
    bodyClass: document.body?.className ?? '', tag: el.tagName.toLowerCase(), id: el.id || null,
    className: String(el.className || ''), text: (el.textContent || '').trim().slice(0, 500),
    rect: {{ x: r.x, y: r.y, top: r.top, right: r.right, bottom: r.bottom, left: r.left, width: r.width, height: r.height }},
    styles, ancestors }};
}})()"#
    )
}

async fn cdp_ready(port: u16) -> bool {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

async fn wait_for_cdp(port: u16, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cdp_ready(port).await {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err(format!("Chromium debug port {port} did not become ready within {timeout:?}"))
}

async fn wait_for_page_target(port: u16, timeout: Duration) -> Result<Value, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let list: Value = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .send()
            .await
            .map_err(|e| format!("list Chromium targets: {e}"))?
            .json()
            .await
            .map_err(|e| format!("parse Chromium targets: {e}"))?;
        if let Some(target) = list
            .as_array()
            .and_then(|a| a.iter().find(|t| t.get("type").and_then(Value::as_str) == Some("page")))
        {
            return Ok(target.clone());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err("no Chromium page target became available".into())
}

async fn create_page_target(port: u16, url: &str) -> Result<Value, String> {
    let mut endpoint = reqwest::Url::parse(&format!("http://127.0.0.1:{port}/json/new"))
        .map_err(|e| e.to_string())?;
    endpoint.set_query(Some(url));
    let response = reqwest::Client::new()
        .put(endpoint)
        .send()
        .await
        .map_err(|e| format!("create Chromium target: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("create Chromium target returned {}", response.status()));
    }
    response.json().await.map_err(|e| format!("parse new Chromium target: {e}"))
}

fn spawn_browser(
    executable: &Path,
    port: u16,
    profile: &Path,
    width: u32,
    height: u32,
    headless: bool,
    url: &str,
) -> Result<Child, String> {
    let mut cmd = Command::new(executable);
    cmd.arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--remote-allow-origins=*")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--window-size={width},{height}"));
    if headless {
        cmd.arg("--headless=new").arg("--disable-gpu");
    }
    cmd.arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("launch Chromium {}: {e}", executable.display()))
}

fn find_browser() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PALMBRIDGE_BROWSER_PATH") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        for base_var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(base_var) {
                let base = PathBuf::from(base);
                candidates.extend([
                    base.join("Google/Chrome/Application/chrome.exe"),
                    base.join("BraveSoftware/Brave-Browser/Application/brave.exe"),
                    base.join("Microsoft/Edge/Application/msedge.exe"),
                ]);
            }
        }
        if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
            return Some(found);
        }
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    for name in ["chrome", "chromium", "chromium-browser", "brave", "brave-browser", "msedge"] {
        if let Some(path) = find_on_path(name) {
            return Some(path);
        }
    }
    None
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let candidate = dir.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn browser_not_found_message() -> String {
    "No Chromium browser found. Install Chrome/Brave/Edge/Chromium or set PALMBRIDGE_BROWSER_PATH.".into()
}

fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("allocate browser port: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

fn browser_pid_path() -> PathBuf {
    crate::host::config_dir().join("browser.pid")
}

fn browser_port_path() -> PathBuf {
    crate::host::config_dir().join("browser.port")
}

fn write_browser_state(pid: u32, port: u16) -> Result<(), String> {
    fs::create_dir_all(crate::host::config_dir()).map_err(|e| e.to_string())?;
    fs::write(browser_pid_path(), pid.to_string()).map_err(|e| e.to_string())?;
    fs::write(browser_port_path(), port.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

fn recorded_port() -> Option<u16> {
    fs::read_to_string(browser_port_path()).ok()?.trim().parse().ok()
}

fn arg_u64(arguments: &Value, key: &str, default: u64) -> u64 {
    arguments.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn arg_u32(arguments: &Value, key: &str, default: u32) -> u32 {
    arg_u64(arguments, key, default as u64).min(u32::MAX as u64) as u32
}

fn arg_u16(arguments: &Value, key: &str, default: u16) -> u16 {
    arg_u64(arguments, key, default as u64).min(u16::MAX as u64) as u16
}

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

struct EphemeralBrowser {
    child: Child,
    profile: PathBuf,
}

impl Drop for EphemeralBrowser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.profile);
    }
}
