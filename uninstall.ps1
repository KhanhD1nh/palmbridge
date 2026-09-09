# Remove Graft binaries and managed local processes. Keeps credentials and build cache.
param(
    [string]$Prefix = "$env:USERPROFILE\.local"
)
$ErrorActionPreference = "Stop"

$graft = Join-Path $Prefix "bin\graft.exe"
$palmbridge = Join-Path $Prefix "bin\palmbridge.exe" # Legacy cleanup.
$tunnelClient = Join-Path $Prefix "bin\tunnel-client.exe"
if (Test-Path $graft) {
    & $graft stop 2>$null
} elseif (Test-Path $palmbridge) {
    & $palmbridge stop 2>$null
}

Remove-Item $graft, $palmbridge, $tunnelClient -Force -ErrorAction SilentlyContinue
Write-Host "removed Graft and tunnel-client from $Prefix\bin"
Write-Host "kept configuration and cache; remove $env:APPDATA\graft and $env:USERPROFILE\.cache\graft manually to purge them"
