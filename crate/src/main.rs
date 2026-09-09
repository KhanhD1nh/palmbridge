//! Graft — unofficial ChatGPT plugin. Local coding tools. No model.

mod host;
mod browser;
mod mcp;
mod plugin;
mod native_glob;
mod security;
mod secrets;
mod state;
mod service;
mod setup;
mod ui;
mod watch;

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::host::{APP, DISPLAY};

const USAGE: &str = "\
Graft — unofficial ChatGPT plugin (local tools, no model)

  graft setup                      first-run checklist (TTY). no browser
  graft setup --ui                 same, then open config page
  graft config                     serve UI at http://127.0.0.1:8787/ (no browser)
  graft config --open              serve UI and open it
  cd /repo && graft use            pin this folder
  graft status [--json]
  graft enable | disable | start | stop
  graft                            MCP stdio (ChatGPT tunnel)

Debug:
  graft list
  graft call <tool> <json>
  graft --http [--port N]
  graft --supervise                detached supervisor (internal)
";

enum Cmd {
    Setup { open_ui: bool },
    Config { addr: SocketAddr, open: bool },
    Use { dir: PathBuf },
    Status { json: bool },
    Enable,
    Disable,
    Start,
    Stop,
    Supervise,
    Watch,
    McpStdio,
    McpHttp { addr: SocketAddr },
    List,
    Call { tool: String, args_json: String },
}

fn parse_args() -> Result<(PathBuf, Cmd), String> {
    let mut fallback = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut http = false;
    let mut port: u16 = 8787;
    let mut open = false;
    let mut rest = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--supervise" => return Ok((fallback, Cmd::Supervise)),
            "--cwd" => {
                let value = args.next().ok_or("--cwd requires a directory")?;
                fallback = PathBuf::from(value);
            }
            "--http" => http = true,
            "--port" => {
                let value = args.next().ok_or("--port requires a number")?;
                port = value.parse().map_err(|_| "invalid --port")?;
            }
            "--open" | "--ui" => open = true,
            "-V" | "--version" => {
                println!("{APP} {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => return Err(USAGE.trim_end().to_string()),
            _ => {
                rest.push(arg);
                rest.extend(args);
                break;
            }
        }
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let cmd = if http {
        Cmd::McpHttp { addr }
    } else {
        match rest.as_slice() {
            [] => Cmd::McpStdio,
            [op] if op == "mcp" => Cmd::McpStdio,
            [op] if op == "setup" => Cmd::Setup { open_ui: open },
            [op] if op == "config" => Cmd::Config { addr, open },
            [op] if op == "watch" => Cmd::Watch,
            [op] if op == "use" => Cmd::Use {
                dir: fallback.clone(),
            },
            [op, dir] if op == "use" => Cmd::Use {
                dir: PathBuf::from(dir),
            },
            [op] if op == "status" => Cmd::Status { json: false },
            [op, flag] if op == "status" && flag == "--json" => Cmd::Status { json: true },
            [op] if op == "enable" => Cmd::Enable,
            [op] if op == "disable" => Cmd::Disable,
            [op] if op == "start" => Cmd::Start,
            [op] if op == "stop" => Cmd::Stop,
            [op] if op == "list" => Cmd::List,
            [op, tool, json] if op == "call" => Cmd::Call {
                tool: tool.clone(),
                args_json: json.clone(),
            },
            _ => return Err(USAGE.trim_end().to_string()),
        }
    };
    Ok((fallback, cmd))
}

#[tokio::main]
async fn main() {
    host::migrate_from_legacy();
    let (fallback, cmd) = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(if e.starts_with(DISPLAY) || e.starts_with("Graft") {
                0
            } else {
                2
            });
        }
    };
    if let Err(e) = run(fallback, cmd).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(fallback: PathBuf, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Setup { open_ui } => {
            setup::run(&fallback)?;
            if open_ui {
                let addr = SocketAddr::from(([127, 0, 0, 1], 8787));
                let url = format!("http://{addr}/");
                eprintln!("{url}");
                ui::open_browser(&url);
                return mcp::McpHost::new(fallback).serve_http(addr).await;
            }
            Ok(())
        }
        Cmd::Config { addr, open } => {
            let url = format!("http://{addr}/");
            eprintln!("{url}");
            if open {
                ui::open_browser(&url);
            }
            mcp::McpHost::new(fallback).serve_http(addr).await
        }
        Cmd::Watch => watch::run(),
        Cmd::Use { dir } => {
            let path = host::resolve_project(&dir.to_string_lossy())?;
            let cwd = host::pin_workspace(&path)?;
            println!("{}", cwd.display());
            match service::ensure() {
                Ok(true) => {
                    eprintln!("pinned. ChatGPT uses this folder on the next tool call.");
                }
                Ok(false) => {
                    eprintln!("pinned. Run: graft setup");
                }
                Err(e) => eprintln!("pinned, tunnel start failed: {e}"),
            }
            Ok(())
        }
        Cmd::Status { json } => {
            let cwd = host::resolve_workspace(&fallback);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&service::status_json(&cwd))
                        .map_err(|e| e.to_string())?
                );
            } else {
                let pin = host::read_pinned_workspace();
                println!("workspace  {}", cwd.display());
                println!(
                    "pin        {}",
                    pin.map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none — using cwd/env)".into())
                );
                println!("tunnel     {}", service::status_line());
            }
            Ok(())
        }
        Cmd::Enable => service::enable(),
        Cmd::Disable => service::disable(),
        Cmd::Start => service::start(),
        Cmd::Stop => service::stop(),
        Cmd::Supervise => {
            service::supervise();
            Ok(())
        }
        Cmd::McpStdio => mcp::McpHost::new(fallback).serve_stdio().await,
        Cmd::McpHttp { addr } => mcp::McpHost::new(fallback).serve_http(addr).await,
        Cmd::List | Cmd::Call { .. } => {
            let mcp = mcp::McpHost::new(fallback);
            match cmd {
                Cmd::List => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&mcp.debug_list().await?)
                            .map_err(|e| e.to_string())?
                    );
                    Ok(())
                }
                Cmd::Call { tool, args_json } => {
                    let params: serde_json::Value = serde_json::from_str(&args_json)
                        .map_err(|e| format!("invalid json args: {e}"))?;
                    println!("{}", mcp.debug_call(&tool, params).await?);
                    Ok(())
                }
                _ => unreachable!(),
            }
        }
    }
}
