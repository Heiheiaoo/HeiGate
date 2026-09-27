param(
    [string]$Version = '',
    [ValidateSet('win-x64', 'win-arm64')]
    [string]$Runtime = 'win-x64',
    [string]$OutputDir = '',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $Version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
}
$arch = $Runtime.Substring(4)
if (-not $OutputDir) {
    $OutputDir = Join-Path $repoRoot "dist\windows-$arch"
}
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$distDir = Join-Path $repoRoot 'dist'

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot 'build-windows.ps1') -Version $Version -Runtime $Runtime -OutputDir $OutputDir
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
}

$zipPath = Join-Path $distDir "HeiGate-Windows-$arch-$Version.zip"
$installerPath = Join-Path $distDir "HeiGate-Windows-$arch-$Version-Setup.exe"
Remove-Item -Force -ErrorAction SilentlyContinue $zipPath, $installerPath
Compress-Archive -Path (Join-Path $OutputDir '*') -DestinationPath $zipPath -CompressionLevel Optimal

$makeNsis = Get-Command makensis -ErrorAction SilentlyContinue
if (-not $makeNsis) {
    $knownPath = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'
    if (Test-Path $knownPath) {
        $makeNsis = Get-Item $knownPath
    }
}
if (-not $makeNsis) {
    throw 'makensis not found. Install NSIS first (choco install nsis).'
}
$makeNsisPath = if ($makeNsis -is [System.IO.FileInfo]) { $makeNsis.FullName } else { $makeNsis.Source }

Push-Location (Join-Path $repoRoot 'desktop\windows')
try {
    # installer.nsi is UTF-8 without a BOM; on Windows makensis would otherwise
    # read it with the ANSI code page and mangle the Chinese shortcut names.
    & $makeNsisPath /INPUTCHARSET UTF8 "/DVERSION=$Version" "/DSOURCE_DIR=$OutputDir" "/DOUTPUT_FILE=$installerPath" installer.nsi
    if ($LASTEXITCODE -ne 0) { throw 'NSIS installer build failed' }
} finally {
    Pop-Location
}

Write-Host "Packaged $zipPath"
Write-Host "Packaged $installerPath"
