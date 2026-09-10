//! Workspace pin + ToolBridge. Unofficial; runtime from xai-org/grok-build.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use xai_grok_tools::bridge::ToolBridge;
use xai_grok_tools::computer::local::{LocalFs, LocalTerminalBackend};
use xai_grok_tools::implementations::codex::ApplyPatchTool;
use xai_grok_tools::implementations::grok_build::LspTool;
use xai_grok_tools::implementations::lsp::config::{LspServerConfig, load_servers};
use xai_grok_tools::implementations::lsp::{LspBackend, LspBackendAdapter, LspManager};
use xai_grok_tools::implementations::{
    BashTool, GrepTool, KillTaskTool, ListDirTool, OpenCodeGlobTool, OpenCodeWriteTool,
    ReadFileTool, SearchReplaceTool, TaskOutputTool, TodoWriteTool,
};
use xai_grok_tools::notification::ToolNotificationHandle;
use xai_grok_tools::registry::types::{SessionContext, ToolConfig, ToolServerConfig};
use xai_grok_tools::reminders::DEFAULT_REMINDER_TAG;

pub const APP: &str = "graft";
pub const DISPLAY: &str = "Graft";

static LEGACY_MIGRATION: OnceLock<()> = OnceLock::new();
#[cfg(windows)]
static NPM_GLOBAL_PREFIX: OnceLock<Option<PathBuf>> = OnceLock::new();

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// XDG on Unix (`~/.config/graft`). `%APPDATA%\graft` on Windows.
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    return dirs::config_dir()
        .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
        .join(APP);
    #[cfg(not(windows))]
    home_dir().join(".config").join(APP)
}

pub fn tunnel_client_dir() -> PathBuf {
    #[cfg(windows)]
    return dirs::config_dir()
        .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
        .join("tunnel-client");
    #[cfg(not(windows))]
    home_dir().join(".config/tunnel-client")
}

pub fn workspace_file() -> PathBuf {
    config_dir().join("workspace")
}

#[cfg(not(windows))]
pub fn mcp_socket() -> PathBuf {
    config_dir().join("mcp.sock")
}

/// Copy existing configuration without overwriting Graft state.
pub fn migrate_from_legacy() {
    LEGACY_MIGRATION.get_or_init(|| {
        let dest = config_dir();
        let mut sources = Vec::new();
        #[cfg(windows)]
        {
            let base = dirs::config_dir().unwrap_or_else(|| home_dir().join("AppData/Roaming"));
            sources.push(base.join("palmbridge"));
            sources.push(base.join("hands"));
        }
        #[cfg(not(windows))]
        {
            let base = home_dir().join(".config");
            sources.push(base.join("palmbridge"));
            sources.push(base.join("hands"));
            sources.push(base.join("grok-harness"));
        }
        for src in sources {
            if !src.is_dir() {
                continue;
            }
            let _ = std::fs::create_dir_all(&dest);
            for name in ["workspace", "control-plane.key", "tunnel_id", "recent"] {
                let from = src.join(name);
                let to = dest.join(name);
                if from.is_file() && !to.exists() {
                    let _ = std::fs::copy(&from, &to);
                }
            }
        }
    });
}

pub fn read_pinned_workspace() -> Option<PathBuf> {
    migrate_from_legacy();
    let raw = std::fs::read_to_string(workspace_file()).ok()?;
    let path = PathBuf::from(raw.trim());
    if path.is_dir() {
        dunce::canonicalize(&path).ok()
    } else {
        None
    }
}

pub fn pin_workspace(dir: &Path) -> Result<PathBuf, String> {
    migrate_from_legacy();
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let cwd = dunce::canonicalize(dir).map_err(|e| format!("canonicalize: {e}"))?;
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    crate::state::atomic_write(&workspace_file(), format!("{}\n", cwd.display()))
        .map_err(|e| format!("write workspace pin: {e}"))?;
    remember_workspace(&cwd);
    Ok(cwd)
}

fn recent_file() -> PathBuf {
    config_dir().join("recent")
}

pub fn read_recent() -> Vec<PathBuf> {
    migrate_from_legacy();
    let Ok(text) = std::fs::read_to_string(recent_file()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let p = PathBuf::from(line.trim());
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
        if out.len() >= 20 {
            break;
        }
    }
    out
}

pub(crate) fn remember_workspace(cwd: &Path) {
    let mut items = read_recent();
    items.retain(|p| p != cwd);
    items.insert(0, cwd.to_path_buf());
    items.truncate(20);
    let body: String = items.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = crate::state::atomic_write(&recent_file(), body);
}

fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if raw == "~" {
        return home_dir();
    }
    PathBuf::from(raw)
}

