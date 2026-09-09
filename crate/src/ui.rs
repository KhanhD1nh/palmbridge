//! Local config page for humans. Agents use `graft status --json` / `graft setup`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::host;
use crate::service;

const PAGE: &str = include_str!("ui.html");

pub fn route(method: &str, path: &str, body: &[u8]) -> Option<(u16, &'static str, Vec<u8>)> {
    let path = path.split('?').next().unwrap_or(path);
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") | ("GET", "/ui") => Some((
            200,
            "text/html; charset=utf-8",
            PAGE.as_bytes().to_vec(),
        )),
        ("GET", "/api/status") => {
            let ws = host::resolve_workspace(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let payload = service::status_json(&ws);
            Some(json_ok(200, payload))
        }
        ("POST", "/api/workspace") => Some(handle_workspace(body)),
        ("POST", "/api/connect") => Some(handle_connect(body)),
        ("POST", "/api/enable") => Some(map_result(service::enable())),
        ("POST", "/api/disable") => Some(map_result(service::disable())),
        ("POST", "/api/start") => Some(map_result(service::start())),
        ("POST", "/api/stop") => Some(map_result(service::stop())),
        _ => None,
    }
}

fn handle_workspace(body: &[u8]) -> (u16, &'static str, Vec<u8>) {
    match parse(body).and_then(|v| {
        let path = v
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "path required".to_string())?;
        host::pin_workspace(Path::new(path)).map(|p| p.display().to_string())
    }) {
        Ok(path) => json_ok(200, json!({ "ok": true, "workspace": path })),
        Err(e) => json_ok(400, json!({ "error": e })),
    }
}

fn handle_connect(body: &[u8]) -> (u16, &'static str, Vec<u8>) {
    match parse(body).and_then(|v| {
        if let Some(path) = v.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            host::pin_workspace(Path::new(path))?;
        }
        service::save_connect(
            v.get("key").and_then(Value::as_str),
            v.get("tunnel_id").and_then(Value::as_str),
        )
    }) {
        Ok(()) => json_ok(200, json!({ "ok": true })),
        Err(e) => json_ok(400, json!({ "error": e })),
    }
}

fn map_result(r: Result<(), String>) -> (u16, &'static str, Vec<u8>) {
    match r {
        Ok(()) => json_ok(200, json!({ "ok": true })),
        Err(e) => json_ok(400, json!({ "error": e })),
    }
}

fn parse(body: &[u8]) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|e| format!("json: {e}"))
}

fn json_ok(status: u16, v: Value) -> (u16, &'static str, Vec<u8>) {
    (
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec(&v).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

#[allow(clippy::disallowed_methods)] // OS opener intentionally outlives this short CLI command.
pub fn open_browser(url: &str) {
    let _ = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
}
