<#
.SYNOPSIS
  Build Claw Fleet into a Windows installer (.exe, NSIS) in one shot.

.DESCRIPTION
  The Windows counterpart of scripts/build-local.sh (macOS). It mirrors the
  Windows job in .github/workflows/ci.yml (desktop-build matrix):

    1. Build the `fleet` CLI sidecar (release) for the host MSVC target.
    2. Stage it as claw-fleet-desktop/binaries/fleet-<target>.exe (the
       externalBin Tauri bundles) - only when the bytes changed, so Tauri's
       build-script watcher does not force a needless desktop recompile.
    3. pnpm install the frontend workspace.
    4. `tauri build` - its beforeBuildCommand runs `pnpm build` to emit ./dist,
       then it compiles the desktop crate and packages the NSIS installer.
    5. Copy the produced setup .exe to dist/claw-fleet-windows-x64-setup.exe.

  Cargo.toml is NOT mutated unless -Version is passed, so the in-app version
  stays 0.0.0 and cargo's incremental cache is preserved across runs (same
  rationale as build-local.sh).

  Real releases still happen via GitHub CI on a tag push (ci.yml + release.yml).
  This script is a local dogfooding / hand-off packaging tool, not a release
  pipeline (no code signing - Windows will show a SmartScreen warning).

.PARAMETER DebugProfile
  Build the debug profile (fast incremental) instead of the optimised release
  profile. The installer still works; it is just larger and slower.

.PARAMETER Version
  Optional x.y.z string. When set, stamps the three crate manifests that CI
  stamps (claw-fleet-core, claw-fleet-desktop, fleet-cli) so the installer and
  in-app "About" show that version. Leaves the edit in place - `git checkout`
  to revert.

.PARAMETER SkipInstall
  Skip `pnpm install` (use when node_modules is already current).

.EXAMPLE
  .\scripts\build-windows.ps1
  .\scripts\build-windows.ps1 -DebugProfile
  .\scripts\build-windows.ps1 -Version 0.2.0
