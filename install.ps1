# Palmbridge — native Windows install (port of install.sh)
# Builds palmbridge.exe from xai-org/grok-build source, downloads tunnel-client.exe.
# Requires: git, python3 (py), rustup (stable-msvc), VS Build Tools (C++).
param(
    [string]$RepoRoot = (Split-Path -Parent $MyInvocation.MyCommand.Path),
    [string]$Prefix = "$env:USERPROFILE\.local",
    [string]$Cache = "$env:USERPROFILE\.cache\palmbridge"
)
$ErrorActionPreference = "Stop"

$GROK_BUILD_URL = "https://github.com/xai-org/grok-build.git"
$GROK_BUILD_REF = "main"
$TC_VERSION = "0.0.13"

New-Item -ItemType Directory -Force -Path $Cache, "$Prefix\bin" | Out-Null
$GROK_BUILD = Join-Path $Cache "grok-build"

if (Test-Path "$GROK_BUILD\.git") {
    git -C $GROK_BUILD fetch --depth 1 origin $GROK_BUILD_REF
    git -C $GROK_BUILD checkout --force FETCH_HEAD
    git -C $GROK_BUILD clean -fdx
} else {
    git clone --depth 1 --branch $GROK_BUILD_REF $GROK_BUILD_URL $GROK_BUILD
    if ($LASTEXITCODE -ne 0) { git clone --depth 1 $GROK_BUILD_URL $GROK_BUILD }
}

py "$RepoRoot\scripts\inject.py" $RepoRoot $GROK_BUILD
if ($LASTEXITCODE -ne 0) { throw "inject.py failed" }

# Windows-only: xai-proto-build invokes protoc with /dev/stdout + /dev/null
# which do not exist on Windows. Apply the portability patch (scripts/windows-proto-build.patch).
git -C $GROK_BUILD apply "$RepoRoot\scripts\windows-proto-build.patch"
if ($LASTEXITCODE -ne 0) { throw "windows-proto-build.patch failed to apply" }
if (-not $env:PROTOC) { Write-Warning "protoc.exe not found on PATH; set PROTOC or install protobuf compiler" }

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw "rustup is required. Install: https://rustup.rs"
}

Push-Location $GROK_BUILD
try {
    cargo build --release -p palmbridge
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

Copy-Item "$GROK_BUILD\target\release\palmbridge.exe" "$Prefix\bin\palmbridge.exe" -Force

$TC_URL = "https://persistent.oaistatic.com/tunnel-client/v$TC_VERSION/tunnel-client-v$TC_VERSION-windows-amd64.zip"
$tcZip = Join-Path $Cache "tunnel-client.zip"
Invoke-WebRequest -Uri $TC_URL -OutFile $tcZip
Expand-Archive -Path $tcZip -DestinationPath (Join-Path $Cache "tunnel-client") -Force
Copy-Item (Join-Path $Cache "tunnel-client\tunnel-client.exe") "$Prefix\bin\tunnel-client.exe" -Force
Remove-Item $tcZip -Force

& "$Prefix\bin\palmbridge.exe" --version
Write-Host ""
Write-Host "installed $Prefix\bin\palmbridge.exe"
Write-Host "next: cd /your/repo && palmbridge setup"
