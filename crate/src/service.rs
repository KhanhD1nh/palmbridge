//! Keep `tunnel-client` running: login start + restart if the MCP child dies.
//!
//! Foreground `tunnel-client run` exits when its stdio MCP child is killed.
//! A LaunchAgent / systemd user unit with KeepAlive is the actual client.

use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::host;

pub const HEALTH_LISTEN: &str = "127.0.0.1:18780";
pub const HEALTH_BASE: &str = "http://127.0.0.1:18780";
pub const MCP_BASE: &str = "http://127.0.0.1:8787";
pub const PROFILE: &str = "palmbridge";
#[cfg(target_os = "macos")]
const LABEL: &str = "dev.palmbridge.tunnel";
#[cfg(target_os = "macos")]
const MCP_LABEL: &str = "dev.palmbridge.mcp";
#[cfg(target_os = "macos")]
const WATCH_LABEL: &str = "dev.palmbridge.watch";
#[cfg(target_os = "macos")]
const LEGACY_LABELS: &[&str] = &["dev.hands.tunnel", "ai.grok.harness.tunnel"];
const LEGACY_PROFILES: &[&str] = &["hands", "grok-harness"];

pub fn profile_file() -> PathBuf {
    host::tunnel_client_dir().join(format!("{PROFILE}.yaml"))
}

fn legacy_profile_files() -> impl Iterator<Item = PathBuf> {
    LEGACY_PROFILES
        .iter()
        .map(|profile| host::tunnel_client_dir().join(format!("{profile}.yaml")))
}

pub fn ready() -> bool {
    ureq_get(&format!("{HEALTH_BASE}/readyz"))
        .ok()
        .is_some_and(|s| s == "ready")
}

pub fn mcp_ready() -> bool {
    ureq_get(&format!("{MCP_BASE}/healthz"))
        .ok()
        .is_some_and(|s| s == "ok")
}

fn wait_mcp(timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if mcp_ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    mcp_ready()
}

pub fn wait_ready(timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    ready()
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
        "off — palmbridge setup"
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
        eprintln!("tunnel on. login start + restart. config: palmbridge config");
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
        "missing runtime key. run palmbridge setup, or export CONTROL_PLANE_API_KEY".to_string()
    })?;
    crate::secrets::ensure_file(&k)
}

fn write_secret(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(path, format!("{}\n", contents.trim()))
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
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
    Err("missing tunnel id. paste it in the config UI (palmbridge config) or export CONTROL_PLANE_TUNNEL_ID".into())
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
    let mut key_path = key.display().to_string();
    // YAML double-quoted scalars treat backslashes as escapes ("\U" → unicode).
    // Windows paths must use forward slashes (accepted by Go and Command::new).
    #[cfg(windows)]
    {
        key_path = key_path.replace('\\', "/");
    }
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
    fs::write(&path, yaml).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
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
mode="${{PALMBRIDGE_CAFFEINATE:-${{HANDS_CAFFEINATE:-${{GROK_HARNESS_CAFFEINATE:-is}}}}}}"
if [ "$mode" = "auto" ]; then
  mode=is
fi
if [ -x /usr/bin/caffeinate ] && [ "$mode" != "off" ]; then
  exec /usr/bin/caffeinate -"$mode" -- "$@"
fi
if command -v systemd-inhibit >/dev/null 2>&1; then
  exec systemd-inhibit --what=idle --who=palmbridge --why="ChatGPT MCP tunnel" --mode=block "$@"
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
        let local = home.join(".local/bin/palmbridge");
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
    let palmbridge = harness_bin()?;
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
        xml_escape(&palmbridge.display().to_string()),
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
    let palmbridge = harness_bin()?;
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
        xml_escape(&palmbridge.display().to_string()),
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
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user/palmbridge-tunnel.service")
}

#[cfg(target_os = "linux")]
fn install_supervisor() -> Result<(), String> {
    let unit = unit_path();
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let wrapper_buf = wrapper_path();
    let wrapper = systemd_exec_path(&wrapper_buf);
    let body = format!(
        r#"[Unit]
Description=Palmbridge ChatGPT tunnel
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={wrapper}
Restart=always
RestartSec=2
Nice=0

[Install]
WantedBy=default.target
"#
    );
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    stop_unmanaged();
    for legacy in ["hands-tunnel.service", "grok-harness-tunnel.service"] {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", legacy])
            .status();
    }
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "palmbridge-tunnel.service"])?;
    let _ = install_watch();
    Ok(())
}

#[cfg(target_os = "linux")]
fn mcp_unit_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user/palmbridge-mcp.service")
}