#>
[CmdletBinding()]
param(
  [switch]$DebugProfile,
  [string]$Version,
  [switch]$SkipInstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Info($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Warn($msg) { Write-Host "==> $msg" -ForegroundColor Yellow }
function Die($msg)  { Write-Host "ERROR: $msg" -ForegroundColor Red; exit 1 }

# Locate repo root (this script lives in <root>/scripts)
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir   = Split-Path -Parent $ScriptDir
Set-Location $RootDir
$Desktop = Join-Path $RootDir 'claw-fleet-desktop'

# Make common toolchain locations importable even from a bare shell.
# rustup puts cargo here; the local Node was unpacked to C:\tools\node.
$maybePaths = @("$env:USERPROFILE\.cargo\bin", 'C:\tools\node')
foreach ($p in $maybePaths) {
  if ((Test-Path $p) -and (($env:Path -split ';') -notcontains $p)) {
    $env:Path = "$p;$env:Path"
  }
}

# Preflight: required tools
function Need($cmd, $hint) {
  $c = Get-Command $cmd -ErrorAction SilentlyContinue
  if (-not $c) { Die "$cmd not found on PATH. $hint" }
  return $c.Source
}
Info "Preflight"
$null = Need 'cargo' 'Install Rust from https://rustup.rs (stable, MSVC toolchain).'
$null = Need 'rustc' 'Install Rust from https://rustup.rs.'
if (-not (Get-Command 'pnpm' -ErrorAction SilentlyContinue)) {
  # corepack ships with Node - try to light pnpm up transparently.
  if (Get-Command 'corepack' -ErrorAction SilentlyContinue) {
    Info "Enabling pnpm via corepack"
    corepack enable | Out-Null
    corepack prepare pnpm@latest --activate | Out-Null
  }
}
$null = Need 'pnpm' 'Install Node.js 20+ (https://nodejs.org) then: corepack enable; corepack prepare pnpm@latest --activate'

# WebView2 is the runtime the packaged app needs; warn (do not fail) if absent.
$wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
if (-not $wv) { Warn "WebView2 runtime not detected. The installed app will prompt to install it on first run." }

# Profile + target
if ($DebugProfile) { $BuildProfile = 'debug';  $CargoProfileFlag = @() }
else               { $BuildProfile = 'release'; $CargoProfileFlag = @('--release') }

# Host triple, e.g. x86_64-pc-windows-msvc. externalBin resolution always
# appends this triple to the sidecar filename regardless of --target.
$Target = (& rustc -vV | Select-String '^host:').ToString().Split(' ')[-1].Trim()
if (-not $Target) { Die "Could not determine host target triple from rustc -vV." }

Info "Root:    $RootDir"
Info "Profile: $BuildProfile"
Info "Target:  $Target"

# Optional version stamp (mirrors CI's sed step)
if ($Version) {
  if ($Version -notmatch '^\d+\.\d+\.\d+') { Die "Version must look like x.y.z (got '$Version')." }
  Info "Stamping version $Version into crate manifests"
  foreach ($rel in 'claw-fleet-core\Cargo.toml','claw-fleet-desktop\Cargo.toml','fleet-cli\Cargo.toml') {
    $toml = Join-Path $RootDir $rel
    $txt  = Get-Content $toml -Raw
    $txt  = [regex]::Replace($txt, '(?m)^version = ".*"', "version = `"$Version`"", 1)
    Set-Content -Path $toml -Value $txt -Encoding utf8 -NoNewline
  }
}

# 1. Build the fleet CLI sidecar
Info "Building fleet CLI sidecar ($BuildProfile)..."
$env:OPENSSL_STATIC = '1'   # matches CI; harmless on Windows (desktop/CLI use SChannel)
& cargo build @CargoProfileFlag -p fleet-cli --target $Target
if ($LASTEXITCODE -ne 0) { Die "cargo build of fleet-cli failed." }

$cliSrc = Join-Path $RootDir "target\$Target\$BuildProfile\fleet-cli.exe"
if (-not (Test-Path $cliSrc)) { Die "Expected sidecar not found at $cliSrc" }

# 2. Stage sidecar as externalBin (copy only when content differs)
$binDir = Join-Path $Desktop 'binaries'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$sidecar = Join-Path $binDir "fleet-$Target.exe"
$needCopy = $true
if (Test-Path $sidecar) {
  $needCopy = (Get-FileHash $cliSrc).Hash -ne (Get-FileHash $sidecar).Hash
}
if ($needCopy) {
  Copy-Item $cliSrc $sidecar -Force
  Info "Staged sidecar -> $sidecar"
} else {
  Info "Sidecar unchanged (skip copy) -> $sidecar"
}

# Also drop a standalone CLI next to the installer for convenience.
$distDir = Join-Path $RootDir 'dist'
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
Copy-Item $cliSrc (Join-Path $distDir 'fleet-windows-x64.exe') -Force

# 3. Frontend deps
if (-not $SkipInstall) {
  Info "Installing frontend deps (pnpm)..."
  Push-Location $Desktop
  & pnpm install --frozen-lockfile
  $ok = $LASTEXITCODE -eq 0
  Pop-Location
  if (-not $ok) { Die "pnpm install failed." }
}

# 4. Build Tauri app + NSIS installer.
# beforeBuildCommand ("pnpm build") emits ./dist; we bundle only NSIS to skip
# the WiX/MSI toolchain. Tauri downloads NSIS itself on first run.
Info "Building Tauri app + NSIS installer ($BuildProfile)... (first run downloads NSIS)"
Push-Location $Desktop
$tauriArgs = @('build', '--target', $Target, '--bundles', 'nsis')
if ($DebugProfile) { $tauriArgs += '--debug' }
& pnpm exec tauri @tauriArgs
$ok = $LASTEXITCODE -eq 0
Pop-Location
if (-not $ok) { Die "tauri build failed." }

# 5. Collect the installer
$nsisDir = Join-Path $RootDir "target\$Target\$BuildProfile\bundle\nsis"
$setup = Get-ChildItem $nsisDir -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) {
  $setup = Get-ChildItem $nsisDir -Filter '*.exe' -ErrorAction SilentlyContinue |
           Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $setup) { Die "No NSIS installer found under $nsisDir" }

$out = Join-Path $distDir 'claw-fleet-windows-x64-setup.exe'
Copy-Item $setup.FullName $out -Force

Write-Host ""
Info "Done."
Write-Host "    Installer : $out" -ForegroundColor Green
Write-Host "    (source)  : $($setup.FullName)"
Write-Host "    CLI       : $(Join-Path $distDir 'fleet-windows-x64.exe')"
Write-Host ""
Write-Host "Run the installer, or open the folder with:  explorer `"$distDir`""
