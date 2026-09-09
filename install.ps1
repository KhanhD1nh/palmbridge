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
$GROK_BUILD_REF = (Get-Content (Join-Path $RepoRoot "GROK_BUILD_REVISION") -Raw).Trim()
$TC_VERSION = "0.0.14"

New-Item -ItemType Directory -Force -Path $Cache, "$Prefix\bin" | Out-Null
$GROK_BUILD = Join-Path $Cache "grok-build"

if (Test-Path "$GROK_BUILD\.git") {
    git -C $GROK_BUILD fetch --depth 1 origin $GROK_BUILD_REF
    git -C $GROK_BUILD checkout --force FETCH_HEAD
    git -C $GROK_BUILD clean -fdx
} else {
    New-Item -ItemType Directory -Force -Path $GROK_BUILD | Out-Null
    git -C $GROK_BUILD init
    if ($LASTEXITCODE -ne 0) { throw "git init grok-build failed" }
    git -C $GROK_BUILD remote add origin $GROK_BUILD_URL
    git -C $GROK_BUILD fetch --depth 1 origin $GROK_BUILD_REF
    if ($LASTEXITCODE -ne 0) { throw "fetch pinned grok-build revision failed" }
    git -C $GROK_BUILD checkout --detach FETCH_HEAD
    if ($LASTEXITCODE -ne 0) { throw "checkout pinned grok-build revision failed" }
}

$Python = Get-ChildItem "$env:LOCALAPPDATA\Programs\Python\Python*\python.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $Python) {
    $Python = (Get-Command py -ErrorAction SilentlyContinue).Source
}
if (-not $Python) {
    throw "Python 3 is required. Install: https://www.python.org/downloads/windows/"
}
& $Python "$RepoRoot\scripts\inject.py" $RepoRoot $GROK_BUILD
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

$TC_NAME = "tunnel-client-v$TC_VERSION-windows-amd64.zip"
$TC_BASE = "https://github.com/openai/tunnel-client/releases/download/v$TC_VERSION"
$TC_URL = "$TC_BASE/$TC_NAME"
$tcZip = Join-Path $Cache "tunnel-client.zip"
Invoke-WebRequest -Uri $TC_URL -OutFile $tcZip
$tcSums = Join-Path $Cache "tunnel-client-SHA256SUMS.txt"
Invoke-WebRequest -Uri "$TC_BASE/SHA256SUMS.txt" -OutFile $tcSums
$tcLine = Get-Content $tcSums | Where-Object { $_ -match "\s+$([regex]::Escape($TC_NAME))$" } | Select-Object -First 1
if (-not $tcLine) { throw "No official checksum found for $TC_NAME" }
$tcExpected = ($tcLine -split '\s+')[0].ToLowerInvariant()
$tcActual = (Get-FileHash -Algorithm SHA256 $tcZip).Hash.ToLowerInvariant()
if ($tcActual -ne $tcExpected) { throw "SHA-256 mismatch for $TC_NAME" }
Expand-Archive -Path $tcZip -DestinationPath (Join-Path $Cache "tunnel-client") -Force
Copy-Item (Join-Path $Cache "tunnel-client\tunnel-client.exe") "$Prefix\bin\tunnel-client.exe" -Force
Remove-Item $tcZip, $tcSums -Force

& "$Prefix\bin\palmbridge.exe" --version
Write-Host ""
Write-Host "installed $Prefix\bin\palmbridge.exe"
Write-Host "next: cd /your/repo && palmbridge setup"
