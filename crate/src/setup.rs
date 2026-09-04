//! First-run checklist. TTY prompts; no browser. Agents use env + PALMBRIDGE_NO_UI.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use crate::host;
use crate::secrets;
use crate::service;

pub fn run(dir: &Path) -> Result<(), String> {
    host::migrate_from_legacy();
    let cwd = host::pin_workspace(dir)?;

    let client_ok = service::tunnel_client_bin().is_ok();
    let mut key_ok = secrets::get().is_some();
    let mut id_ok = service::tunnel_id_opt().is_some();
    let tty = io::stdin().is_terminal();

    eprintln!("Palmbridge setup");
    check("workspace", true, &cwd.display().to_string());
    check(
        "tunnel-client",
        client_ok,
        if client_ok {
            "ok"
        } else {
            "missing — brew install openai/tools/tunnel-client"
        },
    );
    check("runtime key", key_ok, if key_ok { "saved" } else { "missing" });
    check("tunnel id", id_ok, if id_ok { "saved" } else { "missing" });

    if !client_ok {
        return Err("install tunnel-client first".into());
    }

    if !key_ok && tty {
        eprint!("paste runtime key (hidden): ");
        let _ = io::stderr().flush();
        if let Some(k) = read_secret()? {
            secrets::set(&k)?;
            key_ok = true;
            eprintln!("saved to Keychain");
        }
    }
    if !id_ok && tty {
        eprint!("paste tunnel id (tunnel_…): ");
        let _ = io::stderr().flush();
        if let Some(id) = read_line()? {
            service::set_tunnel_id(&id)?;
            id_ok = true;
        }
    }

    if !key_ok || !id_ok {
        if tty {
            return Err("need both runtime key and tunnel id. or: palmbridge config --open".into());
        }
        return Err(
            "non-interactive: set CONTROL_PLANE_API_KEY and CONTROL_PLANE_TUNNEL_ID".into(),
        );
    }

    service::enable()?;
    if let Some(id) = service::tunnel_id_opt() {
        copy_clip(&id);
        eprintln!();
        eprintln!("tunnel id (copied): {id}");
        eprintln!("ChatGPT → chatgpt.com/plugins → Developer mode → Tunnel → paste → Scan tools");
        eprintln!("Skip confirm: first write → Always allow, or Settings → Apps → Palmbridge → Never ask");
    }
    Ok(())
}

fn check(name: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "·" };
    eprintln!("  [{mark}] {name:<14} {detail}");
}

fn read_line() -> Result<Option<String>, String> {
    let mut s = String::new();
    io::stdin()
        .lock()
        .read_line(&mut s)
        .map_err(|e| e.to_string())?;
    let t = s.trim().to_string();
    Ok(if t.is_empty() { None } else { Some(t) })
}

fn read_secret() -> Result<Option<String>, String> {
    #[cfg(not(windows))]
    let _ = Command::new("stty").arg("-echo").status();
    let mut s = String::new();
    let r = io::stdin().lock().read_line(&mut s);
    #[cfg(not(windows))]
    {
        let _ = Command::new("stty").arg("echo").status();
        eprintln!();
    }
    r.map_err(|e| e.to_string())?;
    let t = s.trim().to_string();
    Ok(if t.is_empty() { None } else { Some(t) })
}

fn copy_clip(text: &str) {
    let try_copy = |bin: &str, args: &[&str]| {
        let mut c = Command::new(bin);
        c.args(args);
        c.stdin(std::process::Stdio::piped());
        if let Ok(mut child) = c.spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return true;
        }
        false
    };
    if cfg!(target_os = "macos") {
        let _ = try_copy("pbcopy", &[]);
    } else if cfg!(target_os = "windows") {
        let _ = try_copy("clip.exe", &[]);
    } else {
        let _ = try_copy("wl-copy", &[]) || try_copy("xclip", &["-selection", "clipboard"]);
    }
}