/// Resolve a path or short name (`bunko`, `~/Dev/bunko`) to an existing directory.
pub fn resolve_project(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty path".into());
    }
    let expanded = expand_tilde(raw);
    if expanded.is_dir() {
        return dunce::canonicalize(&expanded).map_err(|e| e.to_string());
    }
    let mut tried = vec![expanded.clone()];
    if !expanded.is_absolute() {
        let name = Path::new(raw);
        tried.push(home_dir().join(name));
        tried.push(home_dir().join("Dev").join(name));
        tried.push(home_dir().join("dev").join(name));
        if let Some(pin) = read_pinned_workspace() {
            tried.push(pin.join(name));
            if let Some(parent) = pin.parent() {
                tried.push(parent.join(name));
            }
        }
    }
    for c in &tried {
        if c.is_dir() {
            return dunce::canonicalize(c).map_err(|e| e.to_string());
        }
    }
    Err(format!(
        "not a directory: {raw}. Try an absolute path or a folder name under ~/Dev."
    ))
}

/// Active workspace: env → pin file → `--cwd`/process cwd.
pub fn resolve_workspace(fallback: &Path) -> PathBuf {
    migrate_from_legacy();
    for var in [
        "PALMBRIDGE_WORKSPACE",
        "HANDS_WORKSPACE",
        "GROK_HARNESS_WORKSPACE",
    ] {
        if let Ok(env_path) = std::env::var(var) {
            let p = PathBuf::from(env_path);
            if let Ok(c) = dunce::canonicalize(&p)
                && c.is_dir()
            {
                return c;
            }
        }
    }
    if let Some(pinned) = read_pinned_workspace() {
        return pinned;
    }
    dunce::canonicalize(fallback).unwrap_or_else(|_| fallback.to_path_buf())
}

fn allowlist() -> ToolServerConfig {
    ToolServerConfig {
        tools: vec![
            ToolConfig::from(&ReadFileTool),
            ToolConfig::from(&GrepTool),
            ToolConfig::from(&ListDirTool),
            ToolConfig::from(&OpenCodeGlobTool),
            ToolConfig::from(&SearchReplaceTool),
            ToolConfig::from(&OpenCodeWriteTool),
            ToolConfig::from(&ApplyPatchTool),
            ToolConfig::from(&TodoWriteTool),
            ToolConfig::from(&LspTool),
            ToolConfig::from(&BashTool)
                .with_param("enabled_background", true)
                .with_param("auto_background_on_timeout", true),
            ToolConfig::from(&TaskOutputTool),
            ToolConfig::from(&KillTaskTool),
        ],
        behavior_preset: None,
    }
}

fn session_context(cwd: PathBuf, owner_session_id: &str) -> SessionContext {
    let host_dir = std::env::temp_dir().join(APP);
    let session_dir = host_dir
        .join("sessions")
        .join(session_folder_name(owner_session_id));
    let _ = std::fs::create_dir_all(&session_dir);
    let notification_handle = ToolNotificationHandle::noop();
    let lsp = build_lsp_backend(&cwd, notification_handle.clone());
    SessionContext {
        backend: Arc::new(LocalTerminalBackend::new()),
        fs: Arc::new(LocalFs),
        cwd,
        session_folder: session_dir,
        session_env: Arc::new(HashMap::new()),
        notification_handle,
        owner_session_id: Some(owner_session_id.to_string()),
        subagent: None,
        parent_scheduler_handle: None,
        skills: vec![],
        // Runtime state is already held by this bridge. Persisting it under a
        // process-global temp path leaks todos/tool state across MCP sessions.
        state_path: PathBuf::new(),
        memory_backend: None,
        web_search_config: Default::default(),
        web_fetch_config: Default::default(),
        lsp,
        image_gen_config: Default::default(),
        video_gen_config: Default::default(),
        app_builder_deployer_config: Default::default(),
        api_key_provider: None,
        auth_provider: None,
        attribution_callback: None,
        system_reminder_tag: DEFAULT_REMINDER_TAG,
    }
}

