$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

# Cargo.toml's [package] version is the single source of truth; it is the first
# top-level `version` key in the manifest.
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"$' | Select-Object -First 1
if (-not $versionLine) { throw 'Could not read the package version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value

$target = if ($env:CARGO_BUILD_TARGET) { "target/$env:CARGO_BUILD_TARGET" } else { 'target' }
$binary = [System.IO.Path]::GetFullPath((Join-Path $root "$target/release/whisper-bro.exe"))
if (-not (Test-Path $binary)) { throw "Missing $binary. Build it with mise run build first." }

$iscc = $null
$command = Get-Command iscc -ErrorAction SilentlyContinue
if ($command) {
    $iscc = $command.Source
} elseif (Test-Path "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") {
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
} elseif (Test-Path "$env:ProgramFiles\Inno Setup 6\ISCC.exe") {
    $iscc = "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
} else {
    throw 'Inno Setup 6 is required to build the installer: https://jrsoftware.org/isinfo.php'
}

& $iscc (Join-Path $root 'packaging/windows.iss') "/DAppVersion=$version" "/DAppBinary=$binary"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Output "Packaged $(Join-Path $root 'dist\whisper-bro-windows-x64-setup.exe')"
