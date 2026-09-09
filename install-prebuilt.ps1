# Palmbridge — install prebuilt Windows binaries from GitHub Releases
# No Rust, Git, or Build Tools required.
param(
    [string]$Version = "latest",
    [string]$Prefix = "$env:USERPROFILE\.local"
)
$ErrorActionPreference = "Stop"

$REPO = "KhanhD1nh/palmbridge"
$TC_VERSION = "0.0.14"

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
$ChecksumAsset = $Release.assets | Where-Object { $_.name -eq "SHA256SUMS" } | Select-Object -First 1
if (-not $ChecksumAsset) { throw "Release $($Release.tag_name) has no SHA256SUMS; refusing unverified install" }

$Dest = "$Prefix\bin\palmbridge.exe"
$Download = "$env:TEMP\palmbridge-$([guid]::NewGuid().ToString('N')).exe"
Write-Host "Downloading $($Asset.name) ($([math]::Round($Asset.size/1MB,1)) MB)..."
Invoke-WebRequest -Uri $Asset.browser_download_url -OutFile $Download
$ChecksumFile = "$env:TEMP\palmbridge-SHA256SUMS"
Invoke-WebRequest -Uri $ChecksumAsset.browser_download_url -OutFile $ChecksumFile
$ChecksumLine = Get-Content $ChecksumFile | Where-Object { $_ -match "\s+$([regex]::Escape($Asset.name))$" } | Select-Object -First 1
if (-not $ChecksumLine) { throw "No checksum found for $($Asset.name)" }
$Expected = ($ChecksumLine -split '\s+')[0]
$Actual = (Get-FileHash -Algorithm SHA256 $Download).Hash.ToLowerInvariant()
if ($Actual -ne $Expected.ToLowerInvariant()) { throw "SHA-256 mismatch for $($Asset.name)" }
Remove-Item $ChecksumFile -Force -ErrorAction SilentlyContinue

# Only stop the active service after the replacement binary has been fully
# downloaded and verified. A failed network/checksum step leaves the working
# installation untouched.
Get-Process -Name palmbridge, tunnel-client -ErrorAction SilentlyContinue |
    ForEach-Object {
        Write-Host "Stopping $($_.Name) (PID $($_.Id))..."
        $_.Kill()
    }
Start-Sleep -Milliseconds 800
Move-Item $Download $Dest -Force
& $Dest --version

# --- Download + verify tunnel-client.exe ---
$TcName = "tunnel-client-v$TC_VERSION-windows-amd64.zip"
$TcBase = "https://github.com/openai/tunnel-client/releases/download/v$TC_VERSION"
$TcUrl = "$TcBase/$TcName"
$TcZip = "$env:TEMP\tunnel-client.zip"
Write-Host "Downloading tunnel-client v$TC_VERSION..."
Invoke-WebRequest -Uri $TcUrl -OutFile $TcZip
$TcSums = "$env:TEMP\tunnel-client-SHA256SUMS.txt"
Invoke-WebRequest -Uri "$TcBase/SHA256SUMS.txt" -OutFile $TcSums
$TcLine = Get-Content $TcSums | Where-Object { $_ -match "\s+$([regex]::Escape($TcName))$" } | Select-Object -First 1
if (-not $TcLine) { throw "No official checksum found for $TcName" }
$TcExpected = ($TcLine -split '\s+')[0].ToLowerInvariant()
$TcActual = (Get-FileHash -Algorithm SHA256 $TcZip).Hash.ToLowerInvariant()
if ($TcActual -ne $TcExpected) { throw "SHA-256 mismatch for $TcName" }
Expand-Archive -Path $TcZip -DestinationPath "$env:TEMP\tunnel-client" -Force
Copy-Item "$env:TEMP\tunnel-client\tunnel-client.exe" "$Prefix\bin\tunnel-client.exe" -Force
Remove-Item $TcZip, $TcSums, "$env:TEMP\tunnel-client" -Recurse -Force -ErrorAction SilentlyContinue

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
