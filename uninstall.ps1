# Remove Palmbridge binaries and managed local processes. Keeps credentials and build cache.
param(
    [string]$Prefix = "$env:USERPROFILE\.local"
)
$ErrorActionPreference = "Stop"

$palmbridge = Join-Path $Prefix "bin\palmbridge.exe"
$tunnelClient = Join-Path $Prefix "bin\tunnel-client.exe"
if (Test-Path $palmbridge) {
    & $palmbridge stop 2>$null
}

Remove-Item $palmbridge, $tunnelClient -Force -ErrorAction SilentlyContinue
Write-Host "removed Palmbridge and tunnel-client from $Prefix\bin"
Write-Host "kept configuration and cache; remove $env:APPDATA\palmbridge and $env:USERPROFILE\.cache\palmbridge manually to purge them"
