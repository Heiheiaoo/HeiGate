param(
    [string]$Version = '',
    [ValidateSet('win-x64', 'win-arm64')]
    [string]$Runtime = 'win-x64',
    [string]$OutputDir = '',
    [switch]$SkipFrontend
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $Version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
}
if (-not $OutputDir) {
    $OutputDir = Join-Path $repoRoot "dist\windows-$($Runtime.Substring(4))"
}
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)

Push-Location $repoRoot
try {
    if (-not $SkipFrontend) {
        & pnpm --dir frontend install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'pnpm install failed' }
        & pnpm --dir frontend run build
        if ($LASTEXITCODE -ne 0) { throw 'frontend build failed' }
    }

    & powershell -NoProfile -ExecutionPolicy Bypass -File desktop/windows/generate-icon.ps1
    if ($LASTEXITCODE -ne 0) { throw 'Windows icon generation failed' }

    if (Test-Path $OutputDir) {
        Remove-Item -Recurse -Force $OutputDir
    }
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

    & dotnet publish desktop/windows/HeiGate.csproj `
        --configuration Release `
        --runtime $Runtime `
        --self-contained true `
        -p:Version=$Version `
        -p:InformationalVersion=$Version `
        --output $OutputDir
    if ($LASTEXITCODE -ne 0) { throw 'Windows desktop shell build failed' }

    $goArch = if ($Runtime -eq 'win-arm64') { 'arm64' } else { 'amd64' }
    $oldCgo = $env:CGO_ENABLED
    $oldGoos = $env:GOOS
    $oldGoarch = $env:GOARCH
    try {
        $env:CGO_ENABLED = '0'
        $env:GOOS = 'windows'
        $env:GOARCH = $goArch
        & go build -tags embed -trimpath "-ldflags=-s -w -X main.version=$Version" -o (Join-Path $OutputDir 'heigate-gateway.exe') ./cmd/heigate
        if ($LASTEXITCODE -ne 0) { throw 'Windows gateway build failed' }
    } finally {
        $env:CGO_ENABLED = $oldCgo
        $env:GOOS = $oldGoos
        $env:GOARCH = $oldGoarch
    }

    if (-not (Test-Path (Join-Path $OutputDir 'HeiGate.exe'))) { throw 'HeiGate.exe was not produced' }
    if (-not (Test-Path (Join-Path $OutputDir 'heigate-gateway.exe'))) { throw 'heigate-gateway.exe was not produced' }
    Write-Host "Built Windows app: $OutputDir"
} finally {
    Pop-Location
}