#[cfg(target_os = "linux")]
fn install_mcp() -> Result<(), String> {
    let palmbridge = harness_bin()?;
    let unit = mcp_unit_path();
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = format!(
        r#"[Unit]
Description=Palmbridge MCP HTTP
After=network-online.target

[Service]
Type=simple
ExecStart={bin} --http --port 8787
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"#,
        bin = systemd_exec_path(&palmbridge)
    );
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "palmbridge-mcp.service"])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_mcp() -> Result<(), String> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "palmbridge-mcp.service"])
        .status();
    let unit = mcp_unit_path();
    if unit.exists() {
        fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn watch_unit_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user/palmbridge-watch.service")
}

#[cfg(target_os = "linux")]
fn install_watch() -> Result<(), String> {
    let palmbridge = harness_bin()?;
    let unit = watch_unit_path();
    if let Some(parent) = unit.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = format!(
        r#"[Unit]
Description=Palmbridge tunnel down notifier
After=palmbridge-tunnel.service

[Service]
Type=simple
ExecStart={} watch
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
"#,
        systemd_exec_path(&palmbridge)
    );
    fs::write(&unit, body).map_err(|e| format!("write {}: {e}", unit.display()))?;
    run_ok("systemctl", &["--user", "daemon-reload"])?;
    run_ok("systemctl", &["--user", "enable", "--now", "palmbridge-watch.service"])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_watch() -> Result<(), String> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "palmbridge-watch.service"])
        .status();
    let unit = watch_unit_path();
    if unit.exists() {
        fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_supervisor() -> Result<(), String> {
    run_ok("systemctl", &["--user", "start", "palmbridge-tunnel.service"])
}

#[cfg(target_os = "linux")]
fn stop_supervisor() -> Result<(), String> {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", "palmbridge-tunnel.service"])
        .status();
    stop_unmanaged();
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_supervisor() -> Result<(), String> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "palmbridge-tunnel.service"])
        .status();
    let _ = uninstall_watch();
    let _ = uninstall_mcp();
    stop_unmanaged();
    let unit = unit_path();
    if unit.exists() {
        fs::remove_file(&unit).map_err(|e| format!("rm {}: {e}", unit.display()))?;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok(())
}

#[cfg(windows)]
fn install_supervisor() -> Result<(), String> {
    stop_unmanaged();
    start_supervisor()
}

#[cfg(windows)]
fn start_supervisor() -> Result<(), String> {
    spawn_tunnel()?;
    if wait_ready(Duration::from_secs(15)) {
        Ok(())
    } else {
        Err("tunnel did not become ready".into())
    }
}

