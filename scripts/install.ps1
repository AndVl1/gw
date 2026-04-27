<#
.SYNOPSIS
    Install gw — Gradle output filter for AI coding agents.

.DESCRIPTION
    Downloads the latest gw release tarball for Windows from GitHub and installs
    to %LOCALAPPDATA%\Programs\gw. Adds the directory to user PATH.

.PARAMETER Version
    Pin version (e.g. v0.2.4). Default: latest GitHub release.

.PARAMETER InstallDir
    Install dir. Default: $env:LOCALAPPDATA\Programs\gw.

.PARAMETER NoVerify
    Skip sha256 verification.

.EXAMPLE
    irm https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.ps1 | iex

.EXAMPLE
    & ([scriptblock]::Create((irm https://raw.githubusercontent.com/AndVl1/gw/main/scripts/install.ps1))) -Version v0.2.4
#>
[CmdletBinding()]
param(
    [string]$Version = $env:GW_VERSION,
    [string]$InstallDir = $env:GW_INSTALL_DIR,
    [switch]$NoVerify
)

$ErrorActionPreference = "Stop"
$Repo = "AndVl1/gw"

function Info($msg) { Write-Host "gw-install: $msg" -ForegroundColor Cyan }
function Fail($msg) { Write-Host "gw-install: $msg" -ForegroundColor Red; exit 1 }

if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\gw"
}

$arch = (Get-CimInstance Win32_Processor).Architecture
switch ($arch) {
    9  { $archTarget = "x86_64" }
    12 { $archTarget = "aarch64" }
    default { Fail "unsupported arch (CIM Architecture=$arch)" }
}

$Target = "$archTarget-pc-windows-msvc"

if (-not $Version) {
    Info "resolving latest release..."
    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    $Version = $latest.tag_name
    if (-not $Version) { Fail "could not resolve latest version" }
}

$VerNum = $Version.TrimStart("v")
$Archive = "gw-$VerNum-$Target.zip"
$Url = "https://github.com/$Repo/releases/download/$Version/$Archive"
$ShaUrl = "$Url.sha256"

$tmp = Join-Path $env:TEMP "gw-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    Info "downloading $Archive ($Version)..."
    Invoke-WebRequest -Uri $Url -OutFile (Join-Path $tmp $Archive) -UseBasicParsing

    if (-not $NoVerify) {
        Info "verifying sha256..."
        $shaFile = Join-Path $tmp "$Archive.sha256"
        Invoke-WebRequest -Uri $ShaUrl -OutFile $shaFile -UseBasicParsing
        $expected = (Get-Content $shaFile -Raw).Trim().Split()[0].ToLower()
        $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $Archive)).Hash.ToLower()
        if ($expected -ne $actual) {
            Fail "sha256 mismatch: expected=$expected actual=$actual"
        }
    }

    Info "extracting..."
    Expand-Archive -Path (Join-Path $tmp $Archive) -DestinationPath $tmp -Force

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }

    $extractedDir = Join-Path $tmp "gw-$VerNum-$Target"
    Copy-Item -Force (Join-Path $extractedDir "gw.exe") (Join-Path $InstallDir "gw.exe")

    Info "installed: $InstallDir\gw.exe"

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not ($userPath -split ";" | Where-Object { $_ -eq $InstallDir })) {
        Info "adding $InstallDir to user PATH..."
        $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        Write-Host ""
        Write-Host "  Restart your shell to pick up PATH changes." -ForegroundColor Yellow
    }

    & (Join-Path $InstallDir "gw.exe") --version
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
