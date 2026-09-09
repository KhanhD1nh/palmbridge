//! Notify when the tunnel drops. On macOS, restart the tunnel when AC is plugged
//! in so caffeinate -s is created while on adapter (lid/system sleep blocked).

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::service;

pub fn run() -> Result<(), String> {
    let mut seen_ready = false;
    let mut notified_at: Option<Instant> = None;
    let mut was_ac = on_ac();
    // Let enable/kickstart finish before the first sample.
    thread::sleep(Duration::from_secs(20));
    loop {
        let ac = on_ac();
        if ac && !was_ac {
            restart_tunnel();
            thread::sleep(Duration::from_secs(8));
        }
        was_ac = ac;

        let ready = service::ready();
        if ready {
            seen_ready = true;
            notified_at = None;
        } else if seen_ready {
            let due = match notified_at {
                None => true,
                Some(t) => t.elapsed() > Duration::from_secs(30 * 60),
            };
            if due {
                notify(
                    "Graft",
                    "Tunnel is down. ChatGPT cannot reach this Mac until it is back.",
                );
                notified_at = Some(Instant::now());
            }
        }
        thread::sleep(Duration::from_secs(15));
    }
}

fn on_ac() -> bool {
    Command::new("pmset")
        .args(["-g", "ps"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|s| s.contains("AC Power"))
}

fn restart_tunnel() {
    #[cfg(target_os = "macos")]
    {
        let uid = Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "501".into());
        let _ = Command::new("launchctl")
            .args(["kickstart", "-k", &format!("gui/{uid}/dev.graft.tunnel")])
            .status();
    }
}

fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"Basso\"",
            escape_as(body),
            escape_as(title)
        );
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = Command::new("notify-send").args([title, body]).status();
    }
}

#[cfg(target_os = "macos")]
fn escape_as(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
