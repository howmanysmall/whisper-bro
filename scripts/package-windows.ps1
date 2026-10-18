param([string]$Binary)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (-not $Binary) {
    $target = if ($env:CARGO_BUILD_TARGET) { "target/$env:CARGO_BUILD_TARGET" } else { 'target' }
    $Binary = Join-Path $root "$target/release/yap.exe"
}
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$archive = Join-Path $dist 'yap-windows-x64-portable.zip'
Compress-Archive -Force -DestinationPath $archive -LiteralPath @(
    $Binary,
    (Join-Path $root 'LICENSE'),
    (Join-Path $root 'packaging/Configure Yap.cmd')
)
Write-Output "Packaged $archive"
