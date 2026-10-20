#!/usr/bin/env pwsh
# install.ps1 - Install proj CLI binary into a writable directory already in PATH

param(
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$localAppData = if ($env:LOCALAPPDATA) {
    $env:LOCALAPPDATA
} elseif ($env:HOME) {
    Join-Path $env:HOME ".local/share"
} else {
    [System.IO.Path]::GetTempPath()
}
$stateDir = Join-Path $localAppData "proj"
$stateFile = Join-Path $stateDir "install-dir.txt"

function Get-ArchName {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch.ToString()) {
        "X64" { return "amd64" }
        "Arm64" { return "arm64" }
        default { throw "Unsupported architecture: $arch" }
    }
}

function Test-WritableDirectory([string]$PathEntry) {
    if (
        [string]::IsNullOrWhiteSpace($PathEntry) -or
        -not [System.IO.Path]::IsPathRooted($PathEntry) -or
        -not (Test-Path -LiteralPath $PathEntry -PathType Container)
    ) {
        return $false
    }
    try {
        $probe = Join-Path $PathEntry ".proj-write-test-$([guid]::NewGuid().ToString()).tmp"
        Set-Content -LiteralPath $probe -Value "ok" -Encoding ascii
        Remove-Item -LiteralPath $probe -Force
        return $true
    } catch {
        return $false
    }
}

function Get-WritablePathDirectory {
    $pathEntries = $env:Path -split ';'
    foreach ($entry in $pathEntries) {
        if (Test-WritableDirectory $entry) {
            return $entry
        }
    }
    throw "No writable directory found in PATH."
}

if ($Uninstall) {
    $removed = $false
    $installDir = $null
    if (Test-Path -LiteralPath $stateFile -PathType Leaf) {
        $installDir = Get-Content -LiteralPath $stateFile -TotalCount 1
    }
    if ($installDir -and [System.IO.Path]::IsPathRooted($installDir)) {
        $candidate = Join-Path $installDir "proj.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            try {
                Remove-Item -LiteralPath $candidate -Force
                Write-Host "Removed $candidate" -ForegroundColor Green
                $removed = $true
            } catch {
                Write-Warning "Could not remove ${candidate}: $($_.Exception.Message)"
            }
        }
    }

    if (-not $removed) {
        Write-Host "No proj.exe found at recorded install location." -ForegroundColor Yellow
    }
    if (Test-Path -LiteralPath $stateFile -PathType Leaf) {
        Remove-Item -LiteralPath $stateFile -Force -ErrorAction SilentlyContinue
    }
    exit 0
}

$releaseRepo = if ($env:PROJ_RELEASE_REPO) { $env:PROJ_RELEASE_REPO } else { "MiguelRodo/projcli" }
$binaryName = if ($env:PROJ_BINARY_NAME) { $env:PROJ_BINARY_NAME } else { "proj" }
if ($binaryName -notmatch '^[A-Za-z0-9_-]+$') { throw "Invalid binary name." }
$archName = Get-ArchName
$installDir = if ($env:PROJ_INSTALL_DIR) { $env:PROJ_INSTALL_DIR } else { Get-WritablePathDirectory }
if (-not (Test-WritableDirectory $installDir)) { throw "Install directory must be an existing writable absolute directory." }
$requestedVersion = if ($env:PROJ_RELEASE_VERSION) { $env:PROJ_RELEASE_VERSION -replace '^v', '' } else { $null }
$downloadBase = if ($env:PROJ_DOWNLOAD_BASE_URL) { $env:PROJ_DOWNLOAD_BASE_URL } elseif ($requestedVersion) {
    "https://github.com/$releaseRepo/releases/download/v$requestedVersion"
} else { "https://github.com/$releaseRepo/releases/latest/download" }

function Invoke-DownloadAsset([string]$Url, [string]$OutFile) {
    $uri = [uri]$Url
    if ($uri.IsFile) {
        Copy-Item -LiteralPath $uri.LocalPath -Destination $OutFile -Force
    } else {
        Invoke-WebRequest -Uri $uri -OutFile $OutFile
    }
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) "proj-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $temporary | Out-Null
$stagedBinary = $null
try {
    $versionFile = Join-Path $temporary "version.txt"
    Invoke-DownloadAsset "$downloadBase/version.txt" $versionFile
    $version = (Get-Content -LiteralPath $versionFile -Raw).TrimEnd("`r", "`n")
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?(\+[A-Za-z0-9.-]+)?$') { throw "Invalid release version metadata." }
    if ($requestedVersion -and $requestedVersion -cne $version) { throw "Downloaded version differs from the requested release." }
    $asset = "${binaryName}_${version}_windows_${archName}.exe"
    $binary = Join-Path $temporary "binary.exe"
    $checksums = Join-Path $temporary "checksums.txt"
    Invoke-DownloadAsset "$downloadBase/$asset" $binary
    Invoke-DownloadAsset "$downloadBase/checksums.txt" $checksums
    $matching = @(Get-Content -LiteralPath $checksums | Where-Object { $_ -cmatch "^[0-9a-fA-F]{64}\s+\*?$([regex]::Escape($asset))$" })
    if ($matching.Count -ne 1) { throw "Release requires exactly one matching SHA256 checksum." }
    $expected = ($matching[0] -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash
    if ($actual -ine $expected) { throw "Checksum verification failed; installed binary unchanged." }
    $target = Join-Path $installDir "$binaryName.exe"
    if (Test-Path -LiteralPath $target -PathType Container) { throw "Binary target is a directory." }
    $stagedBinary = Join-Path $installDir ".proj-install-$([guid]::NewGuid()).exe"
    Copy-Item -LiteralPath $binary -Destination $stagedBinary
    [System.IO.File]::Move($stagedBinary, $target, $true)
    $stagedBinary = $null
    New-Item -ItemType Directory -Path $stateDir -Force | Out-Null
    Set-Content -LiteralPath $stateFile -Value $installDir -Encoding ascii
    Write-Host "Installed $binaryName $version to $target (SHA256 verified)."
} finally {
    if ($stagedBinary -and (Test-Path -LiteralPath $stagedBinary)) { Remove-Item -LiteralPath $stagedBinary -Force }
    # Verify the resolved deletion target is the installer-owned temp directory.
    $resolvedTemporary = [System.IO.Path]::GetFullPath($temporary)
    $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolvedTemporary.StartsWith($temporaryRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path $resolvedTemporary -Leaf) -notmatch '^proj-install-[0-9a-f-]{36}$') {
        throw "Refusing an unexpected installer cleanup path."
    }
    Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force
}
Write-Host "Run: $binaryName --help"
