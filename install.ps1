# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
# SPDX-License-Identifier: Apache-2.0
# CigScript installer for Windows. Downloads a binary release, verifies its
# sha256 (and the gh attestation when gh is on PATH), and offers to add the
# install directory to the user's PATH. No admin rights required.
#
#   Review first (recommended):
#     irm https://raw.githubusercontent.com/otmof-ops/CigScript/v1.3.0/install.ps1 -OutFile install.ps1
#     notepad install.ps1 ; .\install.ps1
#
#   One-liner (reads CIG_VERSION, CIG_PREFIX, CIG_NO_MODIFY_PATH, CIG_YES from
#   the environment because `iex` cannot pass parameters):
#     irm https://raw.githubusercontent.com/otmof-ops/CigScript/main/install.ps1 | iex
#
#   Options:
#     -Version vX.Y.Z    install this release (default: the newest stable release)
#     -Prefix DIR        install directory (default: %LOCALAPPDATA%\Programs\cig)
#     -NoModifyPath      never touch the user's PATH
#     -Yes               answer yes to every question

param(
    [string]$Version,
    [string]$Prefix,
    [switch]$NoModifyPath,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'

# Environment-variable fallbacks for `irm ... | iex`, where parameters cannot
# be passed on the command line.
if (-not $Version -and $env:CIG_VERSION) { $Version = $env:CIG_VERSION }
if (-not $Prefix) {
    if ($env:CIG_PREFIX) { $Prefix = $env:CIG_PREFIX }
    else { $Prefix = Join-Path $env:LOCALAPPDATA 'Programs\cig' }
}
if (-not $NoModifyPath -and $env:CIG_NO_MODIFY_PATH) { $NoModifyPath = $true }
if (-not $Yes -and $env:CIG_YES) { $Yes = $true }

$Repo = 'otmof-ops/CigScript'

function Say([string]$m) { Write-Host $m }
function Fail([string]$m) {
    Write-Host "install.ps1: error: $m" -ForegroundColor Red
    exit 1
}
function Ask([string]$m) {
    if ($Yes) { return $true }
    $ans = Read-Host "$m [y/N]"
    return ($ans -match '^(y|Y|yes|YES)$')
}
function Have([string]$n) { [bool](Get-Command $n -ErrorAction SilentlyContinue) }

# Force TLS 1.2+ on Windows PowerShell 5.1, where it is not the default.
try {
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

# The processor architecture, from the environment.
$archRaw = $env:PROCESSOR_ARCHITECTURE
switch ($archRaw) {
    'AMD64' { $arch = 'x86_64' }
    'ARM64' { $arch = 'aarch64' }
    default { Fail "unsupported architecture '$archRaw' (expected AMD64 or ARM64)" }
}

Say "CigScript installer: windows/$arch, prefix $Prefix"

# Resolve the release tag.
if ($Version) {
    $tag = $Version
} else {
    Say 'resolving the newest release...'
    try {
        $rel = Invoke-RestMethod -UseBasicParsing `
            -Headers @{ 'User-Agent' = 'cigscript-install' } `
            -Uri "https://api.github.com/repos/$Repo/releases/latest"
    } catch {
        Fail "could not read releases for $Repo (a private repository needs ``gh``; try -Version)"
    }
    $tag = $rel.tag_name
    if (-not $tag) { Fail 'no release tag found' }
}
if ($tag -notmatch '^v') { $tag = "v$tag" }

$asset = "cig-$tag-windows-$arch.exe"
$base = "https://github.com/$Repo/releases/download/$tag"
$work = Join-Path $env:TEMP ('cig-install-' + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $work | Out-Null

try {
    Say "downloading $asset ($tag)..."
    $assetPath = Join-Path $work $asset
    $sumPath = "$assetPath.sha256"

    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $assetPath
    } catch {
        Fail "no binary for windows-$arch in $tag"
    }
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset.sha256" -OutFile $sumPath
    } catch {
        Fail "release ships no checksum for $asset; refusing to install"
    }

    # The sha256 file is `<hex>  <name>`; take the first field.
    $expected = ((Get-Content -Raw $sumPath).Trim() -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $assetPath).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "checksum mismatch for $asset (expected $expected, got $actual)"
    }
    Say 'verified: sha256 matches the published checksum (integrity, not authorship; see SECURITY.md)'

    # The build attestation names the release workflow that produced the
    # asset; gh checks it against Sigstore when it is installed.
    if (Have 'gh') {
        $null = & gh attestation verify $assetPath -R $Repo 2>&1
        if ($LASTEXITCODE -eq 0) {
            Say "attested: the build provenance names $Repo's release workflow"
        } else {
            Say 'attestation: gh could not verify the provenance (no network, an old gh, or not logged in); the checksum stands'
        }
    }

    # Install.
    if (-not (Test-Path $Prefix)) {
        New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
    }
    $target = Join-Path $Prefix 'cig.exe'
    Move-Item -Force $assetPath $target
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# PATH handling. Add to the USER scope only if the prefix is not already there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$parts = @()
if ($userPath) { $parts = $userPath -split ';' | Where-Object { $_ } }
$already = $false
$prefixFull = $null
try { $prefixFull = [IO.Path]::GetFullPath($Prefix) } catch { $prefixFull = $Prefix }
foreach ($p in $parts) {
    try {
        if ([IO.Path]::GetFullPath($p) -ieq $prefixFull) {
            $already = $true
            break
        }
    } catch { }
}
if (-not $already) {
    if (-not $NoModifyPath) {
        if (Ask "add $Prefix to your user PATH?") {
            if ($userPath) { $new = "$userPath;$Prefix" } else { $new = $Prefix }
            [Environment]::SetEnvironmentVariable('Path', $new, 'User')
            Say "added $Prefix to the user PATH; open a new terminal for the change to take effect"
        } else {
            Say "$Prefix is not on your PATH; add it yourself when ready"
        }
    } else {
        Say "$Prefix is not on your PATH; add it yourself when ready"
    }
}

# Final version print.
Say ''
try {
    $cig = Join-Path $Prefix 'cig.exe'
    & $cig --version
} catch {
    Say "installed, but could not run ``cig --version`` from $Prefix"
}
