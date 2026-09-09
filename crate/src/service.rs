//! Keep `tunnel-client` running: login start + restart if the MCP child dies.
//!
//! Foreground `tunnel-client run` exits when its stdio MCP child is killed.
//! A LaunchAgent / systemd user unit with KeepAlive is the actual client.

use std::fs;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::host;

pub const HEALTH_PORT: u16 = 18780;
pub const HEALTH_LISTEN: &str = "127.0.0.1:18780";
pub const HEALTH_BASE: &str = "http://127.0.0.1:18780";
pub const MCP_PORT: u16 = 8787;
pub const MCP_BASE: &str = "http://127.0.0.1:8787";
pub const PROFILE: &str = "graft";
#[cfg(target_os = "macos")]
const LABEL: &str = "dev.graft.tunnel";
#[cfg(target_os = "macos")]
const MCP_LABEL: &str = "dev.graft.mcp";
#[cfg(target_os = "macos")]
const WATCH_LABEL: &str = "dev.graft.watch";
#[cfg(target_os = "macos")]
const LEGACY_LABELS: &[&str] = &[
    "dev.palmbridge.tunnel", "dev.palmbridge.mcp", "dev.palmbridge.watch",
    "dev.hands.tunnel", "ai.grok.harness.tunnel",
];
const LEGACY_PROFILES: &[&str] = &["palmbridge", "hands", "grok-harness"];

pub fn profile_file() -> PathBuf {
    host::tunnel_client_dir().join(format!("{PROFILE}.yaml"))
}

fn legacy_profile_files() -> impl Iterator<Item = PathBuf> {
    LEGACY_PROFILES
        .iter()
        .map(|profile| host::tunnel_client_dir().join(format!("{profile}.yaml")))
}

pub fn ready() -> bool {
    http_get(HEALTH_PORT, "/readyz").is_ok_and(|s| s == "ready")
}

pub fn mcp_ready() -> bool {
    http_get(MCP_PORT, "/healthz").is_ok_and(|s| s == "ok")
}

/// Minimal HTTP/1.0 GET over std::net — no subprocess, no deps.
/// Works from sync threads and tokio workers alike.
fn http_get(port: u16, path: &str) -> Result<String, ()> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).map_err(|_| ())?;
    stream.set_read_timeout(Some(Duration::from_millis(900))).map_err(|_| ())?;
    stream.set_write_timeout(Some(Duration::from_millis(500))).map_err(|_| ())?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .map_err(|_| ())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).map_err(|_| ())?;
    let mut parts = buf.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or("");
    if !head.starts_with("HTTP/1.1 2") && !head.starts_with("HTTP/1.0 2") {
        return Err(());
    }
    parts.next().map(str::to_string).map(|s| s.trim().to_string()).ok_or(())
}

