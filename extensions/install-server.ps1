$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..')).Path
$SharedServersDir = Join-Path $ScriptDir 'shared\servers'
$PathInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\rsml-lsp\bin'
$BinaryName = 'rsml-lsp-windows-x86_64.exe'

Write-Host 'Building rsml-lsp (release)...'
cargo build --release --manifest-path (Join-Path $RepoRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

Write-Host 'Building Zed tree-sitter WASM grammar...'
$GrammarDir = Join-Path $ScriptDir 'zed\grammars\rsml'
$TreeSitter = Join-Path $HOME '.cargo\bin\tree-sitter.exe'

if (-not (Test-Path $TreeSitter)) {
    $TreeSitter = 'tree-sitter'
}

& $TreeSitter build --wasm -o (Join-Path $ScriptDir 'zed\grammars\rsml.wasm') $GrammarDir
if ($LASTEXITCODE -ne 0) { throw "tree-sitter build failed with exit code $LASTEXITCODE" }

$BuiltBinary = Join-Path $RepoRoot 'target\release\rsml-lsp.exe'

New-Item -ItemType Directory -Force -Path $SharedServersDir | Out-Null
Copy-Item -Force $BuiltBinary (Join-Path $SharedServersDir $BinaryName)
Write-Host "Installed $BinaryName to $SharedServersDir"

New-Item -ItemType Directory -Force -Path $PathInstallDir | Out-Null
Copy-Item -Force $BuiltBinary (Join-Path $PathInstallDir $BinaryName)
Write-Host "Installed $BinaryName to $PathInstallDir"

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')

if ($UserPath -notlike "*$PathInstallDir*") {
    $NewPath = if ([string]::IsNullOrEmpty($UserPath)) { $PathInstallDir } else { "$UserPath;$PathInstallDir" }
    [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
    Write-Host "Added $PathInstallDir to user PATH. Open a new shell to pick it up."
}
