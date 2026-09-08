# Palmbridge — install prebuilt Windows binaries from GitHub Releases
# No Rust, Git, or Build Tools required.
param(
    [string]$Version = "latest",
    [string]$Prefix = "$env:USERPROFILE\.local"
)
$ErrorActionPreference = "Stop"

$REPO = "KhanhD1nh/palmbridge"
$TC_VERSION = "0.0.13"

New-Item -ItemType Directory -Force -Path "$Prefix\bin" | Out-Null

# --- Download palmbridge.exe ---
if ($Version -eq "latest") {
    $ApiUrl = "https://api.github.com/repos/$REPO/releases/latest"
} else {
    $ApiUrl = "https://api.github.com/repos/$REPO/releases/tags/$Version"
}
Write-Host "Fetching release info from $ApiUrl ..."
$Release = Invoke-RestMethod -Uri $ApiUrl -Headers @{ "User-Agent" = "palmbridge-installer" }
$Asset = $Release.assets | Where-Object { $_.name -like "palmbridge-windows-*.exe" } | Select-Object -First 1
if (-not $Asset) { throw "No Windows asset found in release $($Release.tag_name)" }

$Dest = "$Prefix\bin\palmbridge.exe"
Write-Host "Downloading $($Asset.name) ($([math]::Round($Asset.size/1MB,1)) MB)..."
Invoke-WebRequest -Uri $Asset.browser_download_url -OutFile $Dest
& $Dest --version

# --- Download tunnel-client.exe ---
$TcUrl = "https://persistent.oaistatic.com/tunnel-client/v$TC_VERSION/tunnel-client-v$TC_VERSION-windows-amd64.zip"
$TcZip = "$env:TEMP\tunnel-client.zip"
Write-Host "Downloading tunnel-client v$TC_VERSION..."
Invoke-WebRequest -Uri $TcUrl -OutFile $TcZip
Expand-Archive -Path $TcZip -DestinationPath "$env:TEMP\tunnel-client" -Force
Copy-Item "$env:TEMP\tunnel-client\tunnel-client.exe" "$Prefix\bin\tunnel-client.exe" -Force
Remove-Item $TcZip, "$env:TEMP\tunnel-client" -Recurse -Force -ErrorAction SilentlyContinue

# --- Add to PATH if not already ---
$BinDir = "$Prefix\bin"
$CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($CurrentPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$CurrentPath;$BinDir", "User")
    $env:Path = "$env:Path;$BinDir"
    Write-Host "Added $BinDir to user PATH."
} else {
    Write-Host "$BinDir already in PATH."
}

Write-Host ""
Write-Host "Installed successfully. Run:"
Write-Host "  palmbridge setup"