fn wait_until(probe: impl Fn() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if probe() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_mcp(timeout: Duration) -> bool {
    wait_until(mcp_ready, timeout)
}

pub fn wait_ready(timeout: Duration) -> bool {
    wait_until(ready, timeout)
}

pub fn status_line() -> String {
    let health = if ready() {
        format!("ready  {HEALTH_BASE}/ui")
    } else {
        "down".into()
    };
    let svc = if installed() {
        "enabled (login + restart)"
    } else {
        "off — graft setup"
    };
    format!("{health}\nservice    {svc}")
}

/// Pin-time helper: start the supervised client if it is down.
pub fn ensure() -> Result<bool, String> {
    if ready() {
        if !installed() && can_enable() {
            enable()?;
        }
        return Ok(ready());
    }
    if installed() {
        start()?;
    } else if can_enable() {
        enable()?;
    } else {
        return Ok(false);
    }
    Ok(wait_ready(Duration::from_secs(12)))
}

#[cfg(windows)]
pub fn enable() -> Result<(), String> {
    host::migrate_from_legacy();
    let key = persist_key()?;
    let tunnel_id = resolve_tunnel_id()?;
    let client = tunnel_client_bin()?;
    write_profile(&key, &tunnel_id)?;
    write_wrapper(&client)?;
    install_supervisor()?;
    let _ = install_watch();
    if wait_mcp(Duration::from_secs(8)) && wait_ready(Duration::from_secs(15)) {
        eprintln!("tunnel on. login start + restart. config: graft config");
        eprintln!("admin  {HEALTH_BASE}/ui");
        Ok(())
    } else {
        Err(format!(
            "service installed but tunnel is not up yet. logs: {}",
            host::config_dir().join("logs").display()
        ))
    }
}

#[cfg(not(windows))]
pub fn enable() -> Result<(), String> {
    host::migrate_from_legacy();
    let key = persist_key()?;
    let tunnel_id = resolve_tunnel_id()?;
    let client = tunnel_client_bin()?;
    write_profile(&key, &tunnel_id)?;
    write_wrapper(&client)?;
    install_mcp()?;
    if !wait_mcp(Duration::from_secs(8)) {
        return Err(format!("MCP HTTP not up on {MCP_BASE}"));
    }
    install_supervisor()?;
    let _ = install_watch();
    if wait_ready(Duration::from_secs(15)) {
        eprintln!("tunnel on. login start + restart. config: graft config");
        eprintln!("admin  {HEALTH_BASE}/ui");
        Ok(())
    } else {
        Err(format!(
            "service installed but /readyz is not up yet. logs: {}",
            host::config_dir().join("logs").display()
        ))
    }
}

pub fn disable() -> Result<(), String> {
    uninstall_supervisor()?;
    eprintln!("tunnel auto-start removed.");
    Ok(())
}

#[cfg(windows)]
pub fn start() -> Result<(), String> {
    if !installed() {
        return enable();
    }
    write_profile(&persist_key()?, &resolve_tunnel_id()?)?;
    start_supervisor()?;
    if wait_mcp(Duration::from_secs(8)) && wait_ready(Duration::from_secs(15)) {
        eprintln!("tunnel ready  {HEALTH_BASE}/ui");
        Ok(())
    } else {
        Err(format!(
            "tunnel is starting; logs: {}",
            host::config_dir().join("logs").display()
        ))
    }
}

#[cfg(not(windows))]
pub fn start() -> Result<(), String> {
    if !installed() {
        return enable();
    }
    install_mcp()?;
    if !wait_mcp(Duration::from_secs(8)) {
        let _ = uninstall_mcp();
        return Err(format!("MCP HTTP not up on {MCP_BASE}"));
    }
    if let Err(e) = start_supervisor() {
        let _ = uninstall_mcp();
        return Err(e);
    }
    if wait_ready(Duration::from_secs(15)) {
        eprintln!("tunnel ready  {HEALTH_BASE}/ui");
        Ok(())
    } else {
        let _ = stop_supervisor();
        Err("tunnel did not become ready".into())
    }
}

pub fn stop() -> Result<(), String> {
    stop_supervisor()?;
    eprintln!("tunnel stopped (will start again at next login if enabled).");
    Ok(())
}

pub fn has_key() -> bool {
    host::migrate_from_legacy();
    crate::secrets::get().is_some()
}

pub fn tunnel_id_opt() -> Option<String> {
    resolve_tunnel_id().ok()
}

/// Save credentials from the config UI. Empty strings are ignored.
pub fn save_connect(key: Option<&str>, tunnel_id: Option<&str>) -> Result<(), String> {
    host::migrate_from_legacy();
    if let Some(key) = key.map(str::trim).filter(|s| !s.is_empty()) {
        crate::secrets::set(key)?;
    }
    if let Some(id) = tunnel_id.map(str::trim).filter(|s| !s.is_empty()) {
        set_tunnel_id(id)?;
    }
    if can_enable() {
        enable()?;
    }
    Ok(())
}

pub fn set_tunnel_id(id: &str) -> Result<(), String> {
    let id = id.trim();
    if !id.starts_with("tunnel_") {
        return Err("tunnel id should look like tunnel_…".into());
    }
    let dir = host::config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    write_secret(&dir.join("tunnel_id"), id)
}

pub fn status_json(workspace: &Path) -> serde_json::Value {
    host::migrate_from_legacy();
    let pin = host::read_pinned_workspace();
    serde_json::json!({
        "name": host::DISPLAY,
        "unofficial": true,
        "version": env!("CARGO_PKG_VERSION"),
        "workspace": workspace.display().to_string(),
        "pin": pin.as_ref().map(|p| p.display().to_string()),
        "tunnel_ready": ready(),
        "tunnel_admin": format!("{HEALTH_BASE}/ui"),
        "service": if installed() { "enabled" } else { "off" },
        "has_key": has_key(),
        "tunnel_id": tunnel_id_opt(),
        "chatgpt": "https://chatgpt.com/plugins",
    })
}

fn can_enable() -> bool {
    persist_key().is_ok() && resolve_tunnel_id().is_ok() && tunnel_client_bin().is_ok()
}

fn persist_key() -> Result<PathBuf, String> {
    let k = crate::secrets::get().ok_or_else(|| {
        "missing runtime key. run graft setup, or export CONTROL_PLANE_API_KEY".to_string()
    })?;
    crate::secrets::ensure_file(&k)
}

fn write_secret(path: &Path, contents: &str) -> Result<(), String> {
    crate::state::atomic_write_private(path, format!("{}\n", contents.trim()))
}

fn resolve_tunnel_id() -> Result<String, String> {
    if let Ok(id) = std::env::var("CONTROL_PLANE_TUNNEL_ID") {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    if let Ok(id) = fs::read_to_string(host::config_dir().join("tunnel_id")) {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    for path in std::iter::once(profile_file()).chain(legacy_profile_files()) {
        if let Some(id) = read_tunnel_id(&path) {
            return Ok(id);
        }
    }
    Err("missing tunnel id. paste it in the config UI (graft config) or export CONTROL_PLANE_TUNNEL_ID".into())
}

fn read_tunnel_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("tunnel_id:")
            .map(|id| id.trim().trim_matches('"').trim().to_string())
            .filter(|id| !id.is_empty())
    })
}

