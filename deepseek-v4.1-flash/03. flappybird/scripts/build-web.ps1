<#
.SYNOPSIS
    Builds the WebAssembly package into web/pkg/.

.DESCRIPTION
    Run from anywhere; paths are resolved relative to this script.

    Requires:
      rustup target add wasm32-unknown-unknown
      cargo install wasm-bindgen-cli --version 0.2.126

    The wasm-bindgen CLI version MUST match the `wasm-bindgen` crate version in
    Cargo.toml exactly, or the CLI refuses to process the generated module.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

# Pinned to the `wasm-bindgen` crate version in the workspace Cargo.toml.
$RequiredBindgen = '0.2.126'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RepoRoot

function Fail($message) {
    Write-Host "error: $message" -ForegroundColor Red
    exit 1
}

$bindgen = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
if (-not $bindgen) {
    Fail "wasm-bindgen is not on PATH.`n       cargo install wasm-bindgen-cli --version $RequiredBindgen"
}

$actual = (& wasm-bindgen --version) -replace '^wasm-bindgen\s+', ''
if ($actual.Trim() -ne $RequiredBindgen) {
    Fail "wasm-bindgen CLI is $($actual.Trim()), but the crate is pinned to $RequiredBindgen.`n       cargo install wasm-bindgen-cli --version $RequiredBindgen --force"
}

$targets = (rustup target list --installed) -join "`n"
if ($targets -notmatch 'wasm32-unknown-unknown') {
    Fail "the wasm32-unknown-unknown target is not installed.`n       rustup target add wasm32-unknown-unknown"
}

Write-Host '==> building flappy_web (wasm-release)' -ForegroundColor Cyan
& cargo build --target wasm32-unknown-unknown --profile wasm-release -p flappy_web
if ($LASTEXITCODE -ne 0) { Fail 'cargo build failed' }

$wasm = 'target/wasm32-unknown-unknown/wasm-release/flappy_web.wasm'
if (-not (Test-Path $wasm)) { Fail "expected $wasm to exist after the build." }

Write-Host '==> generating web/pkg' -ForegroundColor Cyan
if (Test-Path 'web/pkg') { Remove-Item -Recurse -Force 'web/pkg' }
New-Item -ItemType Directory -Path 'web/pkg' | Out-Null

# --target web emits an ES module that index.html imports directly.
# --no-typescript skips the .d.ts files, which this project does not consume.
& wasm-bindgen --target web --no-typescript --out-dir web/pkg $wasm
if ($LASTEXITCODE -ne 0) { Fail 'wasm-bindgen failed' }

Write-Host '==> done' -ForegroundColor Green
Get-ChildItem web/pkg | Select-Object -ExpandProperty Name
Write-Host ''
Write-Host 'Serve it with:  python web/serve.py'