fn session_folder_name(owner_session_id: &str) -> String {
    let mut out = String::with_capacity(owner_session_id.len().min(96));
    for c in owner_session_id.chars().take(96) {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

fn build_lsp_backend(
    cwd: &Path,
    notification_handle: ToolNotificationHandle,
) -> Option<Arc<dyn LspBackend>> {
    let mut servers = load_servers(cwd);
    add_auto_detected_lsp_servers(cwd, &mut servers);
    if servers.is_empty() {
        return None;
    }
    let manager = LspManager::new(servers, cwd.to_path_buf(), true, notification_handle);
    Some(Arc::new(LspBackendAdapter::new(Arc::new(
        tokio::sync::Mutex::new(manager),
    ))))
}

fn add_auto_detected_lsp_servers(cwd: &Path, servers: &mut BTreeMap<String, LspServerConfig>) {
    let claimed = |ext: &str, servers: &BTreeMap<String, LspServerConfig>| {
        servers.values().any(|cfg| cfg.extensions.contains_key(ext))
    };

    if looks_like_typescript_project(cwd)
        && !claimed(".ts", servers)
        && let Some((command, mut args)) = find_language_server(cwd, "typescript-language-server")
    {
        args.push("--stdio".into());
        servers.insert(
            "typescript".into(),
            LspServerConfig {
                command,
                args,
                extensions: HashMap::from([
                    (".ts".into(), "typescript".into()),
                    (".tsx".into(), "typescriptreact".into()),
                    (".mts".into(), "typescript".into()),
                    (".cts".into(), "typescript".into()),
                    (".js".into(), "javascript".into()),
                    (".jsx".into(), "javascriptreact".into()),
                ]),
                restart_on_crash: Some(true),
                ..Default::default()
            },
        );
    }

    if looks_like_python_project(cwd)
        && !claimed(".py", servers)
        && let Some((command, mut args)) = find_language_server(cwd, "pyright-langserver")
    {
        args.push("--stdio".into());
        servers.insert(
            "python".into(),
            LspServerConfig {
                command,
                args,
                extensions: HashMap::from([(".py".into(), "python".into())]),
                restart_on_crash: Some(true),
                ..Default::default()
            },
        );
    }
}

fn looks_like_typescript_project(cwd: &Path) -> bool {
    [
        "package.json",
        "tsconfig.json",
        "jsconfig.json",
        "deno.json",
        "deno.jsonc",
    ]
    .iter()
    .any(|name| cwd.join(name).is_file())
}

fn looks_like_python_project(cwd: &Path) -> bool {
    [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "setup.cfg",
        "Pipfile",
        "poetry.lock",
    ]
    .iter()
    .any(|name| cwd.join(name).is_file())
}

fn find_language_server(cwd: &Path, name: &str) -> Option<(String, Vec<String>)> {
    // Rustup proxies can be shadowed by repo-specific shims on PATH. Resolve
    // the actual component first so Graft does not accidentally start a broken
    // rust-analyzer from another workspace.
    if name == "rust-analyzer"
        && let Ok(output) = Command::new("rustup")
            .args(["which", "rust-analyzer"])
            .output()
        && output.status.success()
    {
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if path.is_file() {
            return Some((path.display().to_string(), Vec::new()));
        }
    }

    let local_bin = cwd.join("node_modules").join(".bin");
    #[cfg(windows)]
    for ext in ["cmd", "exe", "bat"] {
        let candidate = local_bin.join(format!("{name}.{ext}"));
        if candidate.is_file() {
            return Some(command_for_path(candidate));
        }
    }
    #[cfg(not(windows))]
    {
        let candidate = local_bin.join(name);
        if candidate.is_file() {
            return Some((candidate.display().to_string(), Vec::new()));
        }
    }

    // PATH is effectively free compared with spawning npm. npm's global bin
    // directory is normally already on PATH, so check it before the fallback.
    if let Some(command) = find_language_server_on_path(name) {
        return Some(command);
    }

    #[cfg(windows)]
    if let Some(prefix) = npm_global_prefix() {
        for ext in ["cmd", "exe", "bat"] {
            let candidate = prefix.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(command_for_path(candidate));
            }
        }
    }

    None
}

fn find_language_server_on_path(name: &str) -> Option<(String, Vec<String>)> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(command_for_path(candidate));
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(command_for_path(candidate));
            }
        }
    }
    None
}

#[cfg(windows)]
fn npm_global_prefix() -> Option<PathBuf> {
    NPM_GLOBAL_PREFIX
        .get_or_init(|| {
            let output = Command::new("npm.cmd")
                .args(["prefix", "-g"])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            path.is_dir().then_some(path)
        })
        .clone()
}

fn command_for_path(path: PathBuf) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let is_script = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        if is_script {
            return (
                "cmd.exe".into(),
                vec![
                    "/d".into(),
                    "/s".into(),
                    "/c".into(),
                    path.display().to_string(),
                ],
            );
        }
    }
    (path.display().to_string(), Vec::new())
}

pub async fn build_bridge(cwd: PathBuf, owner_session_id: &str) -> Result<ToolBridge, String> {
    let mut builder = ToolBridge::get_builder();
    builder.set_system_reminders_enabled(false);
    ToolBridge::finalize_builder(builder, allowlist(), session_context(cwd, owner_session_id))
        .await
        .map_err(|e| e.to_string())
}