fn write_profile(key: &Path, tunnel_id: &str) -> Result<(), String> {
    let path = profile_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    #[cfg(windows)]
    let key_path = key.display().to_string().replace('\\', "/");
    #[cfg(not(windows))]
    let key_path = key.display().to_string();
    #[cfg(windows)]
    let mcp_server = format!("    - channel: main\n      url: \"{MCP_BASE}/mcp\"\n");
    #[cfg(not(windows))]
    let mcp_server = format!(
        "    - channel: main\n      url: \"http://127.0.0.1/mcp\"\n      unix_socket: \"{}\"\n",
        host::mcp_socket().display()
    );
    let yaml = format!(
        r#"config_version: 1
control_plane:
  base_url: "https://api.openai.com"
  tunnel_id: "{tunnel_id}"
  api_key: "file:{key_path}"
  poll_timeout: 60s
health:
  listen_addr: "{HEALTH_LISTEN}"
admin_ui:
  open_browser: false
log:
  level: warn
  format: json
mcp:
  startup_wait_timeout: 30s
  server_urls:
{mcp_server}"#
    );
    crate::state::atomic_write_private(&path, yaml)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn wrapper_path() -> PathBuf {
    #[cfg(windows)]
    let path = host::config_dir().join("run-tunnel.cmd");
    #[cfg(not(windows))]
    let path = host::config_dir().join("run-tunnel.sh");
    path
}

fn write_wrapper(client: &Path) -> Result<(), String> {
    let dir = host::config_dir();
    fs::create_dir_all(dir.join("logs")).map_err(|e| format!("mkdir logs: {e}"))?;
    let path = wrapper_path();
    #[cfg(not(windows))]
    let sock = host::mcp_socket();
    let client = client.display();
    #[cfg(windows)]
    {
        let body = format!(
            "@echo off\r\n\"{client}\" run --profile {PROFILE} --log.level=warn --control-plane.poll-timeout=60s\r\n"
        );
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    #[cfg(not(windows))]
    {
        // Long-poll is the wait-for-request. MCP is HTTP-over-UDS, not stdio.
        let body = format!(
            r#"#!/bin/sh
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
CLIENT="{client}"
SOCK="{}"
set -- "$CLIENT" run --profile {PROFILE} --log.level=warn --control-plane.poll-timeout=60s \
  --mcp.startup-wait-timeout=30s \
  --mcp.server-url "url=http://127.0.0.1/mcp,channel=main,unix-socket=$SOCK"
# -is always. macOS ignores -s on battery; watch kickstarts on AC so -s
# is taken while plugged in. Frozen -i from a battery start allowed lid-sleep.
mode="${{GRAFT_CAFFEINATE:-${{PALMBRIDGE_CAFFEINATE:-${{HANDS_CAFFEINATE:-${{GROK_HARNESS_CAFFEINATE:-is}}}}}}}}"
if [ "$mode" = "auto" ]; then
  mode=is
fi
if [ -x /usr/bin/caffeinate ] && [ "$mode" != "off" ]; then
  exec /usr/bin/caffeinate -"$mode" -- "$@"
fi
if command -v systemd-inhibit >/dev/null 2>&1; then
  exec systemd-inhibit --what=idle --who=graft --why="ChatGPT MCP tunnel" --mode=block "$@"
fi
exec "$@"
"#,
            sock.display()
        );
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn harness_bin() -> Result<PathBuf, String> {
    if let Some(home) = dirs::home_dir() {
        let local = home.join(".local/bin/graft");
        if local.is_file() {
            return Ok(dunce::canonicalize(&local).unwrap_or(local));
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    Ok(dunce::canonicalize(&exe).unwrap_or(exe))
}

pub fn tunnel_client_bin() -> Result<PathBuf, String> {
    which("tunnel-client").ok_or_else(|| {
        "tunnel-client not found. brew install openai/tools/tunnel-client".into()
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        #[cfg(windows)]
        let sep = ';';
        #[cfg(not(windows))]
        let sep = ':';
        dirs.extend(path.split(sep).filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
    }
    #[cfg(windows)]
    dirs.push(PathBuf::from("C:\\Program Files\\OpenAI"));
    #[cfg(not(windows))]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = candidate.with_extension("exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn log_dir() -> PathBuf {
    host::config_dir().join("logs")
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn gui_target() -> String {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".into());
    format!("gui/{uid}")
}

#[cfg(target_os = "macos")]
pub fn installed() -> bool {
    plist_path().is_file()
}

#[cfg(target_os = "linux")]
pub fn installed() -> bool {
    unit_path().is_file()
}

#[cfg(windows)]
pub fn installed() -> bool {
    profile_file().is_file()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn installed() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn install_supervisor() -> Result<(), String> {
    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let wrapper = wrapper_path();
    let out = log_dir().join("tunnel.out");
    let err = log_dir().join("tunnel.err");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>2</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>LowPriorityIO</key><false/>
  <key>Nice</key><integer>0</integer>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&wrapper.display().to_string()),
        xml_escape(&out.display().to_string()),
        xml_escape(&err.display().to_string()),
    );
    fs::write(&plist, xml).map_err(|e| format!("write {}: {e}", plist.display()))?;
    stop_unmanaged();
    let target = gui_target();
    for label in LEGACY_LABELS {
        let _ = launchctl(&["disable", &format!("{target}/{label}")]);
        let _ = launchctl(&["bootout", &target, label]);
        let legacy_plist = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        let _ = fs::remove_file(legacy_plist);
    }
    let _ = launchctl(&["bootout", &target, LABEL]);
    let boot = launchctl(&["bootstrap", &target, &plist.display().to_string()]);
    if !boot.status.success() {
        let msg = String::from_utf8_lossy(&boot.stderr);
        if !msg.contains("already") && !msg.contains("37") {
            // try enable + kickstart anyway
        }
    }
    let _ = launchctl(&["enable", &format!("{target}/{LABEL}")]);
    let kick = launchctl(&["kickstart", "-k", &format!("{target}/{LABEL}")]);
    if !kick.status.success() {
        return Err(format!(
            "launchctl kickstart failed: {}",
            String::from_utf8_lossy(&kick.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn start_supervisor() -> Result<(), String> {
    let plist = plist_path();
    let target = gui_target();
    let _ = launchctl(&["bootstrap", &target, &plist.display().to_string()]);
    let _ = launchctl(&["enable", &format!("{target}/{LABEL}")]);
    let kick = launchctl(&["kickstart", "-k", &format!("{target}/{LABEL}")]);
    if !kick.status.success() {
        return Err(format!(
            "launchctl kickstart failed: {}",
            String::from_utf8_lossy(&kick.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn stop_supervisor() -> Result<(), String> {
    let target = gui_target();
    let _ = launchctl(&["bootout", &target, LABEL]);
    stop_unmanaged();
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_supervisor() -> Result<(), String> {
    stop_supervisor()?;
    let _ = uninstall_mcp();
    let _ = uninstall_watch();
    let plist = plist_path();
    if plist.exists() {
        fs::remove_file(&plist).map_err(|e| format!("rm {}: {e}", plist.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mcp_plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{MCP_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn install_mcp() -> Result<(), String> {
    let graft = harness_bin()?;
    let plist = mcp_plist_path();
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let out = log_dir().join("mcp.out");
    let err = log_dir().join("mcp.err");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{MCP_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--http</string>
    <string>--port</string>
    <string>8787</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>3</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&graft.display().to_string()),
        xml_escape(&out.display().to_string()),
        xml_escape(&err.display().to_string()),
    );
    fs::write(&plist, xml).map_err(|e| format!("write {}: {e}", plist.display()))?;
    let target = gui_target();
    let _ = launchctl(&["bootout", &target, MCP_LABEL]);
    let _ = launchctl(&["bootstrap", &target, &plist.display().to_string()]);
    let _ = launchctl(&["enable", &format!("{target}/{MCP_LABEL}")]);
    let _ = launchctl(&["kickstart", "-k", &format!("{target}/{MCP_LABEL}")]);
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_mcp() -> Result<(), String> {
    let target = gui_target();
    let _ = launchctl(&["bootout", &target, MCP_LABEL]);
    let plist = mcp_plist_path();
    if plist.exists() {
        fs::remove_file(&plist).map_err(|e| format!("rm {}: {e}", plist.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn watch_plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{WATCH_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn install_watch() -> Result<(), String> {
    let graft = harness_bin()?;
    let plist = watch_plist_path();
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let out = log_dir().join("watch.out");
    let err = log_dir().join("watch.err");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{WATCH_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>watch</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&graft.display().to_string()),
        xml_escape(&out.display().to_string()),
        xml_escape(&err.display().to_string()),
    );
    fs::write(&plist, xml).map_err(|e| format!("write {}: {e}", plist.display()))?;
    let target = gui_target();
    let _ = launchctl(&["bootout", &target, WATCH_LABEL]);
    let _ = launchctl(&["bootstrap", &target, &plist.display().to_string()]);
    let _ = launchctl(&["enable", &format!("{target}/{WATCH_LABEL}")]);
    let _ = launchctl(&["kickstart", "-k", &format!("{target}/{WATCH_LABEL}")]);
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_watch() -> Result<(), String> {
    let target = gui_target();
    let _ = launchctl(&["bootout", &target, WATCH_LABEL]);
    let plist = watch_plist_path();
    if plist.exists() {
        fs::remove_file(&plist).map_err(|e| format!("rm {}: {e}", plist.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn unit_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config/systemd/user/graft-tunnel.service")
}

#[cfg(target_os = "linux")]
fn install_supervisor() -> Result<(), String> {
    let unit = unit_path();
    if let Some(parent) = unit.parent() { fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?; }
    let body = format!("[Unit]\nDescription=Graft ChatGPT tunnel\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={}\nRestart=always\nRestartSec=2\nNice=0\n\n[Install]\nWantedBy=default.target\n", systemd_exec_path(&wrapper_path()));
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    for legacy in ["palmbridge-tunnel.service", "palmbridge-mcp.service", "palmbridge-watch.service", "hands-tunnel.service", "grok-harness-tunnel.service"] { let _ = Command::new("systemctl").args(["--user", "disable", "--now", legacy]).status(); }
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "graft-tunnel.service"])?;
    let _ = install_watch();
    Ok(())
}

#[cfg(target_os = "linux")]
fn mcp_unit_path() -> PathBuf { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config/systemd/user/graft-mcp.service") }

#[cfg(target_os = "linux")]
fn install_mcp() -> Result<(), String> {
    let unit = mcp_unit_path();
    if let Some(parent) = unit.parent() { fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?; }
    let body = format!("[Unit]\nDescription=Graft MCP HTTP\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} --http --port 8787\nRestart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n", systemd_exec_path(&harness_bin()?));
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "graft-mcp.service"])
}

#[cfg(target_os = "linux")]
fn uninstall_mcp() -> Result<(), String> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "graft-mcp.service"]).status();
    let unit = mcp_unit_path(); if unit.exists() { fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?; } Ok(())
}

#[cfg(target_os = "linux")]
fn watch_unit_path() -> PathBuf { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config/systemd/user/graft-watch.service") }

#[cfg(target_os = "linux")]
fn install_watch() -> Result<(), String> {
    let unit = watch_unit_path();
    if let Some(parent) = unit.parent() { fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?; }
    let body = format!("[Unit]\nDescription=Graft tunnel down notifier\nAfter=graft-tunnel.service\n\n[Service]\nType=simple\nExecStart={} watch\nRestart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n", systemd_exec_path(&harness_bin()?));
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "graft-watch.service"])
}

#[cfg(target_os = "linux")]
fn uninstall_watch() -> Result<(), String> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "graft-watch.service"]).status();
    let unit = watch_unit_path(); if unit.exists() { fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?; } Ok(())
}

#[cfg(target_os = "linux")]
fn start_supervisor() -> Result<(), String> { run_ok("systemctl", &["--user", "start", "graft-tunnel.service"]) }

#[cfg(target_os = "linux")]
fn stop_supervisor() -> Result<(), String> { let _ = Command::new("systemctl").args(["--user", "stop", "graft-tunnel.service"]).status(); stop_unmanaged(); Ok(()) }

#[cfg(target_os = "linux")]
fn uninstall_supervisor() -> Result<(), String> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "graft-tunnel.service"]).status();
    let _ = uninstall_watch(); let _ = uninstall_mcp(); stop_unmanaged();
    let unit = unit_path(); if unit.exists() { fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?; }
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status(); Ok(())
}
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
#[cfg(windows)]
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectBasicLimitInformation {
    PerProcessUserTimeLimit: i64,
    PerJobUserTimeLimit: i64,
    LimitFlags: u32,
    MinimumWorkingSetSize: usize,
    MaximumWorkingSetSize: usize,
    ActiveProcessLimit: u32,
    Affinity: usize,
    PriorityClass: u32,
    SchedulingClass: u32,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct IoCounters {
    ReadOperationCount: u64,
    WriteOperationCount: u64,
    OtherOperationCount: u64,
    ReadTransferCount: u64,
    WriteTransferCount: u64,
    OtherTransferCount: u64,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectExtendedLimitInformation {
    BasicLimitInformation: JobObjectBasicLimitInformation,
    IoInfo: IoCounters,
    ProcessMemoryLimit: usize,
    JobMemoryLimit: usize,
    PeakProcessMemoryUsed: usize,
    PeakJobMemoryUsed: usize,
}

#[cfg(windows)]
fn supervisor_job() -> Result<*mut std::ffi::c_void, String> {
    static JOB: OnceLock<usize> = OnceLock::new();
    if let Some(handle) = JOB.get() {
        return Ok(*handle as *mut std::ffi::c_void);
    }
    let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    if handle.is_null() {
        return Err("CreateJobObjectW failed".into());
    }
    let info = JobObjectExtendedLimitInformation {
        BasicLimitInformation: JobObjectBasicLimitInformation {
            PerProcessUserTimeLimit: 0,
            PerJobUserTimeLimit: 0,
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            MinimumWorkingSetSize: 0,
            MaximumWorkingSetSize: 0,
            ActiveProcessLimit: 0,
            Affinity: 0,
            PriorityClass: 0,
            SchedulingClass: 0,
        },
        IoInfo: IoCounters {
            ReadOperationCount: 0,
            WriteOperationCount: 0,
            OtherOperationCount: 0,
            ReadTransferCount: 0,
            WriteTransferCount: 0,
            OtherTransferCount: 0,
        },
        ProcessMemoryLimit: 0,
        JobMemoryLimit: 0,
        PeakProcessMemoryUsed: 0,
        PeakJobMemoryUsed: 0,
    };
    let ok = unsafe {
        SetInformationJobObject(
            handle,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
        )
    };
    if ok == 0 {
        unsafe { CloseHandle(handle) };
        return Err("SetInformationJobObject failed".into());
    }
    let _ = JOB.set(handle as usize);
    Ok(handle)
}

#[cfg(windows)]
fn assign_to_supervisor_job(child: &std::process::Child) {
    match supervisor_job() {
        Ok(job) => {
            let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) };
            if ok == 0 {
                watchdog_log("warning: AssignProcessToJobObject failed; PID fallback remains active");
            }
        }
        Err(e) => watchdog_log(&format!("warning: supervisor job unavailable: {e}")),
    }
}

// Windows supervision model: `graft start` launches a detached
// `graft --supervise` process that owns both children and restarts failures.
#[cfg(windows)]
fn supervisor_pid_file() -> PathBuf {
    host::config_dir().join("graft-supervisor.pid")
}

#[cfg(windows)]
fn tunnel_pid_file() -> PathBuf {
    host::config_dir().join("graft-tunnel.pid")
}

#[cfg(windows)]
fn mcp_pid_file() -> PathBuf {
    host::config_dir().join("graft-mcp.pid")
}

#[cfg(windows)]
fn kill_pid_str(pid: &str) {
    let _ = Command::new("taskkill")
        .args(["/PID", pid, "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
fn kill_pid_file_tree(pid_file: &Path) {
    if let Ok(pid) = fs::read_to_string(pid_file) {
        let pid = pid.trim();
        if !pid.is_empty() {
            kill_pid_str(pid);
        }
    }
}

#[cfg(windows)]
fn install_supervisor() -> Result<(), String> {
    stop_unmanaged();
    start_supervisor()
}

#[cfg(windows)]
#[allow(clippy::disallowed_methods)] // Detached Graft supervisor owns the process tree.
fn start_supervisor() -> Result<(), String> {
    // Idempotent: reuse a healthy supervisor if one is already running.
    if let Ok(pid) = fs::read_to_string(supervisor_pid_file())
        && let Ok(pid) = pid.trim().parse::<u32>()
        && process_alive(pid)
    {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let child = Command::new(exe)
        .arg("--supervise")
        .stdin(Stdio::null())
        .stdout(log_file("supervisor.out.log"))
        .stderr(log_file("supervisor.err.log"))
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("start supervisor: {e}"))?;
    fs::create_dir_all(host::config_dir()).map_err(|e| format!("mkdir config: {e}"))?;
    crate::state::atomic_write(&supervisor_pid_file(), child.id().to_string())
        .map_err(|e| format!("record supervisor pid: {e}"))?;
    Ok(())
}

#[cfg(windows)]
fn stop_supervisor() -> Result<(), String> {
    // Kill the supervisor tree first so nothing respawns mid-stop.
    kill_pid_file_tree(&supervisor_pid_file());
    kill_pid_file_tree(&tunnel_pid_file());
    kill_pid_file_tree(&mcp_pid_file());
    let _ = fs::remove_file(supervisor_pid_file());
    let _ = fs::remove_file(tunnel_pid_file());
    let _ = fs::remove_file(mcp_pid_file());
    Ok(())
}

#[cfg(windows)]
fn uninstall_supervisor() -> Result<(), String> {
    stop_supervisor()
}

/// Entry point for `graft --supervise` (detached, long-lived).
#[cfg(windows)]
pub fn supervise() {
    let _ = crate::state::atomic_write(&supervisor_pid_file(), std::process::id().to_string());
    if let Err(e) = supervisor_job() { watchdog_log(&format!("warning: could not create kill-on-close job: {e}")); }
    watchdog_log("supervisor started");
    let mcp = std::thread::spawn(|| babysit("MCP HTTP", mcp_pid_file(), spawn_mcp_process, mcp_ready));
    babysit("tunnel-client", tunnel_pid_file(), spawn_tunnel_process, ready);
    let _ = mcp.join();
}

#[cfg(windows)]
fn babysit(name: &str, pid_file: PathBuf, respawn: fn() -> Result<u32, String>, healthy: fn() -> bool) {
    let mut misses = 0;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if !supervisor_pid_file().exists() { watchdog_log(&format!("supervisor pid file removed, {name} babysit exiting")); return; }
        let pid = fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<u32>().ok());
        match pid {
            Some(pid) if process_alive(pid) && healthy() => { misses = 0; continue; }
            Some(pid) if process_alive(pid) => { misses += 1; if misses < 3 { continue; } watchdog_log(&format!("{name} {pid} hung (health failed {misses}x), killing")); kill_pid_str(&pid.to_string()); }
            Some(pid) => watchdog_log(&format!("{name} {pid} died")),
            None => watchdog_log(&format!("{name} pid file missing or corrupt, respawning")),
        }
        misses = 0;
        match respawn() { Ok(new) => watchdog_log(&format!("{name} restarted as {new}")), Err(e) => { watchdog_log(&format!("{name} respawn failed: {e}")); std::thread::sleep(Duration::from_secs(5)); } }
    }
}

/// Spawn a single tunnel-client process and return its PID.
#[cfg(windows)]
#[allow(clippy::disallowed_methods)] // The detached supervisor owns this child.
fn spawn_tunnel_process() -> Result<u32, String> {
    kill_pid_file_tree(&tunnel_pid_file());
    let _ = fs::remove_file(tunnel_pid_file());
    // Wait for port 18780 to be released instead of fixed sleep.
    wait_port_free(18780, Duration::from_secs(5));

    let child = Command::new(tunnel_client_bin()?)
        .args([
            "run",
            "--profile",
            PROFILE,
            "--log.level=warn",
            "--control-plane.poll-timeout=60s",
        ])
        .stdin(Stdio::null())
        .stdout(log_file("tunnel-client.out.log"))
        .stderr(log_file("tunnel-client.err.log"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("start tunnel-client: {e}"))?;
    assign_to_supervisor_job(&child);
    let id = child.id();
    fs::create_dir_all(host::config_dir()).map_err(|e| format!("mkdir config: {e}"))?;
    crate::state::atomic_write(&tunnel_pid_file(), id.to_string())
        .map_err(|e| format!("record tunnel pid: {e}"))?;
    Ok(id)
}

/// Spawn a single MCP HTTP process and return its PID.
#[cfg(windows)]
#[allow(clippy::disallowed_methods)] // The detached supervisor owns this child.
fn spawn_mcp_process() -> Result<u32, String> {
    uninstall_mcp()?;
    let child = Command::new(harness_bin()?)
        .args(["--http", "--port", "8787"])
        .stdin(Stdio::null())
        .stdout(log_file("mcp.out.log"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("start MCP HTTP: {e}"))?;
    assign_to_supervisor_job(&child);
    fs::create_dir_all(host::config_dir()).map_err(|e| format!("mkdir config: {e}"))?;
    crate::state::atomic_write(&mcp_pid_file(), child.id().to_string())
        .map_err(|e| format!("record MCP pid: {e}"))?;
    Ok(child.id())
}

/// Poll until a TCP port is free or timeout expires.
#[cfg(windows)]
fn wait_port_free(port: u16, timeout: Duration) {
    use std::net::TcpStream;
    let start = Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            return; // Port is free.
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Open an append-mode log file under config_dir/logs for child process output.
#[cfg(windows)]
fn log_file(name: &str) -> Stdio {
    let dir = host::config_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
    {
        Ok(file) => Stdio::from(file),
        Err(_) => Stdio::null(),
    }
}

/// Append a timestamped line to the supervisor event log.
#[cfg(windows)]
fn watchdog_log(msg: &str) {
    let dir = host::config_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("watchdog.log"))
    {
        use std::io::Write;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "{ts} {msg}");
    }
}

// Check whether a Windows process is still running via OpenProcess.
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(
        attributes: *mut std::ffi::c_void,
        name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(
        job: *mut std::ffi::c_void,
        info_class: i32,
        info: *const std::ffi::c_void,
        info_length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(
        job: *mut std::ffi::c_void,
        process: *mut std::ffi::c_void,
    ) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_TIMEOUT: u32 = 258;
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let status = WaitForSingleObject(h, 0);
        CloseHandle(h);
        status == WAIT_TIMEOUT
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn install_supervisor() -> Result<(), String> {
    Err("auto-start is implemented for macOS and Linux".into())
}

/// On macOS/Linux launchd/systemd KeepAlive supervises; Windows uses this
/// binary in --supervise mode instead.
#[cfg(not(windows))]
pub fn supervise() {}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn stop_supervisor() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn uninstall_supervisor() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn install_watch() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn install_mcp() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn uninstall_mcp() -> Result<(), String> {
    Ok(())
}


#[cfg(windows)]
fn uninstall_mcp() -> Result<(), String> {
    kill_pid_file_tree(&mcp_pid_file());
    let _ = fs::remove_file(mcp_pid_file());
    Ok(())
}

#[cfg(windows)]
fn install_watch() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn stop_unmanaged() {}

#[cfg(not(windows))]
fn stop_unmanaged() {
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,command="]).output() else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        let Some((pid, cmd)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !cmd.contains("tunnel-client") {
            continue;
        }
        let ours = cmd.contains("run --profile graft") || LEGACY_PROFILES.iter().any(|profile| cmd.contains(&format!("run --profile {profile}")));
        if !ours || cmd.contains("pkill") {
            continue;
        }
        let _ = Command::new("kill").arg(pid.trim()).status();
    }
    std::thread::sleep(Duration::from_millis(300));
}
#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> std::process::Output {
    Command::new("launchctl")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| {
            use std::os::unix::process::ExitStatusExt;
            std::process::Output {
                status: std::process::ExitStatus::from_raw(1),
                stdout: Vec::new(),
                stderr: e.to_string().into_bytes(),
            }
        })
}

#[cfg(target_os = "linux")]
fn run_ok(bin: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{bin}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{bin} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(target_os = "linux")]
fn systemd_exec_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('\\', "\\\\").replace('"', "\\\""))
}
#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
