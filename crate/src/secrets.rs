//! Runtime key: macOS Keychain (preferred) + 0600 file for the tunnel daemon.

use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::host;

const SERVICE: &str = "dev.palmbridge.runtime-key";
const ACCOUNT: &str = "palmbridge";

pub fn valid_runtime_key(key: &str) -> bool {
    key.starts_with("sk-") && key.len() >= 32 && !key.contains(char::is_whitespace)
}

pub fn key_file() -> PathBuf {
    host::config_dir().join("control-plane.key")
}

pub fn get() -> Option<String> {
    if let Ok(k) = std::env::var("CONTROL_PLANE_API_KEY") {
        let k = k.trim().to_string();
        if valid_runtime_key(&k) {
            return Some(k);
        }
    }
    if let Ok(file) = fs::read_to_string(key_file()) {
        let k = file.trim().to_string();
        if valid_runtime_key(&k) {
            return Some(k);
        }
    }
    // Keychain can prompt; only from a TTY (palmbridge setup), never LaunchAgent.
    if std::io::stdin().is_terminal() {
        if let Some(k) = keychain_get() {
            if valid_runtime_key(&k) {
                let _ = ensure_file(&k);
                return Some(k);
            }
        }
    }
    None
}

pub fn set(key: &str) -> Result<PathBuf, String> {
    let key = key.trim();
    if !valid_runtime_key(key) {
        return Err("runtime key looks invalid (need sk-… from platform.openai.com)".into());
    }
    let path = ensure_file(key)?;
    if std::io::stdin().is_terminal() {
        match keychain_set(key) {
            Ok(()) => {}
            Err(e) => eprintln!("keychain: {e} (key kept in {})", path.display()),
        }
    }
    Ok(path)
}

pub fn ensure_file(key: &str) -> Result<PathBuf, String> {
    let path = key_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(&path, format!("{key}\n")).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn keychain_get() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let k = String::from_utf8(out.stdout).ok()?.trim().to_string();
        return if k.is_empty() { None } else { Some(k) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        secret_tool_get()
    }
}

fn keychain_set(key: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // -U replace; -A allow this machine's agents (LaunchAgent) without a prompt each start.
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-A",
                "-s",
                SERVICE,
                "-a",
                ACCOUNT,
                "-w",
                key,
                "-l",
                "Palmbridge ChatGPT runtime key",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .map_err(|e| format!("security: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err("macOS Keychain write failed".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        secret_tool_set(key)
    }
}

#[cfg(not(target_os = "macos"))]
fn secret_tool_get() -> Option<String> {
    let out = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", ACCOUNT])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let k = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if k.is_empty() { None } else { Some(k) }
}

#[cfg(not(target_os = "macos"))]
fn secret_tool_set(key: &str) -> Result<(), String> {
    let mut child = match Command::new("secret-tool")
        .args([
            "store",
            "--label",
            "Palmbridge ChatGPT runtime key",
            "service",
            SERVICE,
            "account",
            ACCOUNT,
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Ok(()), // no libsecret; file is enough
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(key.as_bytes());
    }
    let _ = child.wait();
    Ok(())
}
