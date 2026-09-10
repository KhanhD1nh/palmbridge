//! Check GitHub Releases and replace the running Graft binary with a verified update.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use semver::Version;
use sha2::{Digest, Sha256};

use crate::service;

const RELEASE_API: &str = "https://api.github.com/repos/KhanhD1nh/palmbridge/releases/latest";
const START_CHECK_TIMEOUT: Duration = Duration::from_secs(4);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct Release {
    tag: String,
    version: Version,
    json: serde_json::Value,
}

pub async fn check_on_start() {
    match latest_release(START_CHECK_TIMEOUT).await {
        Ok(release) if release.version > current_version() => {
            eprintln!(
                "update available  v{} -> {}; run: graft update",
                current_version(),
                release.tag
            );
        }
        Ok(_) => {}
        Err(e) => eprintln!("update check failed (continuing): {e}"),
    }
}

pub async fn run() -> Result<(), String> {
    eprintln!("checking for updates...");
    let release = latest_release(UPDATE_TIMEOUT).await?;
    let current = current_version();
    if release.version <= current {
        eprintln!("graft v{current} is already up to date.");
        return Ok(());
    }

    let asset_name = platform_asset()?;
    let binary_url = asset_url(&release.json, asset_name)?;
    let checksum_url = asset_url(&release.json, "SHA256SUMS")?;
    eprintln!("updating graft v{current} -> {}", release.tag);
    eprintln!("downloading {asset_name}...");

    let client = github_client(UPDATE_TIMEOUT)?;
    let (binary, sums) = tokio::try_join!(
        download_bytes(&client, &binary_url),
        download_bytes(&client, &checksum_url)
    )?;
    verify_checksum(asset_name, &binary, &sums)?;
    eprintln!("verified SHA-256");

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("resolve current executable: {e}"))?;
    let staged = staged_path(&current_exe, &release.version)?;
    if staged.exists() {
        fs::remove_file(&staged).map_err(|e| format!("remove stale update file: {e}"))?;
    }
    fs::write(&staged, &binary).map_err(|e| {
        format!(
            "write update next to {}: {e}",
            current_exe.display()
        )
    })?;

    let was_running = service::ready() || service::mcp_ready();
    if was_running {
        eprintln!("stopping tunnel before replacing graft...");
        if let Err(e) = service::stop() {
            let _ = fs::remove_file(&staged);
            return Err(format!("could not stop tunnel before update: {e}"));
        }
    }

    if let Err(e) = self_replace::self_replace(&staged) {
        let _ = fs::remove_file(&staged);
        if was_running {
            let _ = service::start();
        }
        return Err(format!("replace {}: {e}", current_exe.display()));
    }
    let _ = fs::remove_file(&staged);

    eprintln!("updated graft to {}", release.tag);
    if was_running {
        eprintln!("restarting tunnel with the new version...");
        restart_with_new_binary(&current_exe);
    }
    Ok(())
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version must be valid semver")
}

async fn latest_release(timeout: Duration) -> Result<Release, String> {
    let client = github_client(timeout)?;
    let response = client
        .get(RELEASE_API)
        .send()
        .await
        .map_err(|e| format!("GitHub release request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GitHub release request: {e}"))?;
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse GitHub release response: {e}"))?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or("GitHub release response has no tag_name")?
        .to_string();
    let version = Version::parse(tag.trim_start_matches('v'))
        .map_err(|e| format!("invalid release tag {tag}: {e}"))?;
    Ok(Release { tag, version, json })
}

fn github_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("graft/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(2))
        .timeout(timeout)
        .build()
        .map_err(|e| format!("build update HTTP client: {e}"))
}

async fn download_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download {url}: {e}"))?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|e| format!("read {url}: {e}"))
}

fn asset_url(release: &serde_json::Value, name: &str) -> Result<String, String> {
    release
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                if asset.get("name").and_then(serde_json::Value::as_str) == Some(name) {
                    asset
                        .get("browser_download_url")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| format!("release has no asset named {name}"))
}

fn verify_checksum(asset_name: &str, binary: &[u8], sums: &[u8]) -> Result<(), String> {
    let sums = std::str::from_utf8(sums).map_err(|e| format!("SHA256SUMS is not UTF-8: {e}"))?;
    let expected = checksum_for_asset(sums, asset_name)
        .ok_or_else(|| format!("SHA256SUMS has no entry for {asset_name}"))?;
    let actual = format!("{:x}", Sha256::digest(binary));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch for {asset_name}: expected {expected}, got {actual}"
        ))
    }
}

fn checksum_for_asset<'a>(sums: &'a str, asset_name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name == asset_name && hash.len() == 64 {
            Some(hash)
        } else {
            None
        }
    })
}

fn staged_path(current_exe: &Path, version: &Version) -> Result<PathBuf, String> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", current_exe.display()))?;
    let extension = current_exe
        .extension()
        .and_then(|s| s.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default();
    Ok(parent.join(format!(
        ".graft-update-{}-v{}{}",
        std::process::id(),
        version,
        extension
    )))
}

fn restart_with_new_binary(current_exe: &Path) {
    match Command::new(current_exe).arg("start").status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "warning: graft was updated, but tunnel restart exited with {status}; run: graft start"
        ),
        Err(e) => eprintln!(
            "warning: graft was updated, but tunnel restart failed: {e}; run: graft start"
        ),
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn platform_asset() -> Result<&'static str, String> {
    Ok("graft-windows-x86_64.exe")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn platform_asset() -> Result<&'static str, String> {
    Ok("graft-linux-x86_64")
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn platform_asset() -> Result<&'static str, String> {
    Ok("graft-macos-aarch64")
}

#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
fn platform_asset() -> Result<&'static str, String> {
    Err(format!(
        "automatic updates are not published for {}-{}; use the source installer instead",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lookup_accepts_sha256sum_format() {
        let sums = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  graft-linux-x86_64\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *graft-windows-x86_64.exe\n";
        assert_eq!(
            checksum_for_asset(sums, "graft-windows-x86_64.exe"),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
    }

    #[test]
    fn checksum_verification_rejects_tampering() {
        let digest = format!("{:x}", Sha256::digest(b"expected"));
        let sums = format!("{digest}  graft-linux-x86_64\n");
        assert!(verify_checksum("graft-linux-x86_64", b"expected", sums.as_bytes()).is_ok());
        assert!(verify_checksum("graft-linux-x86_64", b"tampered", sums.as_bytes()).is_err());
    }

    #[test]
    fn release_asset_lookup_requires_exact_name() {
        let release = serde_json::json!({
            "assets": [
                {"name": "graft-windows-x86_64.exe", "browser_download_url": "https://example.test/graft.exe"},
                {"name": "SHA256SUMS", "browser_download_url": "https://example.test/SHA256SUMS"}
            ]
        });
        assert_eq!(
            asset_url(&release, "graft-windows-x86_64.exe").unwrap(),
            "https://example.test/graft.exe"
        );
        assert!(asset_url(&release, "graft-windows-arm64.exe").is_err());
    }
}