#[cfg(windows)]
fn stop_supervisor() -> Result<(), String> {
    uninstall_mcp()?;
    let pid_file = host::config_dir().join("palmbridge-tunnel.pid");
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let pid = pid.trim();
        if !pid.is_empty() {
            let _ = Command::new("taskkill")
                .args(["/PID", pid, "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    // Wait for taskkill to finish before removing the PID file.
    // The watchdog checks pid_file.exists() first; removing it signals
    // the watchdog to exit. If we remove it while the process is still
    // dying, the watchdog could see "file exists + process dead" and
    // respawn an orphan.
    std::thread::sleep(Duration::from_millis(500));
    let _ = fs::remove_file(&pid_file);
    Ok(())
}

#[cfg(windows)]
fn uninstall_supervisor() -> Result<(), String> {
    stop_supervisor()
}
// Windows has no persistent supervisor: start the tunnel-client directly and
// retain its PID so `palmbridge stop` terminates the exact child it started.
// A background watchdog thread restarts it if the process dies unexpectedly.
#[cfg(windows)]
static WATCHDOG_RUNNING: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
fn spawn_tunnel() -> Result<(), String> {
    // Prevent multiple watchdog threads from accumulating across start/stop cycles.
    if WATCHDOG_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // Already running.
    }
    let pid = spawn_tunnel_process()?;
    std::thread::spawn(move || tunnel_watchdog(pid));
    Ok(())
}

/// Spawn a single tunnel-client process and return its PID.
#[cfg(windows)]
fn spawn_tunnel_process() -> Result<u32, String> {
    let pid_file = host::config_dir().join("palmbridge-tunnel.pid");
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let _ = Command::new("taskkill")
            .args(["/PID", pid.trim(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = fs::remove_file(&pid_file);
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
        .spawn()
        .map_err(|e| format!("start tunnel-client: {e}"))?;
    let id = child.id();
    fs::create_dir_all(host::config_dir()).map_err(|e| format!("mkdir config: {e}"))?;
    fs::write(
        host::config_dir().join("palmbridge-tunnel.pid"),
        id.to_string(),
    )
    .map_err(|e| format!("record tunnel pid: {e}"))?;
    Ok(id)
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

/// Append a timestamped line to the watchdog event log.
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

/// Background loop: poll every 15s, restart tunnel-client if it died.
/// Exits when the PID file is removed (i.e. `stop_supervisor` was called).
#[cfg(windows)]
fn tunnel_watchdog(mut current_pid: u32) {
    let pid_file = host::config_dir().join("palmbridge-tunnel.pid");
    watchdog_log("tunnel watchdog started");
    loop {
        std::thread::sleep(Duration::from_secs(15));
        // If PID file was removed, stop was requested — exit watchdog.
        if !pid_file.exists() {
            watchdog_log("pid file removed, tunnel watchdog exiting");
            WATCHDOG_RUNNING.store(false, Ordering::SeqCst);
            return;
        }
        if process_alive(current_pid) {
            continue;
        }
        watchdog_log(&format!("tunnel-client {current_pid} died"));
        match spawn_tunnel_process() {
            Ok(new_pid) => {
                watchdog_log(&format!("tunnel-client restarted as {new_pid}"));
                eprintln!("[watchdog] tunnel-client {current_pid} died, restarted as {new_pid}");
                current_pid = new_pid;
            }
            Err(e) => {
                watchdog_log(&format!("failed to restart tunnel-client: {e}"));
                eprintln!("[watchdog] failed to restart tunnel-client: {e}");
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn install_supervisor() -> Result<(), String> {
    Err("auto-start is implemented for macOS and Linux".into())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn start_supervisor() -> Result<(), String> {
    Err("auto-start is implemented for macOS and Linux".into())
}

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
fn install_mcp() -> Result<(), String> {
    uninstall_mcp()?;
    let child = Command::new(harness_bin()?)
        .args(["--http", "--port", "8787"])
        .stdin(Stdio::null())
        .stdout(log_file("mcp.out.log"))
        .stderr(log_file("mcp.err.log"))
        .spawn()
        .map_err(|e| format!("start MCP HTTP: {e}"))?;
    fs::create_dir_all(host::config_dir()).map_err(|e| format!("mkdir config: {e}"))?;
    fs::write(host::config_dir().join("palmbridge-mcp.pid"), child.id().to_string())
        .map_err(|e| format!("record MCP pid: {e}"))?;
    // Start MCP watchdog so it auto-restarts if it dies mid-session.
    std::thread::spawn(mcp_watchdog);
    Ok(())
}

#[cfg(windows)]
fn uninstall_mcp() -> Result<(), String> {
    let pid_file = host::config_dir().join("palmbridge-mcp.pid");
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let _ = Command::new("taskkill")
            .args(["/PID", pid.trim(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = fs::remove_file(pid_file);
    Ok(())
}

/// Background loop: restart MCP HTTP process if it dies while tunnel is up.
#[cfg(windows)]
fn mcp_watchdog() {
    watchdog_log("mcp watchdog started");
    let pid_file = host::config_dir().join("palmbridge-mcp.pid");
    loop {
        std::thread::sleep(Duration::from_secs(15));
        if !pid_file.exists() {
            watchdog_log("mcp pid file removed, mcp watchdog exiting");
            return; // uninstalled
        }
        let pid = match fs::read_to_string(&pid_file) {
            Ok(s) => match s.trim().parse::<u32>() {
                Ok(p) => p,
                Err(_) => continue,
            },
            Err(_) => return,
        };
        if process_alive(pid) {
            continue;
        }
        watchdog_log(&format!("MCP HTTP {pid} died"));
        // MCP died. Respawn.
        if let Ok(bin) = harness_bin() {
            if let Ok(child) = Command::new(bin)
                .args(["--http", "--port", "8787"])
                .stdin(Stdio::null())
                .stdout(log_file("mcp.out.log"))
                .stderr(log_file("mcp.err.log"))
                .spawn()
            {
                let _ = fs::write(&pid_file, child.id().to_string());
                watchdog_log(&format!("MCP HTTP restarted as {}", child.id()));
                eprintln!("[watchdog] MCP HTTP {pid} died, restarted as {}", child.id());
            } else {
                watchdog_log("failed to restart MCP HTTP");
            }
        } else {
            watchdog_log("harness binary missing, cannot restart MCP HTTP");
        }
    }
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
        let ours = cmd.contains("run --profile palmbridge") || LEGACY_PROFILES.iter().any(|profile| cmd.contains(&format!("run --profile {profile}")));
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

pub fn ureq_get(url: &str) -> Result<String, ()> {
    let mut child = Command::new("curl")
        .args(["-fsS", "--max-time", "1", url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let mut buf = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut buf);
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if ok {
        Ok(buf.trim().to_string())
    } else {
        Err(())
    }
}
